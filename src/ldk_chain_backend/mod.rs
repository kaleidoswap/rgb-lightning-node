//! Chain backends LDK can sync against.
//!
//! Two backends are provided, each behind a Cargo feature and selected at
//! unlock time via [`LdkChainSync`](crate::routes::LdkChainSync):
//!
//! - [`block_sync`]: full blocks from a trusted/local bitcoind
//!   (`lightning-block-sync`).
//! - [`transaction_sync`]: an electrum/esplora indexer
//!   (`lightning-transaction-sync`).
//!
//! This module holds the pieces shared by both backends (fee-estimate buckets,
//! the fee estimator body and the fee-estimate polling logic) plus the
//! backend-agnostic objects the rest of the node wires against.

#[cfg(feature = "block-sync")]
pub(crate) mod block_sync;
#[cfg(feature = "transaction-sync")]
pub(crate) mod transaction_sync;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use lightning::chain::chaininterface::ConfirmationTarget;
#[cfg(feature = "transaction-sync")]
use lightning::chain::Confirm;
use lightning::chain::{BestBlock, Filter};

/// Fee estimator shared by both chain backends, used as a trait object so a
/// single set of LDK type aliases works regardless of the selected sync mode.
pub(crate) type DynFeeEstimator = dyn lightning::chain::chaininterface::FeeEstimator + Send + Sync;
/// Transaction broadcaster shared by both chain backends, see [`DynFeeEstimator`].
pub(crate) type DynBroadcaster =
    dyn lightning::chain::chaininterface::BroadcasterInterface + Send + Sync;

/// The minimum feerate (sat/kw) LDK allows us to use.
pub(crate) const MIN_FEERATE: u32 = 253;

/// The chain backend LDK syncs against, selected at unlock time via the
/// requested [`LdkChainSync`](crate::routes::LdkChainSync) sync mode.
///
/// Both variants provide the [`FeeEstimator`](lightning::chain::chaininterface::FeeEstimator)
/// and [`BroadcasterInterface`](lightning::chain::chaininterface::BroadcasterInterface)
/// LDK needs; they differ in how blocks/transactions are synced and in the
/// UTXO lookup used for gossip verification.
pub(crate) enum ChainBackend {
    /// Sync full blocks from a trusted/local bitcoind over JSON-RPC via
    /// `lightning-block-sync`. Carries the initial validated chain tip used to
    /// bootstrap the SPV client.
    #[cfg(feature = "block-sync")]
    BlockSync {
        client: Arc<block_sync::BitcoindClient>,
        polled_chain_tip: lightning_block_sync::poll::ValidatedBlockHeader,
    },
    /// Sync through the configured electrum/esplora indexer via
    /// `lightning-transaction-sync`.
    #[cfg(feature = "transaction-sync")]
    TransactionSync {
        client: Arc<transaction_sync::IndexerClient>,
        tx_sync: Arc<transaction_sync::IndexerSyncClient>,
    },
}

/// Everything produced while initializing the chain backend that the rest of
/// `start_ldk` needs: the backend itself, the fee estimator / broadcaster LDK
/// wires everywhere, the optional chain source (`Filter`, set only in
/// transaction-sync mode) and the chain tip used to bootstrap a fresh node.
pub(crate) struct ChainSetup {
    pub(crate) backend: ChainBackend,
    pub(crate) fee_estimator: Arc<DynFeeEstimator>,
    pub(crate) broadcaster: Arc<DynBroadcaster>,
    pub(crate) chain_filter: Option<Arc<dyn Filter + Send + Sync>>,
    pub(crate) initial_best_block: BestBlock,
}

/// Run a single transaction-sync pass against the indexer on a blocking thread.
#[cfg(feature = "transaction-sync")]
pub(crate) async fn sync_chain_data(
    tx_sync: Arc<transaction_sync::IndexerSyncClient>,
    confirmables: Vec<Arc<dyn Confirm + Send + Sync>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tokio::task::spawn_blocking(move || tx_sync.sync(confirmables))
        .await
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
}

/// The default fee-estimate buckets, one per [`ConfirmationTarget`], used until
/// the backend's polling loop refreshes them.
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

/// Read the current estimate for `confirmation_target` from the shared buckets.
/// This is the body of both backends' [`FeeEstimator`] implementations.
fn fee_from_bucket(
    fees: &HashMap<ConfirmationTarget, AtomicU32>,
    confirmation_target: ConfirmationTarget,
) -> u32 {
    let fee = fees
        .get(&confirmation_target)
        .unwrap()
        .load(Ordering::Acquire);
    #[cfg(test)]
    let fee = crate::test::mock_fee(fee);
    fee
}

/// Store freshly-polled fee estimates into the shared buckets. Both backends
/// map their four priority estimates onto the [`ConfirmationTarget`]s the same
/// way; they differ only in the value used for
/// [`ConfirmationTarget::MinAllowedAnchorChannelRemoteFee`], passed as
/// `min_allowed_anchor`.
fn store_fee_estimates(
    fees: &HashMap<ConfirmationTarget, AtomicU32>,
    background: u32,
    normal: u32,
    high_prio: u32,
    very_high_prio: u32,
    min_allowed_anchor: u32,
) {
    let set = |target: ConfirmationTarget, value: u32| {
        fees.get(&target).unwrap().store(value, Ordering::Release);
    };
    set(ConfirmationTarget::MaximumFeeEstimate, very_high_prio);
    set(ConfirmationTarget::UrgentOnChainSweep, high_prio);
    set(
        ConfirmationTarget::MinAllowedAnchorChannelRemoteFee,
        min_allowed_anchor,
    );
    set(
        ConfirmationTarget::MinAllowedNonAnchorChannelRemoteFee,
        background.saturating_sub(250),
    );
    set(ConfirmationTarget::AnchorChannelFee, background);
    set(ConfirmationTarget::NonAnchorChannelFee, normal);
    set(ConfirmationTarget::ChannelCloseMinimum, background);
    set(ConfirmationTarget::OutputSpendingFee, background);
}
