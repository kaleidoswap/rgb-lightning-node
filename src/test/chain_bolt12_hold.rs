//! A payment chained across two node implementations under one payment hash.
//!
//! Alice (ldk-server) pays a BOLT12 offer that the maker's ldk-server holds on a hash chosen by
//! Charlie. The maker's RLN node forwards the value as an RGB payment to Charlie's HODL invoice
//! for the same hash. Charlie's claim reveals the preimage to the maker's RLN node, which the
//! maker then uses to settle Alice's held payment, so Alice's proof of payment is Charlie's
//! preimage.
//!
//! Needs `LDK_SERVER_BIN` and `LDK_SERVER_CLI` pointing at binaries built from
//! kaleidoswap/ldk-server@feat/bolt12-receive-for-hash, so it is ignored by default:
//! `cargo test chain_bolt12_hold -- --ignored`.

use std::process::{Child, Command};

use super::*;

const TEST_DIR_BASE: &str = "tmp/chain_bolt12_hold/";
const BITCOIND_RPC: &str = "127.0.0.1:18443";
const HOLD_CLTV_DELTA: &str = "144";
const OFFER_AMOUNT: &str = "50000sat";

struct LdkServer {
    child: Child,
    dir: PathBuf,
    grpc: String,
    p2p: String,
}

