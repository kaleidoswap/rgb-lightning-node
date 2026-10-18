use super::*;

use crate::ldk::UPDATE_WITNESSES_DURING_FUNDING;

const TEST_DIR_BASE: &str = "tmp/colored_funding_window/";

#[serial_test::serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[traced_test]
async fn colored_funding_window() {
    initialize();

    let test_dir_node1 = format!("{TEST_DIR_BASE}node1");
    let test_dir_node2 = format!("{TEST_DIR_BASE}node2");
    let (node1_addr, _) = start_node(&test_dir_node1, NODE1_PEER_PORT, false).await;
    let (node2_addr, _) = start_node(&test_dir_node2, NODE2_PEER_PORT, false).await;

    fund_and_create_utxos(node1_addr, None).await;
    fund_and_create_utxos(node2_addr, None).await;

    let asset_id = issue_asset_nia(node1_addr).await.asset_id;
    let node2_pubkey = node_info(node2_addr).await.pubkey;

    // a full witness update while the funding is still unbroadcast (as the output sweeper may run)
    // must not invalidate the funding allocation that the commitments are colored against
    UPDATE_WITNESSES_DURING_FUNDING.store(true, Ordering::SeqCst);
    open_channel(
        node1_addr,
        &node2_pubkey,
        Some(NODE2_PEER_PORT),
        None,
        None,
        Some(600),
        Some(&asset_id),
    )
    .await;
    assert!(!UPDATE_WITNESSES_DURING_FUNDING.load(Ordering::SeqCst));
    assert_eq!(list_channels(node1_addr).await.len(), 1);

    // the assets of the funding are committed to the channel and the node keeps working
    node_info(node1_addr).await;
    assert_eq!(
        asset_balance_spendable(node1_addr, &asset_id).await,
        ISSUE_AMT - 600
    );
}
