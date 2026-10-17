use super::*;

use crate::ldk::UPDATE_WITNESSES_DURING_FUNDING;
use lightning::rgb_utils::FAIL_RGB_COLORING;

const TEST_DIR_BASE: &str = "tmp/colored_funding_window/";

#[serial_test::serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[traced_test]
async fn colored_funding_window() {
    initialize();

    let test_dir_node1 = format!("{TEST_DIR_BASE}node1");
    let test_dir_node2 = format!("{TEST_DIR_BASE}node2");
    let test_dir_node3 = format!("{TEST_DIR_BASE}node3");
    let (node1_addr, _) = start_node(&test_dir_node1, NODE1_PEER_PORT, false).await;
    let (node2_addr, _) = start_node(&test_dir_node2, NODE2_PEER_PORT, false).await;
    let (node3_addr, _) = start_node(&test_dir_node3, NODE3_PEER_PORT, false).await;

    fund_and_create_utxos(node1_addr, None).await;
    fund_and_create_utxos(node2_addr, None).await;
    fund_and_create_utxos(node3_addr, None).await;

    let asset_id = issue_asset_nia(node1_addr).await.asset_id;
    let node2_pubkey = node_info(node2_addr).await.pubkey;
    let node3_pubkey = node_info(node3_addr).await.pubkey;

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

    // a coloring failure on funding_signed must fail the channel instead of crashing the node
    *FAIL_RGB_COLORING.lock().unwrap() = Some((PathBuf::from(&test_dir_node1).join(".ldk"), 1));
    open_channel_raw(
        node1_addr,
        &node3_pubkey,
        Some(NODE3_PEER_PORT),
        None,
        None,
        Some(100),
        Some(&asset_id),
        None,
        None,
        None,
        None,
        true,
        true,
    )
    .await
    .unwrap();
    let t_0 = OffsetDateTime::now_utc();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if FAIL_RGB_COLORING.lock().unwrap().is_none() && list_channels(node1_addr).await.len() == 1
        {
            break;
        }
        if (OffsetDateTime::now_utc() - t_0).as_seconds_f32() > 20.0 {
            panic!("failed channel is not disappearing");
        }
    }
    *FAIL_RGB_COLORING.lock().unwrap() = None;

    // the node is still up and the assets of the failed funding are spendable again
    node_info(node1_addr).await;
    assert_eq!(
        asset_balance_spendable(node1_addr, &asset_id).await,
        ISSUE_AMT - 600
    );
    open_channel_with_retry(
        node1_addr,
        &node3_pubkey,
        Some(NODE3_PEER_PORT),
        None,
        None,
        Some(100),
        Some(&asset_id),
        None,
        30,
    )
    .await;
}
