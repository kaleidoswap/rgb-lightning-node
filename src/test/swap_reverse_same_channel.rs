use super::*;

const TEST_DIR_BASE: &str = "tmp/swap_reverse_same_channel/";

#[serial_test::serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[traced_test]
async fn swap_reverse_same_channel() {
    initialize();

    let test_dir_node1 = format!("{TEST_DIR_BASE}node1");
    let test_dir_node2 = format!("{TEST_DIR_BASE}node2");
    let (node1_addr, _) = start_node(&test_dir_node1, NODE1_PEER_PORT, false).await;
    let (node2_addr, _) = start_node(&test_dir_node2, NODE2_PEER_PORT, false).await;

    fund_and_create_utxos(node1_addr, None).await;
    fund_and_create_utxos(node2_addr, None).await;

    let asset_id = issue_asset_nia(node1_addr).await.asset_id;

    let node1_pubkey = node_info(node1_addr).await.pubkey;
    let node2_pubkey = node_info(node2_addr).await.pubkey;

    let channel_12 = open_channel(
        node1_addr,
        &node2_pubkey,
        Some(NODE2_PEER_PORT),
        Some(100000),
        Some(50000000),
        Some(600),
        Some(&asset_id),
    )
    .await;

    let channels_1_before = list_channels(node1_addr).await;
    let channels_2_before = list_channels(node2_addr).await;
    let chan_1_12_before = channels_1_before
        .iter()
        .find(|c| c.channel_id == channel_12.channel_id)
        .unwrap();
    let chan_2_12_before = channels_2_before
        .iter()
        .find(|c| c.channel_id == channel_12.channel_id)
        .unwrap();

    println!("\nsetup swap");
    let maker_addr = node1_addr;
    let taker_addr = node2_addr;
    // qty_from must be >= HTLC_MIN_MSAT because the single RGB channel was opened
    // by node1 with our_htlc_minimum_msat = HTLC_MIN_MSAT, and the taker pays
    // the HODL BTC invoice through this same channel (node2→node1 direction).
    let qty_from = 5000000;
    let qty_to = 10;
    let maker_init_response =
        maker_init(maker_addr, qty_from, None, qty_to, Some(&asset_id), 3600, &node2_pubkey).await;
    taker(taker_addr, maker_init_response.swapstring.clone()).await;

    let swaps_maker = list_swaps(maker_addr).await;
    assert!(swaps_maker.taker.is_empty());
    assert_eq!(swaps_maker.maker.len(), 1);
    let swap_maker = swaps_maker.maker.first().unwrap();
    assert_eq!(swap_maker.qty_from, qty_from);
    assert_eq!(swap_maker.qty_to, qty_to);
    assert_eq!(swap_maker.from_asset, None);
    assert_eq!(swap_maker.to_asset, Some(asset_id.clone()));
    assert_eq!(swap_maker.payment_hash, maker_init_response.payment_hash);
    assert_eq!(swap_maker.status, SwapStatus::Waiting);

    let swaps_taker = list_swaps(taker_addr).await;
    assert!(swaps_taker.maker.is_empty());
    assert_eq!(swaps_taker.taker.len(), 1);
    let swap_taker = swaps_taker.taker.first().unwrap();
    assert_eq!(swap_taker.qty_from, qty_from);
    assert_eq!(swap_taker.qty_to, qty_to);
    assert_eq!(swap_taker.from_asset, None);
    assert_eq!(swap_taker.to_asset, Some(asset_id.clone()));
    assert_eq!(swap_taker.payment_hash, maker_init_response.payment_hash);
    assert_eq!(swap_taker.status, SwapStatus::Waiting);

    println!("\nexecute swap");
    taker_pay_invoice(taker_addr, &maker_init_response.bolt11_invoice).await;

    wait_for_swap_status(
        maker_addr,
        &maker_init_response.payment_hash,
        SwapStatus::Succeeded,
    )
    .await;
    wait_for_swap_status(
        taker_addr,
        &maker_init_response.payment_hash,
        SwapStatus::Succeeded,
    )
    .await;

    wait_for_ln_balance(maker_addr, &asset_id, 590).await;
    wait_for_ln_balance(taker_addr, &asset_id, 10).await;

    let swaps_maker = list_swaps(maker_addr).await;
    assert_eq!(swaps_maker.maker.len(), 1);
    let swap_maker = swaps_maker.maker.first().unwrap();
    assert_eq!(swap_maker.status, SwapStatus::Succeeded);
    let swaps_taker = list_swaps(taker_addr).await;
    assert_eq!(swaps_taker.taker.len(), 1);
    let swap_taker = swaps_taker.taker.first().unwrap();
    assert_eq!(swap_taker.status, SwapStatus::Succeeded);

    let channels_1 = list_channels(node1_addr).await;
    let channels_2 = list_channels(node2_addr).await;
    let chan_1_12 = channels_1
        .iter()
        .find(|c| c.channel_id == channel_12.channel_id)
        .unwrap();
    let chan_2_12 = channels_2
        .iter()
        .find(|c| c.channel_id == channel_12.channel_id)
        .unwrap();
    // TODO: verify expected channel balance changes with new swap mechanism.
    // Both HODL and forward payments use the same single channel.
    // HODL: taker(node2)→maker(node1) +qty_from/1000 sats; Forward: maker(node1)→taker(node2) -HTLC_MIN_MSAT/1000 sats.
    use self::routes::HTLC_MIN_MSAT;
    let hodl_sat = qty_from / 1000;
    let fwd_sat = HTLC_MIN_MSAT / 1000;
    assert_eq!(
        chan_1_12.local_balance_sat,
        chan_1_12_before.local_balance_sat + hodl_sat - fwd_sat
    );
    assert_eq!(
        chan_2_12.local_balance_sat,
        chan_2_12_before.local_balance_sat - hodl_sat + fwd_sat
    );

    println!("\nsetup reverse swap");
    let maker_addr = node2_addr;
    let taker_addr = node1_addr;
    // qty_to must be >= HTLC_MIN_MSAT because the forward BTC keysend from the
    // new maker (node2) to the new taker (node1) goes through the same single
    // channel, and node1's our_htlc_minimum_msat = HTLC_MIN_MSAT applies to
    // HTLCs arriving at node1 (channel was opened by node1).
    let qty_from = 10;
    let qty_to = 5000000;
    let maker_init_response =
        maker_init(maker_addr, qty_from, Some(&asset_id), qty_to, None, 3600, &node1_pubkey).await;
    taker(taker_addr, maker_init_response.swapstring.clone()).await;

    let swaps_maker = list_swaps(maker_addr).await;
    assert_eq!(swaps_maker.maker.len(), 1);
    let swap_maker = swaps_maker.maker.first().unwrap();
    assert_eq!(swap_maker.qty_from, qty_from);
    assert_eq!(swap_maker.qty_to, qty_to);
    assert_eq!(swap_maker.from_asset, Some(asset_id.clone()));
    assert_eq!(swap_maker.to_asset, None);
    assert_eq!(swap_maker.payment_hash, maker_init_response.payment_hash);
    assert_eq!(swap_maker.status, SwapStatus::Waiting);

    let swaps_taker = list_swaps(taker_addr).await;
    assert_eq!(swaps_taker.taker.len(), 1);
    let swap_taker = swaps_taker.taker.first().unwrap();
    assert_eq!(swap_taker.qty_from, qty_from);
    assert_eq!(swap_taker.qty_to, qty_to);
    assert_eq!(swap_taker.from_asset, Some(asset_id.clone()));
    assert_eq!(swap_taker.to_asset, None);
    assert_eq!(swap_taker.payment_hash, maker_init_response.payment_hash);
    assert_eq!(swap_taker.status, SwapStatus::Waiting);

    println!("\nexecute reverse swap");
    taker_pay_invoice(taker_addr, &maker_init_response.bolt11_invoice).await;

    wait_for_swap_status(
        maker_addr,
        &maker_init_response.payment_hash,
        SwapStatus::Succeeded,
    )
    .await;
    wait_for_swap_status(
        taker_addr,
        &maker_init_response.payment_hash,
        SwapStatus::Succeeded,
    )
    .await;

    wait_for_ln_balance(maker_addr, &asset_id, 20).await;
    wait_for_ln_balance(taker_addr, &asset_id, 580).await;

    let swaps_maker = list_swaps(maker_addr).await;
    assert_eq!(swaps_maker.maker.len(), 1);
    let swap_maker = swaps_maker.maker.first().unwrap();
    assert_eq!(swap_maker.status, SwapStatus::Succeeded);
    let swaps_taker = list_swaps(taker_addr).await;
    assert_eq!(swaps_taker.taker.len(), 1);
    let swap_taker = swaps_taker.taker.first().unwrap();
    assert_eq!(swap_taker.status, SwapStatus::Succeeded);
}
