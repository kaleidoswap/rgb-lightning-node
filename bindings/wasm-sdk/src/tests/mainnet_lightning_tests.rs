use super::*;
use crate::{RlnWasmSdk, RlnWasmSdkNodeHandle, RlnWasmWallet};
use wasm_bindgen_test::wasm_bindgen_test;

fn assert_mainnet_rejection<T>(result: Result<T, JsValue>) {
    let Err(error) = result else {
        panic!("mainnet Lightning call unexpectedly succeeded");
    };
    assert_eq!(
        error.as_string().as_deref(),
        Some("LightningUnsupportedOnMainnet: RLN on mainnet currently supports only on-chain methods. Lightning APIs are not supported.")
    );
}

fn configured_node(network: &str) -> RlnWasmNode {
    RlnWasmNode::new_with_node_runtime_id(
        "ws://mainnet-guard.invalid".to_string(),
        format!("mainnet-guard-{network}"),
        network.to_string(),
    )
    .expect("configured node")
}

#[wasm_bindgen_test(async)]
async fn mainnet_lightning_operations_reject_before_runtime_or_state_changes() {
    crate::test_utils::reset_wasm_runtime_state_for_tests();
    let node = configured_node("mainnet");
    let runtime_before = serde_json::to_value(node.ldk_runtime.status()).unwrap();
    let core_before = serde_json::to_value(node.runtime_core.status()).unwrap();

    assert_mainnet_rejection(node.connect_peer(String::new(), String::new()).await);
    assert_mainnet_rejection(node.disconnect_peer(String::new()).await);
    assert_mainnet_rejection(node.reconnect_persisted_peers_value().await);
    assert_mainnet_rejection(node.reconnect_manager_start_value());
    assert_mainnet_rejection(node.auto_drive_start_value(0));
    assert_mainnet_rejection(node.chain_sync_tick_value().await);
    assert_mainnet_rejection(node.list_peers_value());
    assert_mainnet_rejection(node.list_channels_value());
    assert_mainnet_rejection(node.open_channel_value(String::new(), 0, false, None, None));
    assert_mainnet_rejection(node.close_channel(String::new()));
    assert_mainnet_rejection(node.get_channel_id(String::new()));
    assert_mainnet_rejection(node.list_pending_funding_requests_value());
    assert_mainnet_rejection(node.submit_funding_transaction_value(JsValue::NULL));
    assert_mainnet_rejection(node.close_all_peers().await);
    assert_mainnet_rejection(node.drain_native_runtime_queue_value());
    assert_mainnet_rejection(node.process_native_runtime_queue_value());
    assert_mainnet_rejection(node.fail_pending_payments_api());
    assert_mainnet_rejection(node.send_payment_value(String::new(), None, None, None));
    assert_mainnet_rejection(node.send_payment_live_value(String::new(), None, None, None));
    assert_mainnet_rejection(node.keysend_value(String::new(), 0, None, None));
    assert_mainnet_rejection(node.keysend_live_value(String::new(), 0, None, None));
    assert_mainnet_rejection(node.live_payment_value(String::new()));
    assert_mainnet_rejection(node.live_payments_value());
    assert_mainnet_rejection(node.list_payments_value());
    assert_mainnet_rejection(node.list_rgb_ln_transfers_value());
    assert_mainnet_rejection(node.get_payment_value(String::new()));
    assert_mainnet_rejection(node.update_payment_status(String::new(), String::new()));
    assert_mainnet_rejection(node.decode_ln_invoice_value(String::new()));
    assert_mainnet_rejection(node.create_ln_invoice_value(None, 0, None, None));
    assert_mainnet_rejection(node.create_ln_invoice_live_value(None, 0, None, None));
    assert_mainnet_rejection(node.create_hodl_ln_invoice_value(None, 0, None, None, String::new()));
    assert_mainnet_rejection(node.cancel_hodl_invoice_value(String::new()));
    assert_mainnet_rejection(node.claim_hodl_invoice_value(String::new(), String::new()));
    assert_mainnet_rejection(node.invoice_status_value(String::new()));
    assert_mainnet_rejection(node.update_payment_status_by_invoice(String::new(), String::new()));
    assert_mainnet_rejection(node.ingest_read_event_payload_hex(String::new()));
    assert_mainnet_rejection(node.ingest_runtime_transport_event_payload_hex_value(String::new()));
    assert_mainnet_rejection(node.drive_rgb_funding_work().await);
    assert_mainnet_rejection(node.process_pending_rgb_transactions().await);
    assert_mainnet_rejection(node.apay_new_value(String::new()).await);
    assert_mainnet_rejection(
        node.apay_new_with_address_value(String::new(), String::new(), String::new())
            .await,
    );

    assert_eq!(
        serde_json::to_value(node.ldk_runtime.status()).unwrap(),
        runtime_before
    );
    assert_eq!(
        serde_json::to_value(node.runtime_core.status()).unwrap(),
        core_before
    );
    assert!(node.channels.borrow().is_empty());
    assert!(node.payments.borrow().is_empty());
    assert!(node.runtime_events.borrow().is_empty());
    assert!(!*node.reconnect_manager_running.borrow());
    assert!(!*node.auto_drive_running.borrow());
}

