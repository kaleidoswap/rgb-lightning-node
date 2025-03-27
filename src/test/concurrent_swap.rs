use super::*;
use tokio::join;

const TEST_DIR_BASE: &str = "tmp/concurrent_swap/";

#[serial_test::serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[traced_test]
async fn concurrent_swap() {
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

    let asset_id = issue_asset_nia(node2_addr).await.asset_id;

    let node1_pubkey = node_info(node1_addr).await.pubkey;
    let node3_pubkey = node_info(node3_addr).await.pubkey;

    // Open channels from node2 to node1 and node3 with asset
    let channel_21 = open_channel(
        node2_addr,
        &node1_pubkey,
        Some(NODE1_PEER_PORT),
        Some(100000),
        Some(50000000),
        Some(100),
        Some(&asset_id),
    )
    .await;

    let channel_23 = open_channel(
        node2_addr,
        &node3_pubkey,
        Some(NODE3_PEER_PORT),
        Some(100000),
        Some(50000000),
        Some(100),
        Some(&asset_id),
    )
    .await;

    // Open a vanilla channel from node2 to node1
    let channel_21_vanilla = open_channel(
        node2_addr,
        &node1_pubkey,
        Some(NODE1_PEER_PORT),
        Some(100000),
        Some(50000000),
        None,
        None,
    )
    .await;

    // Wait for balances to be updated
    // wait_for_ln_balance(node2_addr, &asset_id, 200).await;
    // wait_for_ln_balance(node3_addr, &asset_id, 100).await;

    println!("\nsetup concurrent buy swaps from node1 and node3 to node2");
    let maker_addr = node2_addr;
    let qty_from = 2500000; // BTC amount
    let qty_to = 10; // Asset amount

    // Setup swap for node1
    let maker_init_response_1 =
        maker_init(maker_addr, qty_from, None, qty_to, Some(&asset_id), 3600).await;
    let swapstring_1 = maker_init_response_1.swapstring.clone();
    let payment_secret_1 = maker_init_response_1.payment_secret.clone();
    let payment_hash_1 = maker_init_response_1.payment_hash.clone();

    // Setup swap for node3
    let maker_init_response_3 =
        maker_init(maker_addr, qty_from, None, qty_to, Some(&asset_id), 3600).await;
    let swapstring_3 = maker_init_response_3.swapstring.clone();
    let payment_secret_3 = maker_init_response_3.payment_secret.clone();
    let payment_hash_3 = maker_init_response_3.payment_hash.clone();

    // Execute both swaps concurrently
    let (_, _) = join!(
        async {
            taker(node1_addr, swapstring_1).await;
            maker_execute(
                maker_addr,
                maker_init_response_1.swapstring,
                payment_secret_1,
                node1_pubkey.clone(),
            )
            .await;
            wait_for_swap_status(node1_addr, &payment_hash_1, SwapStatus::Succeeded).await;
        },
        async {
            taker(node3_addr, swapstring_3).await;
            maker_execute(
                maker_addr,
                maker_init_response_3.swapstring,
                payment_secret_3,
                node3_pubkey.clone(),
            )
            .await;
            wait_for_swap_status(node3_addr, &payment_hash_3, SwapStatus::Succeeded).await;
        }
    );

    wait_for_ln_balance(maker_addr, &asset_id, 180).await;
    wait_for_ln_balance(node1_addr, &asset_id, 10).await;
    wait_for_ln_balance(node3_addr, &asset_id, 10).await;

    println!("\nsetup sell swap from node2 to node1");
    let qty_from = 5; // Asset amount
    let qty_to = 1250000; // BTC amount
    let taker_addr = node1_addr;
    let maker_init_response =
        maker_init(maker_addr, qty_from, Some(&asset_id), qty_to, None, 3600).await;
    taker(taker_addr, maker_init_response.swapstring.clone()).await;

    println!("\nexecute sell swap");
    maker_execute(
        maker_addr,
        maker_init_response.swapstring,
        maker_init_response.payment_secret,
        node1_pubkey.clone(),
    )
    .await;

    wait_for_swap_status(taker_addr, &maker_init_response.payment_hash, SwapStatus::Succeeded).await;

    wait_for_ln_balance(taker_addr, &asset_id, 5).await;
    wait_for_ln_balance(maker_addr, &asset_id, 185).await;

    println!("\ncheck final balances");
    let balance_1 = asset_balance(node1_addr, &asset_id).await;
    let balance_2 = asset_balance(node2_addr, &asset_id).await;
    let balance_3 = asset_balance(node3_addr, &asset_id).await;
    assert_eq!(balance_1.offchain_outbound, 5);
    assert_eq!(balance_1.offchain_inbound, 95);
    assert_eq!(balance_2.offchain_outbound, 185);
    assert_eq!(balance_2.offchain_inbound, 15);
    assert_eq!(balance_3.offchain_outbound, 10);
    assert_eq!(balance_3.offchain_inbound, 90);

    println!("\nclose channels");
    close_channel(node2_addr, &channel_21.channel_id, &node1_pubkey, false).await;
    close_channel(node2_addr, &channel_23.channel_id, &node3_pubkey, false).await;
    close_channel(node2_addr, &channel_21_vanilla.channel_id, &node1_pubkey, false).await;

    wait_for_balance(node1_addr, &asset_id, 5).await;
    wait_for_balance(node3_addr, &asset_id, 10).await;
} 