use bitcoin::block::Header;
use bitcoin::blockdata::transaction::Transaction;
use bitcoin::consensus::encode;
use bitcoin::constants::ChainHash;
use bitcoin::{BlockHash, Script, ScriptBuf, Txid};
use electrum_client::utils::validate_merkle_proof;
use electrum_client::{Client as ElectrumClient, ElectrumApi, Param};
use esplora_client::blocking::BlockingClient as EsploraBlockingClient;
use esplora_client::Builder as EsploraBuilder;
use lightning::chain::chaininterface::{BroadcasterInterface, ConfirmationTarget, FeeEstimator};
use lightning::chain::{BestBlock, Confirm, Filter, WatchedOutput};
use lightning::log_warn;
use lightning::routing::utxo::{UtxoFuture, UtxoLookup, UtxoLookupError, UtxoResult};
use lightning::util::logger::Logger;
use lightning_transaction_sync::{ElectrumSyncClient, EsploraSyncClient};
use rgb_lib::wallet::rust_only::IndexerProtocol as RgbLibIndexerProtocol;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::str::FromStr;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::disk::FilesystemLogger;
use crate::ldk::PeerGossipSync;

use super::{default_fee_buckets, fee_from_bucket, store_fee_estimates, MIN_FEERATE};

type Confirmable = Arc<dyn Confirm + Send + Sync>;

enum IndexerBackend {
    Electrum(Arc<ElectrumClient>),
    Esplora(Arc<EsploraBlockingClient>),
}

pub(crate) struct IndexerClient {
    backend: IndexerBackend,
    fees: Arc<HashMap<ConfirmationTarget, AtomicU32>>,
    handle: tokio::runtime::Handle,
    logger: Arc<FilesystemLogger>,
}

pub(crate) struct IndexerGossipVerifier {
    client: Arc<IndexerClient>,
    gossiper: Arc<PeerGossipSync>,
    peer_manager_wake: Arc<dyn Fn() + Send + Sync>,
}

pub(crate) enum IndexerSyncClient {
    Electrum {
        client: ElectrumSyncClient<Arc<FilesystemLogger>>,
        registered_txs: Mutex<HashMap<Txid, RegisteredTx>>,
    },
    Esplora(EsploraSyncClient<Arc<FilesystemLogger>>),
}

#[derive(Clone)]
pub(crate) struct RegisteredTx {
    script_pubkey: ScriptBuf,
    confirmed: Option<(u32, BlockHash)>,
}

struct ConfirmedRegisteredTx {
    tx: Transaction,
    header: Header,
    height: u32,
    pos: usize,
}

impl IndexerClient {
    pub(crate) fn new(
        server_url: String,
        protocol: RgbLibIndexerProtocol,
        handle: tokio::runtime::Handle,
        logger: Arc<FilesystemLogger>,
    ) -> io::Result<Self> {
        let fees = Arc::new(default_fee_buckets());
        let backend = match protocol {
            RgbLibIndexerProtocol::Electrum => {
                let client = Arc::new(ElectrumClient::new(&server_url).map_err(|e| {
                    io::Error::other(format!("failed to connect to electrum server: {e}"))
                })?);
                client.server_features().map_err(|e| {
                    io::Error::other(format!("failed to query electrum server features: {e}"))
                })?;
                poll_electrum_fee_estimates(
                    fees.clone(),
                    client.clone(),
                    logger.clone(),
                    handle.clone(),
                );
                IndexerBackend::Electrum(client)
            }
            RgbLibIndexerProtocol::Esplora => {
                let client = Arc::new(EsploraBuilder::new(&server_url).build_blocking());
                client.get_tip_hash().map_err(|e| {
                    io::Error::other(format!("failed to connect to esplora server: {e}"))
                })?;
                client.get_height().map_err(|e| {
                    io::Error::other(format!("failed to query esplora tip height: {e}"))
                })?;
                poll_esplora_fee_estimates(
                    fees.clone(),
                    client.clone(),
                    logger.clone(),
                    handle.clone(),
                );
                IndexerBackend::Esplora(client)
            }
        };

        Ok(Self {
            backend,
            fees,
            handle,
            logger,
        })
    }

