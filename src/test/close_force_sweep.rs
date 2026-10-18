use super::*;

use lightning::rgb_utils::WALLET_MASTER_FINGERPRINT_FNAME;

const TEST_DIR_BASE: &str = "tmp/close_force_sweep/";

fn sweep_generations(node_test_dir: &str) -> usize {
    ldk_log_lines(node_test_dir)
        .iter()
        .filter(|l| l.contains("Generating and broadcasting sweeping transaction"))
        .count()
}

fn rgb_lib_log(node_test_dir: &str) -> String {
    let fingerprint =
        std::fs::read_to_string(PathBuf::from(node_test_dir).join(WALLET_MASTER_FINGERPRINT_FNAME))
            .unwrap();
    std::fs::read_to_string(PathBuf::from(node_test_dir).join(fingerprint).join("log")).unwrap()
}

// LDK syncs through the indexer, so the sweeper only learns that its sweep confirmed if it
// registers the swept outputs with the chain source
#[serial_test::serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[traced_test]
async fn close_force_sweep() {
    initialize();

    let test_dir_node1 = format!("{TEST_DIR_BASE}node1");
    let test_dir_node2 = format!("{TEST_DIR_BASE}node2");
    let ldk_chain_sync = || LdkChainSync::TransactionSync {
        indexer_url: ELECTRUM_URL_REGTEST.to_string(),
    };
    let (node1_addr, _) =
        start_node_with(&test_dir_node1, NODE1_PEER_PORT, false, ldk_chain_sync()).await;
    let (node2_addr, _) =
        start_node_with(&test_dir_node2, NODE2_PEER_PORT, false, ldk_chain_sync()).await;

    fund_and_create_utxos(node1_addr, None).await;
    fund_and_create_utxos(node2_addr, None).await;

    let asset_id = issue_asset_nia(node1_addr).await.asset_id;

    let node2_pubkey = node_info(node2_addr).await.pubkey;

    let channel = open_channel(
        node1_addr,
        &node2_pubkey,
        Some(NODE2_PEER_PORT),
        None,
        Some(3000000),
        Some(600),
        Some(&asset_id),
    )
    .await;
    assert_eq!(asset_balance_spendable(node1_addr, &asset_id).await, 400);

    keysend(node1_addr, &node2_pubkey, None, Some(&asset_id), Some(150)).await;

    // this sleep prevents non-deterministic issue where force close broadcasts an old commitment
    // TX (one that still has an HTLC output)
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    close_channel(node1_addr, &channel.channel_id, &node2_pubkey, true).await;
    wait_for_balance(node1_addr, &asset_id, 850).await;
    wait_for_balance(node2_addr, &asset_id, 150).await;

    // confirm the sweeps, then make sure new blocks don't cause them to be regenerated
    mine(false);
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let node1_sweeps = sweep_generations(&test_dir_node1);
    let node2_sweeps = sweep_generations(&test_dir_node2);
    assert!(node1_sweeps > 0);
    assert!(node2_sweeps > 0);
    mine_n_blocks(false, 2);
    // the sweeper runs every 30 seconds
    tokio::time::sleep(std::time::Duration::from_secs(35)).await;
    assert_eq!(sweep_generations(&test_dir_node1), node1_sweeps);
    assert_eq!(sweep_generations(&test_dir_node2), node2_sweeps);

    // sweeping only touches the closing witness, never re-resolving the other ones
    assert!(!rgb_lib_log(&test_dir_node1).contains("Updating witnesses"));
    assert!(!rgb_lib_log(&test_dir_node2).contains("Updating witnesses"));

    // the swept assets are spendable
    let recipient_id = rgb_invoice(node2_addr, None, false).await.recipient_id;
    send_asset(
        node1_addr,
        &asset_id,
        Assignment::Fungible(800),
        recipient_id,
        None,
    )
    .await;
    mine(false);
    refresh_transfers(node2_addr).await;
    refresh_transfers(node2_addr).await;
    refresh_transfers(node1_addr).await;
    assert_eq!(asset_balance_spendable(node1_addr, &asset_id).await, 50);
    assert_eq!(asset_balance_spendable(node2_addr, &asset_id).await, 950);
}
