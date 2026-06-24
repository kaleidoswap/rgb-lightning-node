use crate::helpers::*;
use serial_test::serial;
use std::{fs, time::Duration};

#[test]
#[serial]
fn apay_new_requires_a_live_channel() {
    ensure_regtest_available();

    let test_dir = test_dir("sdk_apay_peer_gate");
    if test_dir.exists() {
        fs::remove_dir_all(&test_dir).expect("remove previous lib_sdk test dir");
    }
    fs::create_dir_all(&test_dir).expect("create lib_sdk test dir");
    let node_a_dir = test_dir.join("node_a");
    let node_b_dir = test_dir.join("node_b");

    let node_a = make_node(&node_a_dir, NODE_A_DAEMON_PORT, NODE_A_PEER_PORT);
    let node_b = make_node(&node_b_dir, NODE_B_DAEMON_PORT, NODE_B_PEER_PORT);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        node_a
            .init("nodeApass".to_string(), None)
            .expect("node A init");
        node_b
            .init("nodeBpass".to_string(), None)
            .expect("node B init");

        node_a
            .unlock(unlock_request("nodeApass"))
            .expect("node A unlock");
        node_b
            .unlock(unlock_request("nodeBpass"))
            .expect("node B unlock");

        ensure_funded(&node_a, 200_000, "node A");
        ensure_funded(&node_b, 200_000, "node B");

        let node_b_pubkey = node_b.node_info().expect("node B node_info").pubkey;
        let peer_uri = format!("{node_b_pubkey}@127.0.0.1:{NODE_B_PEER_PORT}");

        node_a
            .connectpeer(peer_uri.clone())
            .expect("node A connectpeer");

        node_a
            .apay_new(node_b_pubkey.to_string())
            .expect_err("apay_new must fail before a live channel exists");

        let _open_channel = node_a
            .openchannel(SdkOpenChannelRequest {
                peer_pubkey_and_opt_addr: peer_uri,
                capacity_sat: OPEN_CHANNEL_CAPACITY_SAT,
                push_msat: OPEN_CHANNEL_PUSH_MSAT,
                public: true,
                with_anchors: true,
                fee_base_msat: None,
                fee_proportional_millionths: None,
                temporary_channel_id: None,
                asset_id: None,
                asset_amount: None,
                push_asset_amount: None,
                virtual_open_mode: None,
            })
            .expect("node A openchannel");

        let channel_id = wait_for_channel_open(
            &node_a,
            |channel| channel.peer_pubkey == node_b_pubkey && channel.asset_id.is_none(),
            Duration::from_secs(180),
        );
        wait_for_channel_ready(&node_b, channel_id, Duration::from_secs(180));

        let response = node_a
            .apay_new(node_b_pubkey.to_string())
            .expect("apay_new over live channel");
        assert_eq!(response.host_node_id, node_b_pubkey.to_string());
        assert!(!response.hashes.is_empty());
    }));

    node_a.shutdown();
    node_b.shutdown();

    if let Err(err) = result {
        std::panic::resume_unwind(err);
    }
}