#[wasm_bindgen_test]
fn mainnet_lightning_error_propagates_through_facade_and_json_handles() {
    crate::test_utils::reset_wasm_runtime_state_for_tests();
    let node = configured_node("MAINNET");
    let sdk = RlnWasmSdk::new();
    assert_mainnet_rejection(sdk.list_channels_json(&node));
    assert_mainnet_rejection(sdk.decode_ln_invoice_json(&node, String::new()));
    let handle = RlnWasmSdkNodeHandle { inner: node };
    assert_mainnet_rejection(handle.list_payments_json());
    assert_mainnet_rejection(handle.get_channel_id(String::new()));
}

#[wasm_bindgen_test]
fn non_mainnet_lightning_queries_and_validation_are_unchanged() {
    for network in ["testnet", "testnet4", "signet", "regtest"] {
        crate::test_utils::reset_wasm_runtime_state_for_tests();
        let node = configured_node(network);
        node.check_lightning_supported().expect("supported network");
        assert_eq!(node.list_channels_json().expect("list channels"), "[]");
        assert_eq!(
            node.decode_ln_invoice_value(String::new())
                .unwrap_err()
                .as_string()
                .as_deref(),
            Some(sdk_contracts::ERR_INVOICE_EMPTY)
        );
    }
}

#[wasm_bindgen_test(async)]
async fn mainnet_wallet_remains_available_and_adopted_network_restricts_lightning() {
    crate::test_utils::reset_wasm_runtime_state_for_tests();
    let mut wallet_data: serde_json::Value =
        serde_json::from_str(&crate::test_utils::test_wallet_data_json()).unwrap();
    let keys = rgb_lib_wasm::restore_keys(
        rgb_lib_wasm::BitcoinNetwork::Mainnet,
        wallet_data["mnemonic"].as_str().unwrap().to_string(),
    )
    .expect("mainnet keys");
    wallet_data["bitcoin_network"] = serde_json::json!("Mainnet");
    wallet_data["supported_schemas"] = serde_json::json!(["Nia"]);
    wallet_data["account_xpub_vanilla"] = serde_json::json!(keys.account_xpub_vanilla);
    wallet_data["account_xpub_colored"] = serde_json::json!(keys.account_xpub_colored);
    let wallet = RlnWasmWallet::create(&wallet_data.to_string())
        .await
        .expect("mainnet wallet");
    assert!(wallet
        .get_address()
        .expect("on-chain address")
        .starts_with("bc1"));
    let node = RlnWasmNode::new("ws://mainnet-wallet-guard.invalid".to_string()).unwrap();
    // Model an already-running reconnect manager without starting timers or opening sockets.
    *node.reconnect_manager_running.borrow_mut() = true;
    node.attach_wallet(&wallet).expect("adopt mainnet wallet");
    assert_eq!(node.network.borrow().as_str(), "mainnet");
    let backoff_before = *node.reconnect_manager_backoff_ms.borrow();
    assert_mainnet_rejection(node.reconnect_manager_on_resume());
    assert_eq!(*node.reconnect_manager_backoff_ms.borrow(), backoff_before);
    assert!(node.peers.borrow().is_empty());
    *node.reconnect_manager_running.borrow_mut() = false;
    {
        let _busy_wallet = wallet.inner.borrow_mut();
        assert_mainnet_rejection(node.list_channels_json());
    }
    // Hooks were installed before attachWallet adopted mainnet; their guard must observe it.
    let bridge = RlnWasmRustPeerManagerBridge::new(None).unwrap();
    assert_mainnet_rejection(
        bridge
            .connect_session(String::new(), String::new(), String::new())
            .await,
    );
    assert_mainnet_rejection(
        wallet
            .build_lightning_funding_tx_value(JsValue::NULL, "00".to_string(), 1, 1)
            .await,
    );
    // The on-chain decoder retains its existing validation error, rather than a Lightning error.
    assert_eq!(
        node.decode_rgb_invoice_value(String::new())
            .unwrap_err()
            .as_string()
            .as_deref(),
        Some(sdk_contracts::ERR_INVOICE_EMPTY)
    );
    assert!(wallet
        .get_address()
        .expect("on-chain address after rejection")
        .starts_with("bc1"));
}

#[wasm_bindgen_test]
fn mainnet_guard_uses_configured_network_despite_restored_chain_sync_status() {
    crate::test_utils::reset_wasm_runtime_state_for_tests();
    let proxy_url = "ws://mainnet-stale-network.invalid".to_string();
    let runtime_id = "mainnet-stale-network".to_string();
    let previous = RlnWasmNode::new_with_node_runtime_id(
        proxy_url.clone(),
        runtime_id.clone(),
        "regtest".to_string(),
    )
    .unwrap();
    previous.chain_sync.set_network("regtest").unwrap();
    drop(previous);
    let node = RlnWasmNode::new_with_node_runtime_id(proxy_url, runtime_id, "mainnet".to_string())
        .unwrap();
    assert_eq!(node.chain_sync.status().network, "regtest");
    node.chain_sync_start_value("http://127.0.0.1:1".to_string(), None)
        .unwrap();
    node.chain_sync_stop_value().unwrap();
    assert_eq!(node.network.borrow().as_str(), "regtest");
    assert_mainnet_rejection(node.decode_ln_invoice_value(String::new()));
    assert_mainnet_rejection(node.list_channels_value());
}