impl Drop for LdkServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl LdkServer {
    async fn start(dir: &str, grpc_port: u16, p2p_port: u16) -> Self {
        let dir = PathBuf::from(dir);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p2p = format!("127.0.0.1:{p2p_port}");
        let grpc = format!("127.0.0.1:{grpc_port}");
        let config = format!(
            "[node]\nnetwork = \"regtest\"\nlistening_addresses = [\"{p2p}\"]\n\
             announcement_addresses = [\"{p2p}\"]\ngrpc_service_address = \"{grpc}\"\n\
             alias = \"chain\"\n\n[storage.disk]\ndir_path = \"{}\"\n\n[bitcoind]\n\
             rpc_address = \"{BITCOIND_RPC}\"\nrpc_user = \"user\"\nrpc_password = \"password\"\n",
            dir.display()
        );
        let config_path = dir.join("config.toml");
        std::fs::write(&config_path, config).unwrap();
        let bin = std::env::var("LDK_SERVER_BIN").expect("LDK_SERVER_BIN must be set");
        let log = std::fs::File::create(dir.join("ldk-server.log")).unwrap();
        let child = Command::new(bin)
            .arg(&config_path)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let server = Self {
            child,
            dir,
            grpc,
            p2p,
        };
        let t_0 = OffsetDateTime::now_utc();
        while !server.macaroon_path().exists() || !server.dir.join("tls.crt").exists() {
            if (OffsetDateTime::now_utc() - t_0).as_seconds_f32() > 30.0 {
                panic!("ldk-server in {} did not start", server.dir.display());
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        // the macaroon file can appear before it is fully written
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        server
    }

    fn macaroon_path(&self) -> PathBuf {
        self.dir
            .join("regtest")
            .join("macaroons")
            .join("admin.macaroon")
    }

    fn cli(&self, args: &[&str]) -> serde_json::Value {
        let cli = std::env::var("LDK_SERVER_CLI").expect("LDK_SERVER_CLI must be set");
        let macaroon = std::fs::read_to_string(self.macaroon_path()).unwrap();
        let output = Command::new(cli)
            .arg("--base-url")
            .arg(&self.grpc)
            .arg("--macaroon")
            .arg(macaroon.trim())
            .arg("--tls-cert")
            .arg(self.dir.join("tls.crt"))
            .args(args)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "ldk-server-cli {args:?} failed: {stdout} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_str(&stdout).unwrap_or_else(|_| panic!("non-JSON output: {stdout}"))
    }

    fn node_id(&self) -> String {
        self.cli(&["get-node-info"])["node_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn payment_with_hash(&self, payment_hash: &str) -> Option<serde_json::Value> {
        self.cli(&["list-payments"])["list"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p.to_string().contains(payment_hash))
            .cloned()
    }
}

async fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let t_0 = OffsetDateTime::now_utc();
    while !ready() {
        if (OffsetDateTime::now_utc() - t_0).as_seconds_f32() > 120.0 {
            panic!("timeout waiting for {what}");
        }
        mine_n_blocks(false, 1);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[serial_test::serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[traced_test]
#[ignore = "needs LDK_SERVER_BIN and LDK_SERVER_CLI"]
async fn chain_bolt12_hold_to_rgb_hodl() {
    initialize();

    // RLN side: the maker's node holds the asset in a channel to Charlie
    let test_dir_base = TEST_DIR_BASE.to_string();
    let maker_rln_port = NODE1_PEER_PORT + 90;
    let charlie_port = NODE2_PEER_PORT + 90;
    let charlie_dir = format!("{test_dir_base}charlie");
    let (maker_rln, _) =
        start_node(&format!("{test_dir_base}maker-rln"), maker_rln_port, false).await;
    let (charlie, _) = start_node(&charlie_dir, charlie_port, false).await;
    fund_and_create_utxos(maker_rln, None).await;
    fund_and_create_utxos(charlie, None).await;
    let asset_id = issue_asset_nia(maker_rln).await.asset_id;
    fund_and_create_utxos(maker_rln, None).await;
    let charlie_pubkey = node_info(charlie).await.pubkey;
    open_channel_with_retry(
        maker_rln,
        &charlie_pubkey,
        Some(charlie_port),
        Some(500_000),
        Some(0),
        Some(100),
        Some(&asset_id),
        None,
        5,
    )
    .await;

    // LDK side: Alice opens an announced channel to the maker, so the maker's invoice can use
    // a one-hop blinded path naming itself
    let alice = LdkServer::start(&format!("{test_dir_base}alice"), 13900, 9890).await;
    let maker_ln = LdkServer::start(&format!("{test_dir_base}maker-ln"), 13901, 9891).await;
    for server in [&alice, &maker_ln] {
        let address = server.cli(&["onchain-receive"])["address"]
            .as_str()
            .unwrap()
            .to_string();
        fund_wallet(address, 10_000_000);
    }
    mine_n_blocks(false, 6);
    wait_until("ldk-server on-chain funds", || {
        alice.cli(&["get-balances"])["spendable_onchain_balance_sats"].as_u64() > Some(0)
    })
    .await;
    let maker_ln_id = maker_ln.node_id();
    alice.cli(&[
        "open-channel",
        &maker_ln_id,
        &maker_ln.p2p,
        "2000000sat",
        "--announce-channel",
    ]);
    wait_until("usable announced LDK channel", || {
        maker_ln.cli(&["list-channels"])["channels"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["is_usable"] == true))
    })
    .await;
    wait_until("maker in its own network graph", || {
        let cli = std::env::var("LDK_SERVER_CLI").unwrap();
        let macaroon = std::fs::read_to_string(maker_ln.macaroon_path()).unwrap();
        Command::new(cli)
            .args([
                "--base-url",
                &maker_ln.grpc,
                "--macaroon",
                macaroon.trim(),
                "--tls-cert",
            ])
            .arg(maker_ln.dir.join("tls.crt"))
            .args(["graph-get-node", &maker_ln_id])
            .output()
            .is_ok_and(|o| o.status.success())
    })
    .await;

    // Charlie picks the preimage and asks for the asset under its hash
    let (preimage, payment_hash) = random_preimage_and_hash();
    let charlie_invoice = ln_invoice_hodl(
        charlie,
        Some(HTLC_MIN_MSAT),
        Some(&asset_id),
        Some(10),
        600,
        payment_hash.clone(),
    )
    .await
    .invoice;

    // the maker offers to be paid under the same hash, holding the payment
    let offer = maker_ln.cli(&[
        "bolt12-receive",
        "chain",
        OFFER_AMOUNT,
        "--payment-hash",
        &payment_hash,
        "--min-final-cltv-expiry-delta",
        HOLD_CLTV_DELTA,
    ])["offer"]
        .as_str()
        .unwrap()
        .to_string();
    alice.cli(&["bolt12-send", &offer]);
    wait_until("the maker to hold Alice's payment", || {
        maker_ln.payment_with_hash(&payment_hash).is_some()
    })
    .await;
    let held = maker_ln.payment_with_hash(&payment_hash).unwrap();
    let held_id = held["payment_id"].as_str().unwrap().to_string();

    // the maker forwards as RGB; Charlie's claim reveals the preimage to the maker's RLN node
    send_payment_with_status(maker_rln, charlie_invoice, HTLCStatus::Pending).await;
    wait_for_inbound_payment_status(&charlie_dir, &payment_hash, HTLCStatus::Claimable)
        .await
        .unwrap_or_else(|err| panic!("Charlie's HODL invoice never became claimable: {err}"));
    claim_hodl_invoice(charlie, payment_hash.clone(), preimage.clone()).await;
    let forwarded = wait_for_ln_payment(maker_rln, &payment_hash, HTLCStatus::Succeeded).await;
    let learned = forwarded
        .preimage
        .expect("a succeeded payment carries the preimage");
    assert_eq!(learned, preimage);

    // the maker settles Alice's held payment with what it learned
    maker_ln.cli(&["bolt11-claim-for-id", &held_id, &learned]);
    wait_until("Alice's payment to succeed", || {
        alice
            .payment_with_hash(&payment_hash)
            .is_some_and(|p| p.to_string().to_lowercase().contains("succeeded"))
    })
    .await;
    let alice_payment = alice.payment_with_hash(&payment_hash).unwrap().to_string();
    assert!(
        alice_payment.contains(&preimage),
        "Alice's proof of payment is Charlie's preimage"
    );
    wait_for_ln_balance(charlie, &asset_id, 10).await;
}