    pub(crate) fn get_best_block(&self) -> io::Result<BestBlock> {
        match &self.backend {
            IndexerBackend::Electrum(client) => {
                let tip = client.block_headers_subscribe().map_err(|e| {
                    io::Error::other(format!("failed to fetch electrum tip header: {e}"))
                })?;
                Ok(BestBlock::new(tip.header.block_hash(), tip.height as u32))
            }
            IndexerBackend::Esplora(client) => {
                let tip_hash = client.get_tip_hash().map_err(|e| {
                    io::Error::other(format!("failed to fetch esplora tip hash: {e}"))
                })?;
                // Derive the height from the tip hash we just fetched (rather
                // than a separate `get_height` call) so the hash and height stay
                // consistent even if a new block arrives between requests.
                let tip_height = client
                    .get_block_status(&tip_hash)
                    .map_err(|e| {
                        io::Error::other(format!("failed to fetch esplora tip status: {e}"))
                    })?
                    .height
                    .ok_or_else(|| io::Error::other("esplora tip block has no height"))?;
                Ok(BestBlock::new(tip_hash, tip_height))
            }
        }
    }

    /// Current chain tip height, used to enforce the gossip confirmation depth.
    fn tip_height(&self) -> io::Result<u32> {
        match &self.backend {
            IndexerBackend::Electrum(client) => {
                let tip = client.block_headers_subscribe().map_err(|e| {
                    io::Error::other(format!("failed to fetch electrum tip header: {e}"))
                })?;
                Ok(tip.height as u32)
            }
            IndexerBackend::Esplora(client) => client
                .get_height()
                .map_err(|e| io::Error::other(format!("failed to fetch esplora tip height: {e}"))),
        }
    }
}

impl IndexerGossipVerifier {
    pub(crate) fn new(
        client: Arc<IndexerClient>,
        gossiper: Arc<PeerGossipSync>,
        peer_manager_wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            client,
            gossiper,
            peer_manager_wake,
        }
    }
}

impl UtxoLookup for IndexerGossipVerifier {
    // The chain hash is intentionally ignored: LDK already validates it before
    // invoking the UTXO lookup, keeping this symmetric with the block-sync
    // gossip verifier.
    fn get_utxo(&self, _chain_hash: &ChainHash, short_channel_id: u64) -> UtxoResult {
        let result = UtxoFuture::new();
        let future = result.clone();
        let client = self.client.clone();
        let gossiper = self.gossiper.clone();
        let peer_manager_wake = self.peer_manager_wake.clone();
        self.client.handle.spawn(async move {
            let lookup = tokio::task::spawn_blocking(move || {
                let height = (short_channel_id >> 40) as u32;
                let tx_index = ((short_channel_id >> 16) & 0x00ff_ffff) as usize;
                let vout = (short_channel_id & 0xffff) as usize;

                // Match the block-sync gossip verifier: require the funding
                // output to be buried by at least six confirmations (with one
                // block of headroom) before resolving it, per the BOLT gossip
                // rules.
                let tip_height = client
                    .tip_height()
                    .map_err(|_| UtxoLookupError::UnknownTx)?;
                if height + 5 > tip_height {
                    return Err(UtxoLookupError::UnknownTx);
                }

                let txout = match &client.backend {
                    IndexerBackend::Electrum(c) => {
                        match electrum_txid_from_pos(c, height as usize, tx_index)
                            .and_then(|txid| c.transaction_get(&txid))
                        {
                            Ok(tx) => tx.output.get(vout).cloned(),
                            Err(_) => None,
                        }
                    }
                    IndexerBackend::Esplora(c) => c
                        .get_block_hash(height)
                        .and_then(|block_hash| c.get_txid_at_block_index(&block_hash, tx_index))
                        .and_then(|txid| match txid {
                            Some(txid) => c.get_tx_no_opt(&txid).map(Some),
                            None => Ok(None),
                        })
                        .ok()
                        .flatten()
                        .and_then(|tx| tx.output.get(vout).cloned()),
                };

                txout.ok_or(UtxoLookupError::UnknownTx)
            })
            .await
            .unwrap_or(Err(UtxoLookupError::UnknownTx));
            future.resolve(gossiper.network_graph(), &*gossiper, lookup);
            peer_manager_wake();
        });
        UtxoResult::Async(result)
    }
}

