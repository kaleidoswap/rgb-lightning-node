use bitcoin::blockdata::transaction::Transaction;
use bitcoin::consensus::encode;
use bitcoin::{Script, Txid};
use electrum_client::{Client as ElectrumClient, ElectrumApi};
use esplora_client::blocking::BlockingClient as EsploraBlockingClient;
use esplora_client::Builder as EsploraBuilder;
use lightning::chain::chaininterface::{BroadcasterInterface, ConfirmationTarget, FeeEstimator};
use lightning::chain::{BestBlock, Confirm, Filter, WatchedOutput};
use lightning::log_warn;
use lightning::util::logger::Logger;
use lightning_transaction_sync::{ElectrumSyncClient, EsploraSyncClient};
use rgb_lib::wallet::rust_only::IndexerProtocol as RgbLibIndexerProtocol;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::disk::FilesystemLogger;
#[cfg(test)]
use crate::test::mock_fee;

type Confirmable = Arc<dyn Confirm + Send + Sync>;

/// The minimum feerate we are allowed to send, as specified by LDK.
const MIN_FEERATE: u32 = 253;

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

pub(crate) enum IndexerSyncClient {
    Electrum(ElectrumSyncClient<Arc<FilesystemLogger>>),
    Esplora(EsploraSyncClient<Arc<FilesystemLogger>>),
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
                let _ = client.server_features().map_err(|e| {
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
                let _ = client.get_tip_hash().map_err(|e| {
                    io::Error::other(format!("failed to connect to esplora server: {e}"))
                })?;
                let _ = client.get_height().map_err(|e| {
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
                let tip_height = client.get_height().map_err(|e| {
                    io::Error::other(format!("failed to fetch esplora tip height: {e}"))
                })?;
                Ok(BestBlock::new(tip_hash, tip_height))
            }
        }
    }
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
                Ok(Self::Electrum(client))
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
            Self::Electrum(client) => client
                .sync(confirmables)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) }),
            Self::Esplora(client) => client
                .sync(confirmables)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) }),
        }
    }
}

impl Filter for IndexerSyncClient {
    fn register_tx(&self, txid: &Txid, script_pubkey: &Script) {
        match self {
            Self::Electrum(client) => client.register_tx(txid, script_pubkey),
            Self::Esplora(client) => client.register_tx(txid, script_pubkey),
        }
    }

    fn register_output(&self, output: WatchedOutput) {
        match self {
            Self::Electrum(client) => client.register_output(output),
            Self::Esplora(client) => client.register_output(output),
        }
    }
}

impl FeeEstimator for IndexerClient {
    fn get_est_sat_per_1000_weight(&self, confirmation_target: ConfirmationTarget) -> u32 {
        let fee = self
            .fees
            .get(&confirmation_target)
            .unwrap()
            .load(Ordering::Acquire);
        #[cfg(test)]
        let fee = mock_fee(fee);
        fee
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
                        for tx in txs {
                            client.transaction_broadcast_raw(&tx)?;
                        }
                        Ok::<(), electrum_client::Error>(())
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
                        for tx in txs {
                            client.broadcast(&tx)?;
                        }
                        Ok::<(), esplora_client::Error>(())
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

fn default_fee_buckets() -> HashMap<ConfirmationTarget, AtomicU32> {
    let mut fees = HashMap::new();
    fees.insert(
        ConfirmationTarget::MaximumFeeEstimate,
        AtomicU32::new(50000),
    );
    fees.insert(ConfirmationTarget::UrgentOnChainSweep, AtomicU32::new(5000));
    fees.insert(
        ConfirmationTarget::MinAllowedAnchorChannelRemoteFee,
        AtomicU32::new(MIN_FEERATE),
    );
    fees.insert(
        ConfirmationTarget::MinAllowedNonAnchorChannelRemoteFee,
        AtomicU32::new(MIN_FEERATE),
    );
    fees.insert(
        ConfirmationTarget::AnchorChannelFee,
        AtomicU32::new(MIN_FEERATE),
    );
    fees.insert(
        ConfirmationTarget::NonAnchorChannelFee,
        AtomicU32::new(2000),
    );
    fees.insert(
        ConfirmationTarget::ChannelCloseMinimum,
        AtomicU32::new(MIN_FEERATE),
    );
    fees.insert(
        ConfirmationTarget::OutputSpendingFee,
        AtomicU32::new(MIN_FEERATE),
    );
    fees
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

                    fees.get(&ConfirmationTarget::MaximumFeeEstimate)
                        .unwrap()
                        .store(very_high_prio_estimate, Ordering::Release);
                    fees.get(&ConfirmationTarget::UrgentOnChainSweep)
                        .unwrap()
                        .store(high_prio_estimate, Ordering::Release);
                    fees.get(&ConfirmationTarget::MinAllowedAnchorChannelRemoteFee)
                        .unwrap()
                        .store(MIN_FEERATE, Ordering::Release);
                    fees.get(&ConfirmationTarget::MinAllowedNonAnchorChannelRemoteFee)
                        .unwrap()
                        .store(background_estimate.saturating_sub(250), Ordering::Release);
                    fees.get(&ConfirmationTarget::AnchorChannelFee)
                        .unwrap()
                        .store(background_estimate, Ordering::Release);
                    fees.get(&ConfirmationTarget::NonAnchorChannelFee)
                        .unwrap()
                        .store(normal_estimate, Ordering::Release);
                    fees.get(&ConfirmationTarget::ChannelCloseMinimum)
                        .unwrap()
                        .store(background_estimate, Ordering::Release);
                    fees.get(&ConfirmationTarget::OutputSpendingFee)
                        .unwrap()
                        .store(background_estimate, Ordering::Release);
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

                    fees.get(&ConfirmationTarget::MaximumFeeEstimate)
                        .unwrap()
                        .store(very_high_prio_estimate, Ordering::Release);
                    fees.get(&ConfirmationTarget::UrgentOnChainSweep)
                        .unwrap()
                        .store(high_prio_estimate, Ordering::Release);
                    fees.get(&ConfirmationTarget::MinAllowedAnchorChannelRemoteFee)
                        .unwrap()
                        .store(MIN_FEERATE, Ordering::Release);
                    fees.get(&ConfirmationTarget::MinAllowedNonAnchorChannelRemoteFee)
                        .unwrap()
                        .store(background_estimate.saturating_sub(250), Ordering::Release);
                    fees.get(&ConfirmationTarget::AnchorChannelFee)
                        .unwrap()
                        .store(background_estimate, Ordering::Release);
                    fees.get(&ConfirmationTarget::NonAnchorChannelFee)
                        .unwrap()
                        .store(normal_estimate, Ordering::Release);
                    fees.get(&ConfirmationTarget::ChannelCloseMinimum)
                        .unwrap()
                        .store(background_estimate, Ordering::Release);
                    fees.get(&ConfirmationTarget::OutputSpendingFee)
                        .unwrap()
                        .store(background_estimate, Ordering::Release);
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
