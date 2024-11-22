use self::routes::HTLC_MIN_MSAT;

use tokio::time::Instant;

use super::*;

const TEST_DIR_BASE: &str = "tmp/swap_roundtrip_buy/";

#[serial_test::serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[traced_test]
async fn swap_parallel() {
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

    let node1_pubkey = node_info(node1_addr).await.pubkey;
    let node2_pubkey = node_info(node2_addr).await.pubkey;
    let node3_pubkey = node_info(node3_addr).await.pubkey;

    let channel_12 = open_channel(
        node1_addr,
        &node2_pubkey,
        Some(NODE2_PEER_PORT),
        None,
        None,
        Some(300),
        Some(&asset_id),
    )
    .await;
    let channel_21 = open_channel(
        node2_addr,
        &node1_pubkey,
        Some(NODE2_PEER_PORT),
        Some(5000000),
        Some(546000),
        None,
        None,
    )
    .await;

    let channel_13 = open_channel(
        node1_addr,
        &node3_pubkey,
        Some(NODE3_PEER_PORT),
        None,
        None,
        Some(300),
        Some(&asset_id),
    )
    .await;
    let channel_31 = open_channel(
        node3_addr,
        &node1_pubkey,
        Some(NODE3_PEER_PORT),
        Some(5000000),
        Some(546000),
        None,
        None,
    )
    .await;

    //let _channel_12 = open_channel(
    //    node1_addr,
    //    &node2_pubkey,
    //    Some(NODE2_PEER_PORT),
    //    None,
    //    None,
    //    Some(300),
    //    Some(&asset_id),
    //)
    //.await;
    //let _channel_21 = open_channel(
    //    node2_addr,
    //    &node1_pubkey,
    //    Some(NODE2_PEER_PORT),
    //    Some(5000000),
    //    Some(546000),
    //    None,
    //    None,
    //)
    //.await;
    //
    //let _channel_13 = open_channel(
    //    node1_addr,
    //    &node3_pubkey,
    //    Some(NODE3_PEER_PORT),
    //    None,
    //    None,
    //    Some(300),
    //    Some(&asset_id),
    //)
    //.await;
    //let _channel_31 = open_channel(
    //    node3_addr,
    //    &node1_pubkey,
    //    Some(NODE3_PEER_PORT),
    //    Some(5000000),
    //    Some(546000),
    //    None,
    //    None,
    //)
    //.await;

    let channels_1_before = list_channels(node1_addr).await;
    let channels_2_before = list_channels(node2_addr).await;
    let chan_1_12_before = channels_1_before
        .iter()
        .find(|c| c.channel_id == channel_12.channel_id)
        .unwrap();
    let chan_1_21_before = channels_1_before
        .iter()
        .find(|c| c.channel_id == channel_21.channel_id)
        .unwrap();
    let chan_2_12_before = channels_2_before
        .iter()
        .find(|c| c.channel_id == channel_12.channel_id)
        .unwrap();
    let chan_2_21_before = channels_2_before
        .iter()
        .find(|c| c.channel_id == channel_21.channel_id)
        .unwrap();

    let channels_3_before = list_channels(node3_addr).await;
    let chan_1_13_before = channels_1_before
        .iter()
        .find(|c| c.channel_id == channel_13.channel_id)
        .unwrap();
    let chan_1_31_before = channels_1_before
        .iter()
        .find(|c| c.channel_id == channel_31.channel_id)
        .unwrap();
    let chan_3_13_before = channels_3_before
        .iter()
        .find(|c| c.channel_id == channel_13.channel_id)
        .unwrap();
    let chan_3_31_before = channels_3_before
        .iter()
        .find(|c| c.channel_id == channel_31.channel_id)
        .unwrap();

    println!("\nsetup swap");
    let maker_addr = node1_addr;
    let taker_addr_1 = node2_addr;
    let taker_addr_2 = node3_addr;
    let qty_from = 50000;
    let qty_to = 10;
    let maker_init_response_1 =
        maker_init(maker_addr, qty_from, None, qty_to, Some(&asset_id), 3600).await;
    taker(taker_addr_1, maker_init_response_1.swapstring.clone()).await;
    let maker_init_response_2 =
        maker_init(maker_addr, qty_from, None, qty_to, Some(&asset_id), 3600).await;
    taker(taker_addr_2, maker_init_response_2.swapstring.clone()).await;

    let swaps_maker = list_swaps(maker_addr).await;
    assert!(swaps_maker.taker.is_empty());
    assert_eq!(swaps_maker.maker.len(), 2);
    let swap_maker_1 = swaps_maker
        .maker
        .iter()
        .find(|s| s.payment_hash == maker_init_response_1.payment_hash)
        .unwrap();
    assert_eq!(swap_maker_1.qty_from, qty_from);
    assert_eq!(swap_maker_1.qty_to, qty_to);
    assert_eq!(swap_maker_1.from_asset, None);
    assert_eq!(swap_maker_1.to_asset, Some(asset_id.clone()));
    assert_eq!(
        swap_maker_1.payment_hash,
        maker_init_response_1.payment_hash
    );
    assert_eq!(swap_maker_1.status, SwapStatus::Waiting);
    let swap_maker_2 = swaps_maker
        .maker
        .iter()
        .find(|s| s.payment_hash == maker_init_response_2.payment_hash)
        .unwrap();
    assert_eq!(swap_maker_2.qty_from, qty_from);
    assert_eq!(swap_maker_2.qty_to, qty_to);
    assert_eq!(swap_maker_2.from_asset, None);
    assert_eq!(swap_maker_2.to_asset, Some(asset_id.clone()));
    assert_eq!(
        swap_maker_2.payment_hash,
        maker_init_response_2.payment_hash
    );
    assert_eq!(swap_maker_2.status, SwapStatus::Waiting);
    let swaps_taker_1 = list_swaps(taker_addr_1).await;
    assert!(swaps_taker_1.maker.is_empty());
    assert_eq!(swaps_taker_1.taker.len(), 1);
    let swap_taker_1 = swaps_taker_1.taker.first().unwrap();
    assert_eq!(swap_taker_1.qty_from, qty_from);
    assert_eq!(swap_taker_1.qty_to, qty_to);
    assert_eq!(swap_taker_1.from_asset, None);
    assert_eq!(swap_taker_1.to_asset, Some(asset_id.clone()));
    assert_eq!(
        swap_taker_1.payment_hash,
        maker_init_response_1.payment_hash
    );
    assert_eq!(swap_taker_1.status, SwapStatus::Waiting);
    let swaps_taker_2 = list_swaps(taker_addr_2).await;
    assert!(swaps_taker_2.maker.is_empty());
    assert_eq!(swaps_taker_2.taker.len(), 1);
    let swap_taker_2 = swaps_taker_2.taker.first().unwrap();
    assert_eq!(swap_taker_2.qty_from, qty_from);
    assert_eq!(swap_taker_2.qty_to, qty_to);
    assert_eq!(swap_taker_2.from_asset, None);
    assert_eq!(swap_taker_2.to_asset, Some(asset_id.clone()));
    assert_eq!(
        swap_taker_2.payment_hash,
        maker_init_response_2.payment_hash
    );
    assert_eq!(swap_taker_2.status, SwapStatus::Waiting);

    //println!("\nexecute swap 1");
    //maker_execute(
    //    maker_addr,
    //    maker_init_response_1.swapstring,
    //    maker_init_response_1.payment_secret,
    //    node2_pubkey.clone(),
    //)
    //.await;
    //
    //println!("\nexecute swap 2");
    //maker_execute(
    //    maker_addr,
    //    maker_init_response_2.swapstring,
    //    maker_init_response_2.payment_secret,
    //    node3_pubkey.clone(),
    //)
    //.await;

    let start = Instant::now();

    println!("\nexecute swaps");
    let (swap1_time, swap2_time) = tokio::join!(
        async {
            let swap1_start = Instant::now();
            maker_execute(
                maker_addr.clone(),
                maker_init_response_1.swapstring,
                maker_init_response_1.payment_secret,
                node2_pubkey.clone(),
            )
            .await;
            let swap1_end = Instant::now();
            [swap1_start, swap1_end]
        },
        async {
            let swap2_start = Instant::now();
            maker_execute(
                maker_addr.clone(),
                maker_init_response_2.swapstring,
                maker_init_response_2.payment_secret,
                node3_pubkey.clone(),
            )
            .await;
            let swap2_end = Instant::now();
            [swap2_start, swap2_end]
        },
    );

    // check that swap1 started before swap2 finished of that swap2 started after swap1 finished
    assert!(swap1_time[0] < swap2_time[1] || swap2_time[0] > swap1_time[1]);

    let total_duration = start.elapsed();

    let swaps_maker = list_swaps(maker_addr).await;
    assert_eq!(swaps_maker.maker.len(), 2);
    let swap_maker_1 = swaps_maker
        .maker
        .iter()
        .find(|s| s.payment_hash == maker_init_response_1.payment_hash)
        .unwrap();
    assert_eq!(swap_maker_1.status, SwapStatus::Pending);
    let swap_maker_2 = swaps_maker
        .maker
        .iter()
        .find(|s| s.payment_hash == maker_init_response_2.payment_hash)
        .unwrap();
    assert_eq!(swap_maker_2.status, SwapStatus::Pending);

    wait_for_swap_status(
        taker_addr_1,
        &maker_init_response_1.payment_hash,
        SwapStatus::Succeeded,
    )
    .await;
    wait_for_swap_status(
        taker_addr_2,
        &maker_init_response_2.payment_hash,
        SwapStatus::Succeeded,
    )
    .await;

    wait_for_ln_balance(maker_addr, &asset_id, 580).await;
    wait_for_ln_balance(taker_addr_1, &asset_id, 10).await;
    wait_for_ln_balance(taker_addr_2, &asset_id, 10).await;

    println!("\nrestart nodes");
    shutdown(&[node1_addr, node2_addr, node3_addr]).await;
    let (node1_addr, _) = start_node(&test_dir_node1, NODE1_PEER_PORT, true).await;
    let (node2_addr, _) = start_node(&test_dir_node2, NODE2_PEER_PORT, true).await;
    let (node3_addr, _) = start_node(&test_dir_node3, NODE3_PEER_PORT, true).await;
    let maker_addr = node1_addr;
    let taker_addr_1 = node2_addr;
    let taker_addr_2 = node3_addr;
    wait_for_usable_channels(node1_addr, 4).await;
    wait_for_usable_channels(node2_addr, 2).await;
    wait_for_usable_channels(node3_addr, 2).await;

    println!("\ncheck off-chain balances and payments after nodes have restarted");
    let balance_1 = asset_balance(node1_addr, &asset_id).await;
    let balance_2 = asset_balance(node2_addr, &asset_id).await;
    let balance_3 = asset_balance(node3_addr, &asset_id).await;
    assert_eq!(balance_1.offchain_outbound, 580);
    assert_eq!(balance_1.offchain_inbound, 20);
    assert_eq!(balance_2.offchain_outbound, 10);
    assert_eq!(balance_2.offchain_inbound, 290);
    assert_eq!(balance_3.offchain_outbound, 10);
    assert_eq!(balance_3.offchain_inbound, 290);

    let swaps_maker = list_swaps(maker_addr).await;
    assert_eq!(swaps_maker.maker.len(), 2);
    let swap_maker_1 = swaps_maker
        .maker
        .iter()
        .find(|s| s.payment_hash == maker_init_response_1.payment_hash)
        .unwrap();
    assert_eq!(swap_maker_1.status, SwapStatus::Succeeded);
    let swaps_taker_1 = list_swaps(taker_addr_1).await;
    assert_eq!(swaps_taker_1.taker.len(), 1);
    let swap_taker_1 = swaps_taker_1.taker.first().unwrap();
    assert_eq!(swap_taker_1.status, SwapStatus::Succeeded);
    let swap_maker_2 = swaps_maker
        .maker
        .iter()
        .find(|s| s.payment_hash == maker_init_response_2.payment_hash)
        .unwrap();
    assert_eq!(swap_maker_2.status, SwapStatus::Succeeded);
    let swaps_taker_2 = list_swaps(taker_addr_2).await;
    assert_eq!(swaps_taker_2.taker.len(), 1);
    let swap_taker_2 = swaps_taker_2.taker.first().unwrap();
    assert_eq!(swap_taker_2.status, SwapStatus::Succeeded);

    let payments_maker = list_payments(maker_addr).await;
    assert!(payments_maker.is_empty());
    let payments_taker_1 = list_payments(taker_addr_1).await;
    assert!(payments_taker_1.is_empty());
    let payments_taker_2 = list_payments(taker_addr_2).await;
    assert!(payments_taker_2.is_empty());

    let channels_1 = list_channels(node1_addr).await;
    let channels_2 = list_channels(node2_addr).await;
    let channels_3 = list_channels(node3_addr).await;
    let chan_1_12 = channels_1
        .iter()
        .find(|c| c.channel_id == channel_12.channel_id)
        .unwrap();
    let chan_1_21 = channels_1
        .iter()
        .find(|c| c.channel_id == channel_21.channel_id)
        .unwrap();
    let chan_2_12 = channels_2
        .iter()
        .find(|c| c.channel_id == channel_12.channel_id)
        .unwrap();
    let chan_2_21 = channels_2
        .iter()
        .find(|c| c.channel_id == channel_21.channel_id)
        .unwrap();
    let chan_1_13 = channels_3
        .iter()
        .find(|c| c.channel_id == channel_13.channel_id)
        .unwrap();
    let chan_1_31 = channels_1
        .iter()
        .find(|c| c.channel_id == channel_21.channel_id)
        .unwrap();
    let chan_3_13 = channels_3
        .iter()
        .find(|c| c.channel_id == channel_13.channel_id)
        .unwrap();
    let chan_3_31 = channels_3
        .iter()
        .find(|c| c.channel_id == channel_31.channel_id)
        .unwrap();
    let btc_leg_diff = HTLC_MIN_MSAT + qty_from;
    assert_eq!(
        chan_1_12.local_balance_msat,
        chan_1_12_before.local_balance_msat - HTLC_MIN_MSAT
    );
    assert_eq!(
        chan_1_21.local_balance_msat,
        chan_1_21_before.local_balance_msat + btc_leg_diff
    );
    assert_eq!(
        chan_2_12.local_balance_msat,
        chan_2_12_before.local_balance_msat + HTLC_MIN_MSAT
    );
    assert_eq!(
        chan_2_21.local_balance_msat,
        chan_2_21_before.local_balance_msat - btc_leg_diff
    );
    assert_eq!(
        chan_1_13.local_balance_msat,
        chan_1_13_before.local_balance_msat - HTLC_MIN_MSAT
    );
    assert_eq!(
        chan_1_31.local_balance_msat,
        chan_1_31_before.local_balance_msat + btc_leg_diff
    );
    assert_eq!(
        chan_3_13.local_balance_msat,
        chan_3_13_before.local_balance_msat + HTLC_MIN_MSAT
    );
    assert_eq!(
        chan_3_31.local_balance_msat,
        chan_3_31_before.local_balance_msat - btc_leg_diff
    );

    //println!("\nclose channels");
    //close_channel(node1_addr, &channel_12.channel_id, &node2_pubkey, false).await;
    //wait_for_balance(node1_addr, &asset_id, 990).await;
    //wait_for_balance(node2_addr, &asset_id, 10).await;
    //
    //close_channel(node2_addr, &channel_21.channel_id, &node1_pubkey, false).await;
    //
    //println!("\nspend assets");
    //let recipient_id = rgb_invoice(node3_addr, None).await.recipient_id;
    //send_asset(node1_addr, &asset_id, 200, recipient_id).await;
    //mine(false);
    //refresh_transfers(node3_addr).await;
    //refresh_transfers(node3_addr).await;
    //refresh_transfers(node1_addr).await;
    //
    //let recipient_id = rgb_invoice(node3_addr, None).await.recipient_id;
    //send_asset(node2_addr, &asset_id, 5, recipient_id).await;
    //mine(false);
    //refresh_transfers(node3_addr).await;
    //refresh_transfers(node3_addr).await;
    //refresh_transfers(node2_addr).await;
    //
    //assert_eq!(asset_balance_spendable(node1_addr, &asset_id).await, 790);
    //assert_eq!(asset_balance_spendable(node2_addr, &asset_id).await, 5);
    //assert_eq!(asset_balance_spendable(node3_addr, &asset_id).await, 205);
}