fn electrum_txid_from_pos(
    client: &ElectrumClient,
    height: usize,
    tx_pos: usize,
) -> Result<Txid, electrum_client::Error> {
    let value = client.raw_call(
        "blockchain.transaction.id_from_pos",
        [
            Param::Usize(height),
            Param::Usize(tx_pos),
            Param::Bool(true),
        ],
    )?;
    let txid = value
        .as_str()
        .or_else(|| value.get("tx_hash").and_then(serde_json::Value::as_str))
        .or_else(|| value.get("txid").and_then(serde_json::Value::as_str))
        .or_else(|| value.get("tx_id").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .ok_or_else(|| electrum_client::Error::InvalidResponse(value.clone()))?;

    Txid::from_str(&txid).map_err(|_| electrum_client::Error::InvalidResponse(value))
}

impl IndexerSyncClient {
    pub(crate) fn new(
        server_url: String,
        protocol: RgbLibIndexerProtocol,
        logger: Arc<FilesystemLogger>,
    ) -> io::Result<Self> {
        match protocol {
            RgbLibIndexerProtocol::Electrum => {
                let client = ElectrumSyncClient::new(server_url, logger).map_err(|e| {
                    io::Error::other(format!("failed to initialize electrum sync client: {e}"))
                })?;
                Ok(Self::Electrum {
                    client,
                    registered_txs: Mutex::new(HashMap::new()),
                })
            }
            RgbLibIndexerProtocol::Esplora => {
                Ok(Self::Esplora(EsploraSyncClient::new(server_url, logger)))
            }
        }
    }

    pub(crate) fn sync(
        &self,
        confirmables: Vec<Confirmable>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self {
            Self::Electrum {
                client,
                registered_txs,
            } => {
                client
                    .sync(confirmables.clone())
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
                // Supplementary confirmation pass for transactions registered via
                // `Filter::register_tx`. Empirically, `ElectrumSyncClient::sync`
                // alone does not confirm these (the channel funding transaction
                // never reaches `channel_ready`, hanging the
                // `transaction_sync_electrum` test at funding lock-in), whereas
                // the esplora client does. Until that upstream gap is closed we
                // confirm registered transactions directly against the electrum
                // server here.
                sync_electrum_registered_txs(client.client(), registered_txs, &confirmables)
            }
            Self::Esplora(client) => client
                .sync(confirmables)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) }),
        }
    }
}

impl Filter for IndexerSyncClient {
    fn register_tx(&self, txid: &Txid, script_pubkey: &Script) {
        match self {
            Self::Electrum {
                client,
                registered_txs,
            } => {
                registered_txs.lock().unwrap().insert(
                    *txid,
                    RegisteredTx {
                        script_pubkey: script_pubkey.to_owned(),
                        confirmed: None,
                    },
                );
                client.register_tx(txid, script_pubkey);
            }
            Self::Esplora(client) => client.register_tx(txid, script_pubkey),
        }
    }

    fn register_output(&self, output: WatchedOutput) {
        match self {
            Self::Electrum { client, .. } => client.register_output(output),
            Self::Esplora(client) => client.register_output(output),
        }
    }
}

/// Confirm transactions registered through [`Filter::register_tx`] directly
/// against the electrum server.
///
/// This supplements [`ElectrumSyncClient::sync`], which in our setup does not
/// notify the confirmables about these registered transactions (see the caller
/// for details). The electrum calls are made without holding the
/// `registered_txs` lock, so registrations from LDK are never blocked on
/// network round-trips.
fn sync_electrum_registered_txs(
    client: Arc<ElectrumClient>,
    registered_txs: &Mutex<HashMap<Txid, RegisteredTx>>,
    confirmables: &[Confirmable],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Snapshot the registered transactions so the blocking electrum lookups
    // below run without the lock held.
    let snapshot: Vec<(Txid, RegisteredTx)> = {
        let registered_txs = registered_txs.lock().unwrap();
        registered_txs
            .iter()
            .map(|(txid, tx)| (*txid, tx.clone()))
            .collect()
    };

    let mut confirmed = Vec::new();
    let mut unconfirmed = Vec::new();

    for (txid, registered_tx) in snapshot {
        let history = client.script_get_history(&registered_tx.script_pubkey)?;
        let confirmed_history = history
            .iter()
            .find(|history| history.tx_hash == txid && history.height > 0);

        let Some(confirmed_history) = confirmed_history else {
            if registered_tx.confirmed.is_some() {
                unconfirmed.push(txid);
            }
            continue;
        };

        let height = confirmed_history.height as u32;
        let tx = client.transaction_get(&txid)?;
        let merkle_res = client.transaction_get_merkle(&txid, height as usize)?;
        let header = client.block_header(height as usize)?;
        if !validate_merkle_proof(&txid, &header.merkle_root, &merkle_res) {
            return Err(Box::new(io::Error::other(format!(
                "invalid merkle proof for transaction {txid}"
            ))));
        }

        let block_hash = header.block_hash();
        if registered_tx.confirmed == Some((height, block_hash)) {
            continue;
        }
        confirmed.push(ConfirmedRegisteredTx {
            tx,
            header,
            height,
            pos: merkle_res.pos,
        });
    }

    // Re-acquire the lock only to apply the observed state: drop confirmed
    // transactions and clear the confirmed marker on ones that went back to
    // unconfirmed. Entries registered concurrently are left for the next pass.
    {
        let mut registered_txs = registered_txs.lock().unwrap();
        for txid in &unconfirmed {
            if let Some(entry) = registered_txs.get_mut(txid) {
                entry.confirmed = None;
            }
        }
        for confirmed_tx in &confirmed {
            registered_txs.remove(&confirmed_tx.tx.compute_txid());
        }
    }

    for txid in unconfirmed {
        for confirmable in confirmables {
            confirmable.transaction_unconfirmed(&txid);
        }
    }
    for confirmed_tx in confirmed {
        for confirmable in confirmables {
            confirmable.transactions_confirmed(
                &confirmed_tx.header,
                &[(confirmed_tx.pos, &confirmed_tx.tx)],
                confirmed_tx.height,
            );
        }
    }

    Ok(())
}