#[wasm_bindgen_test(async)]
async fn mainnet_peer_bridge_rejects_before_opening_a_socket() {
    crate::test_utils::reset_wasm_runtime_state_for_tests();
    let _node = configured_node("mainnet");
    let bridge = RlnWasmRustPeerManagerBridge::new(None).unwrap();
    // An invalid WebSocket URL would fail during socket creation if the guard ran too late.
    assert_mainnet_rejection(
        bridge
            .connect_session(String::new(), String::new(), String::new())
            .await,
    );
    assert_mainnet_rejection(
        bridge
            .connect_session_with_options(
                String::new(),
                String::new(),
                String::new(),
                JsValue::NULL,
            )
            .await,
    );
    // Clearing the hooks removes the standalone bridge's configured-node registration.
    clear_rln_ldk_peer_manager_hooks();
    assert!(!has_peer_manager_hooks());
}

#[wasm_bindgen_test(async)]
async fn lightning_guard_does_not_borrow_busy_onchain_wallet() {
    crate::test_utils::reset_wasm_runtime_state_for_tests();
    let wallet = RlnWasmWallet::create(&crate::test_utils::test_wallet_data_json())
        .await
        .expect("regtest wallet");
    let node = RlnWasmNode::new("ws://busy-wallet-guard.invalid".to_string()).unwrap();
    node.attach_wallet(&wallet).unwrap();
    let _busy_wallet = wallet.inner.borrow_mut();
    node.check_lightning_supported()
        .expect("non-mainnet remains supported");
    assert_eq!(
        node.decode_ln_invoice_value(String::new())
            .unwrap_err()
            .as_string()
            .as_deref(),
        Some(sdk_contracts::ERR_INVOICE_EMPTY)
    );
}

#[wasm_bindgen_test]
fn mainnet_reconnect_resume_rejects_through_node_and_wrappers() {
    crate::test_utils::reset_wasm_runtime_state_for_tests();
    let node = configured_node("mainnet");
    let sdk = RlnWasmSdk::new();
    assert_mainnet_rejection(node.reconnect_manager_on_resume());
    assert_mainnet_rejection(sdk.reconnect_manager_on_resume(&node));
    let handle = RlnWasmSdkNodeHandle { inner: node };
    assert_mainnet_rejection(handle.reconnect_manager_on_resume());
    assert!(!*handle.inner.reconnect_manager_running.borrow());

    for network in ["testnet", "testnet4", "signet", "regtest"] {
        crate::test_utils::reset_wasm_runtime_state_for_tests();
        let node = configured_node(network);
        node.reconnect_manager_on_resume()
            .expect("inactive reconnect remains a no-op on supported networks");
        sdk.reconnect_manager_on_resume(&node)
            .expect("facade preserves supported-network behavior");
        let handle = RlnWasmSdkNodeHandle { inner: node };
        handle
            .reconnect_manager_on_resume()
            .expect("handle preserves supported-network behavior");
        assert!(!*handle.inner.reconnect_manager_running.borrow());
    }
}

#[wasm_bindgen_test(async)]
async fn node_peer_bridges_keep_their_network_in_both_creation_orders() {
    for mainnet_first in [false, true] {
        crate::test_utils::reset_wasm_runtime_state_for_tests();
        let (mainnet, regtest) = if mainnet_first {
            let mainnet = configured_node("mainnet");
            (mainnet, configured_node("regtest"))
        } else {
            let regtest = configured_node("regtest");
            (configured_node("mainnet"), regtest)
        };
        assert_mainnet_rejection(
            mainnet
                .bridge
                .connect_session(String::new(), String::new(), String::new())
                .await,
        );
        assert_mainnet_rejection(
            mainnet
                .bridge
                .connect_session_with_options(
                    String::new(),
                    String::new(),
                    String::new(),
                    JsValue::NULL,
                )
                .await,
        );
        for result in [
            regtest
                .bridge
                .connect_session(String::new(), String::new(), String::new())
                .await,
            regtest
                .bridge
                .connect_session_with_options(
                    String::new(),
                    String::new(),
                    String::new(),
                    JsValue::NULL,
                )
                .await,
        ] {
            let Err(error) = result else {
                panic!("empty peer pubkey unexpectedly accepted");
            };
            assert_eq!(
                error.as_string().as_deref(),
                Some(sdk_contracts::ERR_PEER_PUBKEY_EMPTY),
                "another node's mainnet policy must not replace this node's validation"
            );
        }
    }
}
