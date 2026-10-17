use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Duration;

use super::*;

const TEST_DIR_BASE: &str = "tmp/nodeinfo_gossip_backfill/";
const NODEINFO_TIMEOUT: Duration = Duration::from_secs(30);
const ROUNDS: usize = 10;
// LDK peers ask for a full gossip sync on their first connections only
const FULL_SYNC_CONNECTIONS: usize = 5;

// /nodeinfo must keep answering while the node backfills gossip to its peers.
//
// When a peer asks for a full gossip sync, the node sends it the whole network graph, taking the
// graph locks while holding that peer's lock. /nodeinfo takes the graph and peer locks too. Here
// three nodes build a public graph and keep reconnecting to node1, each connection asking it for a
// full sync, while node1 is polled on /nodeinfo. Locking and unlocking the peers restarts their
// gossip sync, so they ask for full syncs again.
//
// A plain #[test] with its own runtime, shut down in the background, so that a deadlocked node
// makes the test fail instead of hanging it.
#[serial_test::serial]
#[test]
#[traced_test]
fn nodeinfo_during_gossip_backfill() {
    initialize();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .enable_all()
        .build()
        .unwrap();
    let res = runtime.block_on(async {
        let test_dir_node1 = format!("{TEST_DIR_BASE}node1");
        let test_dir_node2 = format!("{TEST_DIR_BASE}node2");
        let test_dir_node3 = format!("{TEST_DIR_BASE}node3");
        let test_dir_node4 = format!("{TEST_DIR_BASE}node4");
        let (node1_addr, _) = start_node(&test_dir_node1, NODE1_PEER_PORT, false).await;
        let (node2_addr, node2_pw) = start_node(&test_dir_node2, NODE2_PEER_PORT, false).await;
        let (node3_addr, node3_pw) = start_node(&test_dir_node3, NODE3_PEER_PORT, false).await;
        let (node4_addr, node4_pw) = start_node(&test_dir_node4, NODE4_PEER_PORT, false).await;

        fund_and_create_utxos(node2_addr, None).await;
        fund_and_create_utxos(node3_addr, None).await;
        fund_and_create_utxos(node4_addr, None).await;

        let node1_pubkey = node_info(node1_addr).await.pubkey;
        let node2_pubkey = node_info(node2_addr).await.pubkey;
        let node3_pubkey = node_info(node3_addr).await.pubkey;
        let node4_pubkey = node_info(node4_addr).await.pubkey;

        open_channel(
            node2_addr,
            &node3_pubkey,
            Some(NODE3_PEER_PORT),
            None,
            None,
            None,
            None,
        )
        .await;
        open_channel(
            node3_addr,
            &node4_pubkey,
            Some(NODE4_PEER_PORT),
            None,
            None,
            None,
            None,
        )
        .await;
        open_channel(
            node4_addr,
            &node2_pubkey,
            Some(NODE2_PEER_PORT),
            None,
            None,
            None,
            None,
        )
        .await;

        // node1 learns the public graph from the peers
        let peers = [
            (node2_addr, node2_pw, node2_pubkey, NODE2_PEER_PORT),
            (node3_addr, node3_pw, node3_pubkey, NODE3_PEER_PORT),
            (node4_addr, node4_pw, node4_pubkey, NODE4_PEER_PORT),
        ];
        for (_, _, pubkey, port) in &peers {
            connect_peer(node1_addr, pubkey, &format!("127.0.0.1:{port}")).await;
        }
        let t_0 = OffsetDateTime::now_utc();
        while node_info(node1_addr).await.network_channels < 3 {
            if (OffsetDateTime::now_utc() - t_0).as_seconds_f32() > 120.0 {
                panic!("node1 did not learn the public graph");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        for (_, _, pubkey, _) in &peers {
            disconnect_peer(node1_addr, pubkey).await;
        }

        let stop = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let (stop_poll, calls_poll) = (stop.clone(), calls.clone());
        let mut poller = tokio::spawn(async move {
            let client = reqwest::Client::new();
            while !stop_poll.load(Ordering::SeqCst) {
                client
                    .get(format!("http://{node1_addr}/nodeinfo"))
                    .timeout(NODEINFO_TIMEOUT)
                    .send()
                    .await
                    .map_err(|e| {
                        format!(
                            "/nodeinfo #{} did not answer (timed out: {}): {e}",
                            calls_poll.load(Ordering::SeqCst),
                            e.is_timeout()
                        )
                    })?;
                calls_poll.fetch_add(1, Ordering::SeqCst);
            }
            Ok::<_, String>(())
        });

        let node1_pubkey = &node1_pubkey;
        let reconnect =
            futures::future::join_all(peers.iter().map(|(addr, password, _, _)| async move {
                for round in 0..ROUNDS {
                    for _ in 0..FULL_SYNC_CONNECTIONS {
                        connect_peer(*addr, node1_pubkey, &format!("127.0.0.1:{NODE1_PEER_PORT}"))
                            .await;
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        disconnect_peer(*addr, node1_pubkey).await;
                    }
                    println!("peer {addr} finished round {round}");
                    lock(*addr).await;
                    unlock(*addr, password).await;
                }
            }));
        tokio::select! {
            res = &mut poller => res.unwrap()?,
            _ = reconnect => {}
        }

        stop.store(true, Ordering::SeqCst);
        poller.await.unwrap()?;
        Ok::<_, String>(calls.load(Ordering::SeqCst))
    });
    runtime.shutdown_background();

    let calls = res.unwrap();
    assert!(calls > 0);
}