impl FeeEstimator for IndexerClient {
    fn get_est_sat_per_1000_weight(&self, confirmation_target: ConfirmationTarget) -> u32 {
        fee_from_bucket(&self.fees, confirmation_target)
    }
}

impl BroadcasterInterface for IndexerClient {
    fn broadcast_transactions(&self, txs: &[&Transaction]) {
        match &self.backend {
            IndexerBackend::Electrum(client) => {
                let txs = txs
                    .iter()
                    .map(|tx| encode::serialize(*tx))
                    .collect::<Vec<_>>();
                let client = client.clone();
                let logger = self.logger.clone();
                self.handle.spawn(async move {
                    let res = tokio::task::spawn_blocking(move || {
                        let mut last_error = None;
                        for tx in txs {
                            if let Err(e) = client.transaction_broadcast_raw(&tx) {
                                last_error = Some(e.to_string());
                            }
                        }
                        last_error.map_or(Ok(()), Err)
                    })
                    .await;

                    match res {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            log_warn!(
                                logger,
                                "Warning, failed to broadcast transaction(s) via electrum: {}",
                                e
                            );
                        }
                        Err(e) => {
                            log_warn!(
                                logger,
                                "Warning, failed to spawn electrum broadcaster task: {}",
                                e
                            );
                        }
                    }
                });
            }
            IndexerBackend::Esplora(client) => {
                let txs = txs.iter().map(|tx| (*tx).clone()).collect::<Vec<_>>();
                let client = client.clone();
                let logger = self.logger.clone();
                self.handle.spawn(async move {
                    let res = tokio::task::spawn_blocking(move || {
                        let mut last_error = None;
                        for tx in txs {
                            if let Err(e) = client.broadcast(&tx) {
                                last_error = Some(e.to_string());
                            }
                        }
                        last_error.map_or(Ok(()), Err)
                    })
                    .await;

                    match res {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            log_warn!(
                                logger,
                                "Warning, failed to broadcast transaction(s) via esplora: {}",
                                e
                            );
                        }
                        Err(e) => {
                            log_warn!(
                                logger,
                                "Warning, failed to spawn esplora broadcaster task: {}",
                                e
                            );
                        }
                    }
                });
            }
        }
    }
}

