use super::*;

use ldk_server_client::client::LdkServerClient;
use ldk_server_client::ldk_server_grpc::api::{
    GetBalancesRequest, GetNodeInfoRequest, ListChannelsRequest, OnchainReceiveRequest,
    OpenChannelRequest,
};
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TEST_DIR_BASE: &str = "tmp/stock_ldk_interop/";

struct StockLdkNode {
    child: Child,
    client: LdkServerClient,
    node_id: String,
}

impl StockLdkNode {
    async fn start(test_dir: &str) -> Self {
        if Path::new(test_dir).exists() {
            std::fs::remove_dir_all(test_dir).unwrap();
        }
        std::fs::create_dir_all(test_dir).unwrap();

        let grpc_port = available_port();
        let p2p_port = available_port();
        let config_path = Path::new(test_dir).join("config.toml");
        let config = format!(
            r#"[node]
network = "regtest"
listening_addresses = ["127.0.0.1:{p2p_port}"]
grpc_service_address = "127.0.0.1:{grpc_port}"
alias = "stock-ldk-interop"

[storage.disk]
dir_path = "{test_dir}"

[bitcoind]
rpc_address = "127.0.0.1:18443"
rpc_user = "user"
rpc_password = "password"

[liquidity.lsps2_service]
advertise_service = false
channel_opening_fee_ppm = 10000
channel_over_provisioning_ppm = 100000
min_channel_opening_fee_msat = 0
min_channel_lifetime = 100
max_client_to_self_delay = 1024
min_payment_size_msat = 0
max_payment_size_msat = 1000000000
client_trusts_lsp = true
disable_client_reserve = false

[metrics]
enabled = false
"#,
        );
        std::fs::write(&config_path, config).unwrap();

        let mut child = Command::new(stock_ldk_server_binary())
            .arg(&config_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start stock ldk-server fixture");

        forward_logs("stdout", child.stdout.take().unwrap());
        forward_logs("stderr", child.stderr.take().unwrap());

        let api_key_path = Path::new(test_dir).join("regtest/api_key");
        let tls_cert_path = Path::new(test_dir).join("tls.crt");
        wait_for_file(&api_key_path, Duration::from_secs(30)).await;
        wait_for_file(&tls_cert_path, Duration::from_secs(30)).await;

        let api_key = std::fs::read(api_key_path)
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let tls_cert = std::fs::read(tls_cert_path).unwrap();
        let client =
            LdkServerClient::new(format!("127.0.0.1:{grpc_port}"), api_key, &tls_cert).unwrap();

        let started = Instant::now();
        let node_id = loop {
            if let Ok(info) = client.get_node_info(GetNodeInfoRequest {}).await {
                break info.node_id;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "stock ldk-server did not become ready"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        };

        Self {
            child,
            client,
            node_id,
        }
    }
}

fn stock_ldk_server_binary() -> PathBuf {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/ldk-server");
    let target_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/stock-ldk");
    let binary = target_dir.join("debug/ldk-server");
    if binary.exists() {
        return binary;
    }

    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--locked",
            "-p",
            "ldk-server",
            "--features",
            "experimental-lsps2-support",
        ])
        .current_dir(fixture_dir)
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .expect("failed to build stock ldk-server fixture");
    assert!(status.success(), "failed to build stock ldk-server fixture");
    binary
}

impl Drop for StockLdkNode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn available_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn forward_logs(label: &'static str, stream: impl std::io::Read + Send + 'static) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            eprintln!("[stock ldk-server {label}] {line}");
        }
    });
}

async fn wait_for_file(path: &Path, timeout: Duration) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for {path:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[serial_test::serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[traced_test]
async fn accepts_plain_channel_opened_by_stock_ldk() {
    initialize();

    let rln_dir = format!("{TEST_DIR_BASE}rln");
    let stock_dir = format!("{TEST_DIR_BASE}stock");
    let (rln_addr, _) = start_node(&rln_dir, NODE1_PEER_PORT, false).await;
    let stock = StockLdkNode::start(&stock_dir).await;

    fund_and_create_utxos(rln_addr, None).await;
    let stock_address = stock
        .client
        .onchain_receive(OnchainReceiveRequest {})
        .await
        .unwrap()
        .address;
    fund_wallet(stock_address, 100_000_000);
    mine_n_blocks(false, 6);

    let started = Instant::now();
    loop {
        let balances = stock
            .client
            .get_balances(GetBalancesRequest {})
            .await
            .unwrap();
        if balances.spendable_onchain_balance_sats > 0 {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "stock LDK wallet did not see its confirmed funds"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    let rln_pubkey = node_info(rln_addr).await.pubkey;
    stock
        .client
        .open_channel(OpenChannelRequest {
            node_pubkey: rln_pubkey.clone(),
            address: format!("127.0.0.1:{NODE1_PEER_PORT}"),
            channel_amount_sats: 600_000,
            push_to_counterparty_msat: None,
            channel_config: None,
            announce_channel: false,
            disable_counterparty_reserve: false,
        })
        .await
        .unwrap();

    let started = Instant::now();
    loop {
        let channels = stock
            .client
            .list_channels(ListChannelsRequest {})
            .await
            .unwrap();
        if channels
            .channels
            .iter()
            .any(|channel| channel.counterparty_node_id == rln_pubkey && channel.is_channel_ready)
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "plain channel from stock LDK did not reach channel_ready"
        );
        mine_n_blocks(false, 1);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    assert!(list_channels(rln_addr)
        .await
        .iter()
        .any(|channel| channel.peer_pubkey == stock.node_id && channel.ready));
}