fn poll_electrum_fee_estimates(
    fees: Arc<HashMap<ConfirmationTarget, AtomicU32>>,
    client: Arc<ElectrumClient>,
    logger: Arc<FilesystemLogger>,
    handle: tokio::runtime::Handle,
) {
    handle.spawn(async move {
        loop {
            let res = tokio::task::spawn_blocking({
                let client = client.clone();
                move || {
                    Ok::<_, electrum_client::Error>((
                        client.estimate_fee(144)?,
                        client.estimate_fee(18)?,
                        client.estimate_fee(6)?,
                        client.estimate_fee(2)?,
                    ))
                }
            })
            .await;

            match res {
                Ok(Ok((background, normal, high_prio, very_high_prio))) => {
                    let background_estimate =
                        fee_rate_from_btc_per_kb(background, MIN_FEERATE).unwrap_or(MIN_FEERATE);
                    let normal_estimate = fee_rate_from_btc_per_kb(normal, 2000).unwrap_or(2000);
                    let high_prio_estimate =
                        fee_rate_from_btc_per_kb(high_prio, 5000).unwrap_or(5000);
                    let very_high_prio_estimate =
                        fee_rate_from_btc_per_kb(very_high_prio, 50000).unwrap_or(50000);

                    store_fee_estimates(
                        &fees,
                        background_estimate,
                        normal_estimate,
                        high_prio_estimate,
                        very_high_prio_estimate,
                        MIN_FEERATE,
                    );
                }
                Ok(Err(e)) => {
                    log_warn!(logger, "Error getting fee estimate from electrum: {}", e);
                }
                Err(e) => {
                    log_warn!(logger, "Error polling electrum fee estimates: {}", e);
                }
            }

            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

fn poll_esplora_fee_estimates(
    fees: Arc<HashMap<ConfirmationTarget, AtomicU32>>,
    client: Arc<EsploraBlockingClient>,
    logger: Arc<FilesystemLogger>,
    handle: tokio::runtime::Handle,
) {
    handle.spawn(async move {
        loop {
            let res = tokio::task::spawn_blocking({
                let client = client.clone();
                move || client.get_fee_estimates()
            })
            .await;

            match res {
                Ok(Ok(estimate_map)) => {
                    let background_estimate =
                        estimate_fee_rate_sat_per_kw(&estimate_map, 144, MIN_FEERATE);
                    let normal_estimate = estimate_fee_rate_sat_per_kw(&estimate_map, 18, 2000);
                    let high_prio_estimate = estimate_fee_rate_sat_per_kw(&estimate_map, 6, 5000);
                    let very_high_prio_estimate =
                        estimate_fee_rate_sat_per_kw(&estimate_map, 2, 50000);

                    store_fee_estimates(
                        &fees,
                        background_estimate,
                        normal_estimate,
                        high_prio_estimate,
                        very_high_prio_estimate,
                        MIN_FEERATE,
                    );
                }
                Ok(Err(e)) => {
                    log_warn!(logger, "Error getting fee estimate from esplora: {}", e)
                }
                Err(e) => log_warn!(logger, "Error polling esplora fee estimates: {}", e),
            }

            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

fn estimate_fee_rate_sat_per_kw(
    fee_estimates: &HashMap<u16, f64>,
    blocks: u16,
    default: u32,
) -> u32 {
    let Some(sat_per_vb) = interpolate_fee_rate(fee_estimates, blocks) else {
        return default;
    };
    std::cmp::max((sat_per_vb * 250.0).round() as u32, MIN_FEERATE)
}

fn interpolate_fee_rate(fee_estimates: &HashMap<u16, f64>, blocks: u16) -> Option<f64> {
    if blocks == 0 || fee_estimates.is_empty() {
        return None;
    }

    let estimate_map = BTreeMap::from_iter(fee_estimates.iter().map(|(k, v)| (*k, *v)));
    if let Some(estimate) = estimate_map.get(&blocks) {
        return Some(*estimate);
    }

    let lower_key = estimate_map.range(..blocks).next_back().map(|(k, _)| *k);
    let upper_key = estimate_map.range(blocks..).next().map(|(k, _)| *k);

    match (lower_key, upper_key) {
        (Some(x1), Some(x2)) if x1 != x2 => {
            let y1 = estimate_map[&x1];
            let y2 = estimate_map[&x2];
            Some(y1 + (blocks as f64 - x1 as f64) / (x2 as f64 - x1 as f64) * (y2 - y1))
        }
        (Some(x), _) | (_, Some(x)) => estimate_map.get(&x).copied(),
        _ => None,
    }
}

fn fee_rate_from_btc_per_kb(feerate_btc_per_kb: f64, default: u32) -> Option<u32> {
    if !feerate_btc_per_kb.is_finite() || feerate_btc_per_kb.is_sign_negative() {
        return Some(default);
    }
    Some(std::cmp::max(
        (feerate_btc_per_kb * 100_000_000.0 / 4.0).round() as u32,
        MIN_FEERATE,
    ))
}
