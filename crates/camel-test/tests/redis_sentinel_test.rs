//! Redis Sentinel failover integration tests.
//!
//! Proves that `redis-sentinel://` producers and consumers recover from a
//! real Sentinel failover issued with `SENTINEL FAILOVER`:
//!
//! - a producer SET/GET round-trip succeeds against the newly elected master,
//! - a BLPOP queue consumer delivers an item pushed after the failover,
//! - a SUBSCRIBE consumer resubscribes and delivers a post-failover publish.
//!
//! Topology (self-provisioned, single `redis` container, fixed loopback
//! ports): one master on 16379, one replica on 16380 announcing itself as
//! `127.0.0.1:16380`, and one sentinel on 26379 monitoring `mymaster` with
//! quorum 1. All three processes live in one container, and each port is
//! published with the same number on the host. Sentinel therefore reports
//! master addresses (`127.0.0.1:16379` or `127.0.0.1:16380`) that the test
//! process can reach directly — no Docker network aliases are involved.
//!
//! Each test triggers one failover, so the master role ping-pongs between
//! the two nodes across the suite. Every test reads the current master from
//! the sentinel instead of assuming a fixed port. A process-wide lock keeps
//! the three tests sequential because they share the topology.
//!
//! These tests run in CI's `full-tests-linux` job behind
//! `--features integration-tests` and require a Docker daemon with loopback
//! port publishing (a remote `DOCKER_HOST` will not work). They are never
//! `#[ignore]`d (ADR-0054).

#![cfg(feature = "integration-tests")]

mod support;

use std::time::{Duration, Instant};

use camel_api::{Body, Exchange, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_redis::{RedisComponent, RedisSentinelComponent};
use camel_test::CamelTestContext;
use redis::Commands;
use support::send_to_direct;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::{Mutex, OnceCell};

const MASTER_PORT: u16 = 16379;
const REPLICA_PORT: u16 = 16380;
const SENTINEL_PORT: u16 = 26379;
const MASTER_NAME: &str = "mymaster";

/// Labels this suite's container so a previous crashed run can be identified
/// and removed before the fixed ports are bound again.
const TOPOLOGY_LABEL_KEY: &str = "org.rust-camel.redis-sentinel-test";
const TOPOLOGY_LABEL_VALUE: &str = "true";

/// Hard bound for every failover-recovery wait in this suite.
const FAILOVER_DEADLINE: Duration = Duration::from_secs(60);
/// Poll interval shared by the failover-recovery loops.
const RECOVERY_POLL: Duration = Duration::from_millis(500);

/// Serializes the three tests: they share one topology and each issues a
/// failover, so two failovers in flight at once would corrupt each other.
static TOPOLOGY_LOCK: Mutex<()> = Mutex::const_new(());

static TOPOLOGY: OnceCell<ContainerAsync<GenericImage>> = OnceCell::const_new();

/// Force-removes containers left by a previous crashed run of this suite.
///
/// The topology publishes fixed host ports, so a stale container that still
/// holds them would make every subsequent start fail with "port is already
/// allocated". Removal is best-effort: a Docker failure here surfaces as the
/// normal start error below.
async fn remove_stale_topology_containers() {
    use std::collections::HashMap;

    let docker = match bollard::Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert(
        "label".to_string(),
        vec![format!("{TOPOLOGY_LABEL_KEY}={TOPOLOGY_LABEL_VALUE}")],
    );
    let options = bollard::query_parameters::ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    let stale = match docker.list_containers(Some(options)).await {
        Ok(list) => list,
        Err(_) => return,
    };
    let remove = bollard::query_parameters::RemoveContainerOptionsBuilder::default()
        .force(true)
        .build();
    for container in stale {
        if let Some(id) = container.id {
            let _ = docker.remove_container(&id, Some(remove.clone())).await;
        }
    }
}

/// Starts (once) the master + replica + sentinel container described in the
/// module doc and waits until sentinel tracks the master and the replica is
/// connected, so the first `SENTINEL FAILOVER` has a promotion candidate.
async fn shared_topology() -> &'static ContainerAsync<GenericImage> {
    TOPOLOGY
        .get_or_init(|| async {
            support::init_tracing();
            remove_stale_topology_containers().await;

            let script = format!(
                "set -e\n\
                 redis-server --port {MASTER_PORT} --daemonize yes\n\
                 until redis-cli -p {MASTER_PORT} ping | grep -q PONG; do sleep 0.1; done\n\
                 redis-server --port {REPLICA_PORT} --daemonize yes \
                 --slaveof 127.0.0.1 {MASTER_PORT} \
                 --slave-announce-ip 127.0.0.1 \
                 --slave-announce-port {REPLICA_PORT}\n\
                 until redis-cli -p {REPLICA_PORT} ping | grep -q PONG; do sleep 0.1; done\n\
                 printf 'port {SENTINEL_PORT}\\n\
                 sentinel monitor {MASTER_NAME} 127.0.0.1 {MASTER_PORT} 1\\n\
                 sentinel down-after-milliseconds {MASTER_NAME} 2000\\n\
                 sentinel failover-timeout {MASTER_NAME} 10000\\n\
                 sentinel parallel-syncs {MASTER_NAME} 1\\n' > /tmp/sentinel.conf\n\
                 exec redis-sentinel /tmp/sentinel.conf\n"
            );

            let image = GenericImage::new("redis", "5.0")
                .with_cmd(["sh", "-c", &script])
                .with_label(TOPOLOGY_LABEL_KEY, TOPOLOGY_LABEL_VALUE)
                .with_mapped_port(MASTER_PORT, ContainerPort::Tcp(MASTER_PORT))
                .with_mapped_port(REPLICA_PORT, ContainerPort::Tcp(REPLICA_PORT))
                .with_mapped_port(SENTINEL_PORT, ContainerPort::Tcp(SENTINEL_PORT))
                .with_ready_conditions(vec![WaitFor::message_on_stdout("+monitor")]);

            let container = image
                .start()
                .await
                .expect("redis sentinel topology failed to start");
            eprintln!(
                "redis sentinel topology ready: master 127.0.0.1:{MASTER_PORT}, \
                 replica 127.0.0.1:{REPLICA_PORT}, sentinel 127.0.0.1:{SENTINEL_PORT}"
            );

            // Sentinel must report the initial master, and the master must
            // have the replica attached, otherwise the first manual failover
            // has nothing to promote.
            support::wait::wait_until(
                "sentinel tracks master with a connected replica",
                Duration::from_secs(30),
                Duration::from_millis(250),
                || async {
                    let Ok((ip, port)) = sentinel_master_addr() else {
                        return Ok(false);
                    };
                    if ip != "127.0.0.1" || port != MASTER_PORT.to_string() {
                        return Ok(false);
                    }
                    Ok(master_has_connected_replica(MASTER_PORT))
                },
            )
            .await
            .expect("sentinel topology never became ready");

            container
        })
        .await
}

// ── Sentinel and direct-node clients (synchronous, per task spec) ───────────

fn sentinel_conn() -> redis::Connection {
    let client = redis::Client::open(format!("redis://127.0.0.1:{SENTINEL_PORT}"))
        .expect("open sentinel client");
    client.get_connection().expect("connect to sentinel")
}

/// Current master address as tracked by sentinel, e.g. ("127.0.0.1", "16379").
fn sentinel_master_addr() -> Result<(String, String), String> {
    let mut conn = sentinel_conn();
    redis::cmd("SENTINEL")
        .arg("get-master-addr-by-name")
        .arg(MASTER_NAME)
        .query::<(String, String)>(&mut conn)
        .map_err(|e| format!("SENTINEL get-master-addr-by-name failed: {e}"))
}

/// Issues `SENTINEL FAILOVER mymaster`. Returns once the failover has been
/// accepted; the switch itself is polled separately by the caller.
///
/// A young topology can pass the master/replica readiness gate before
/// sentinel's own replica discovery has marked the replica promotable, in
/// which case the command fails with NOGOODSLAVE. That is a topology-warmup
/// condition, not a failover result, so it is retried within a bounded
/// window; the caller's recovery deadline keeps running unchanged.
fn trigger_failover() {
    let give_up = Instant::now() + Duration::from_secs(20);
    loop {
        let mut conn = sentinel_conn();
        let outcome: Result<(), redis::RedisError> = redis::cmd("SENTINEL")
            .arg("FAILOVER")
            .arg(MASTER_NAME)
            .query(&mut conn);
        match outcome {
            Ok(()) => return,
            Err(e) if e.to_string().contains("NOGOODSLAVE") => {
                assert!(
                    Instant::now() < give_up,
                    "sentinel never obtained a promotable replica: {e}"
                );
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => panic!("SENTINEL FAILOVER command: {e}"),
        }
    }
}

fn direct_conn(port: u16) -> Result<redis::Connection, String> {
    let client = redis::Client::open(format!("redis://127.0.0.1:{port}"))
        .map_err(|e| format!("open direct client failed: {e}"))?;
    client
        .get_connection()
        .map_err(|e| format!("direct connect to 127.0.0.1:{port} failed: {e}"))
}

fn master_has_connected_replica(port: u16) -> bool {
    let mut conn = match direct_conn(port) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let info: String = redis::cmd("INFO")
        .arg("replication")
        .query(&mut conn)
        .unwrap_or_default();
    info.contains("role:master") && info.contains("connected_slaves:1")
}

/// True when the node at `port` reports `role:slave` in INFO replication.
fn node_is_replica(port: u16) -> bool {
    direct_conn(port)
        .ok()
        .and_then(|mut conn| {
            redis::cmd("INFO")
                .arg("replication")
                .query::<String>(&mut conn)
                .ok()
        })
        .map(|info| info.contains("role:slave"))
        .unwrap_or(false)
}

/// Polls the node at `port` until its demotion to replica completes.
/// Sentinel announces `+switch-master` before it reconfigures the old
/// master with SLAVEOF, so a write issued right after the address flip can
/// still land on the old node through a stale connection. Only after the
/// demotion is an accepted write provably against the new master.
async fn wait_node_demoted(port: u16, deadline: Instant) {
    loop {
        if node_is_replica(port) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "old master on port {port} was not demoted to replica within the deadline"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Best-effort `CLIENT KILL` for every client listed on the node at `port`.
/// A manual `SENTINEL FAILOVER` leaves the old node alive, so the
/// component's connections to it survive and can keep serving the demoted
/// replica (replicated PUBLISHes deliver to subscribers on a replica, and
/// list data replicates too). Killing them forces the disconnect — and
/// therefore the sentinel re-resolve — that a crash failover produces,
/// which is the recovery path this suite exists to prove.
fn kill_node_clients(port: u16) {
    let addrs = direct_conn(port).ok().and_then(|mut conn| {
        redis::cmd("CLIENT")
            .arg("LIST")
            .query::<String>(&mut conn)
            .ok()
            .map(|list| {
                list.lines()
                    .filter_map(|line| {
                        line.split_whitespace()
                            .find(|field| field.starts_with("addr="))
                            .map(|field| field.trim_start_matches("addr=").to_string())
                    })
                    .collect::<Vec<String>>()
            })
    });
    let Some(addrs) = addrs else { return };
    for addr in addrs {
        if let Ok(mut conn) = direct_conn(port) {
            // SKIPME (the default) protects the killing connection itself.
            let _: Result<(), redis::RedisError> = redis::cmd("CLIENT")
                .arg("KILL")
                .arg("ADDR")
                .arg(&addr)
                .query(&mut conn);
        }
    }
}

fn lpush_to_master(port: u16, key: &str, value: &str) {
    let mut conn = direct_conn(port).expect("connect to current master");
    let _: i64 = conn.lpush(key, value).expect("LPUSH to current master");
}

fn publish_to_master(port: u16, channel: &str, message: &str) {
    let mut conn = direct_conn(port).expect("connect to current master");
    let _: i64 = conn
        .publish(channel, message)
        .expect("PUBLISH to current master");
}

/// Subscriber count for `channel` on the redis node at `port` (`PUBSUB
/// NUMSUB`). Returns 0 when the node is unreachable or the query fails.
fn channel_subscribers(port: u16, channel: &str) -> i64 {
    direct_conn(port)
        .ok()
        .and_then(|mut conn| {
            redis::cmd("PUBSUB")
                .arg("NUMSUB")
                .arg(channel)
                .query::<(String, i64)>(&mut conn)
                .ok()
        })
        .map(|(_, n)| n)
        .unwrap_or(0)
}

/// Waits until the node at `port` reports at least one subscriber on
/// `channel`. Pub/Sub delivery is fire-and-forget, so a publish issued
/// before the SUBSCRIBE handshake completes is lost silently; gating the
/// pre-failover publish on this removes that race.
async fn wait_channel_subscribed(port: u16, channel: &str) {
    support::wait::wait_until(
        "SUBSCRIBE handshake complete",
        Duration::from_secs(10),
        Duration::from_millis(100),
        || async { Ok(channel_subscribers(port, channel) >= 1) },
    )
    .await
    .expect("subscription never went live before the pre-failover publish");
}

// ── Harness helpers ─────────────────────────────────────────────────────────

fn sentinel_uri() -> String {
    format!("redis-sentinel://127.0.0.1:{SENTINEL_PORT}/{MASTER_NAME}/0")
}

async fn sentinel_harness() -> CamelTestContext {
    shared_topology().await;
    CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_direct()
        .with_component(RedisComponent::new())
        .with_component(RedisSentinelComponent::new())
        .build()
        .await
}

async fn received_bodies(h: &CamelTestContext, endpoint: &str) -> Vec<String> {
    h.mock()
        .get_endpoint(endpoint)
        .expect("mock endpoint registered")
        .get_received_exchanges()
        .await
        .iter()
        .map(|ex| {
            ex.input
                .body
                .as_text()
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_default()
        })
        .collect()
}

/// Sends one SET through the sentinel producer and returns true when the
/// exchange reached `mock:set-done` (the SET was accepted by the current
/// master). A failed SET never reaches the mock, which keeps the signal
/// independent of how direct-route errors surface to the caller.
async fn send_set(h: &CamelTestContext, value: &str) -> bool {
    let before = received_bodies(h, "set-done").await.len();
    let mut ex = Exchange::default();
    ex.input
        .set_header("CamelRedis.Value", Value::String(value.to_string()));
    let _ = send_to_direct(h, "direct:sentinel-set", ex).await;
    support::wait::wait_until(
        "SET exchange delivered",
        Duration::from_secs(2),
        Duration::from_millis(50),
        || async { Ok(received_bodies(h, "set-done").await.len() > before) },
    )
    .await
    .is_ok()
}

/// Sends GETs through the sentinel producer until the body matches `expected`
/// or `timeout` elapses. Redis command results land in the body as JSON
/// (`Body::Json`), so both the Text and JSON-string shapes are compared.
/// Returns the last observed value.
async fn poll_get(h: &CamelTestContext, expected: &str, timeout: Duration) -> Option<String> {
    let body_string = |ex: &Exchange| -> Option<String> {
        match &ex.input.body {
            Body::Text(s) => Some(s.trim_matches('"').to_string()),
            Body::Json(serde_json::Value::String(s)) => Some(s.clone()),
            _ => None,
        }
    };
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(ex) = send_to_direct(h, "direct:sentinel-get", Exchange::default()).await {
            let got = body_string(&ex);
            if got.as_deref() == Some(expected) {
                return got;
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Polls until sentinel reports a master port other than `previous_port`.
/// Fails the test (hard deadline) if the switch does not happen in time.
async fn wait_master_switched(previous_port: &str, deadline: Instant) -> u16 {
    loop {
        match sentinel_master_addr() {
            Ok((ip, port)) if ip == "127.0.0.1" && port != previous_port => {
                return port.parse().expect("master port is numeric");
            }
            Ok(_) => {}
            Err(_) => {}
        }
        assert!(
            Instant::now() < deadline,
            "sentinel did not switch the master away from port {previous_port} within the deadline"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

// ===========================================================================
// Producer failover recovery
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn producer_recovers_after_sentinel_failover() {
    let _guard = TOPOLOGY_LOCK.lock().await;
    let h = sentinel_harness().await;
    let uri = sentinel_uri();

    // The route pins CamelRedis.Key; CamelRedis.Value rides on each exchange
    // so every probe attempt writes a distinct value.
    let set_route = RouteBuilder::from("direct:sentinel-set")
        .set_header("CamelRedis.Key", Value::String("failover:probe".into()))
        .to(format!("{uri}?command=SET"))
        .to("mock:set-done")
        .route_id("sentinel-producer-set")
        .build()
        .unwrap();
    let get_route = RouteBuilder::from("direct:sentinel-get")
        .set_header("CamelRedis.Key", Value::String("failover:probe".into()))
        .to(format!("{uri}?command=GET"))
        .to("mock:get-done")
        .route_id("sentinel-producer-get")
        .build()
        .unwrap();
    h.add_route(set_route).await.unwrap();
    h.add_route(get_route).await.unwrap();
    h.start().await;

    // Baseline: SET + GET round-trip through the sentinel topology.
    assert!(
        send_set(&h, "pre-failover").await,
        "baseline SET must succeed before the failover"
    );
    let got = poll_get(&h, "pre-failover", Duration::from_secs(10)).await;
    assert_eq!(
        got.as_deref(),
        Some("pre-failover"),
        "baseline GET round-trip"
    );

    let (_, before_port) = sentinel_master_addr().unwrap();

    let deadline = Instant::now() + FAILOVER_DEADLINE;
    trigger_failover();
    let new_port = wait_master_switched(&before_port, deadline).await;
    // Sentinel announces the switch before the old master is demoted;
    // wait for the demotion so any later accepted SET provably hit the
    // new master (a stale connection to the old node is read-only then).
    wait_node_demoted(before_port.parse::<u16>().unwrap(), deadline).await;
    assert_ne!(
        new_port,
        before_port.parse::<u16>().unwrap(),
        "master must change"
    );

    let mut attempt = 0usize;
    loop {
        let value = format!("post-failover-{attempt}");
        if send_set(&h, &value).await {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or(Duration::ZERO);
            let got = poll_get(&h, &value, remaining.min(Duration::from_secs(10))).await;
            if got.as_deref() == Some(value.as_str()) {
                break; // recovered: SET+GET round-trip against the new master
            }
        }
        assert!(
            Instant::now() < deadline,
            "producer did not SET/GET against the new master within {:?}",
            FAILOVER_DEADLINE
        );
        tokio::time::sleep(RECOVERY_POLL).await;
        attempt += 1;
    }

    h.stop().await;
}

// ===========================================================================
// Queue consumer failover recovery
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn queue_consumer_recovers_after_sentinel_failover() {
    let _guard = TOPOLOGY_LOCK.lock().await;
    let h = sentinel_harness().await;
    let uri = sentinel_uri();

    let consumer = RouteBuilder::from(&format!("{uri}?command=BLPOP&key=failover:queue&timeout=1"))
        .to("mock:queue-consumed")
        .route_id("sentinel-queue-consumer")
        .build()
        .unwrap();
    h.add_route(consumer).await.unwrap();
    h.start().await;

    let (_, before_port) = sentinel_master_addr().unwrap();
    let before_port_num: u16 = before_port.parse().unwrap();

    // Pre-failover delivery: an item pushed to the current master is
    // delivered as an Exchange.
    lpush_to_master(before_port_num, "failover:queue", "pre-failover-item");
    support::wait::wait_until(
        "pre-failover queue item delivered",
        Duration::from_secs(10),
        Duration::from_millis(100),
        || async { Ok(!received_bodies(&h, "queue-consumed").await.is_empty()) },
    )
    .await
    .unwrap();

    let deadline = Instant::now() + FAILOVER_DEADLINE;
    trigger_failover();
    let new_port = wait_master_switched(&before_port, deadline).await;
    // Break the consumer's blocked BLPOP connection on the old node: the
    // manual failover leaves it alive, and without this the stale
    // connection could be served by replicated data instead of forcing
    // the sentinel re-resolve the suite must prove.
    kill_node_clients(before_port_num);

    // Push after the switch: the item sits on the new master until the
    // consumer's reconnect loop re-resolves the master and BLPOPs it.
    lpush_to_master(new_port, "failover:queue", "post-failover-item");

    loop {
        let bodies = received_bodies(&h, "queue-consumed").await;
        if bodies.iter().any(|b| b == "post-failover-item") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "queue consumer did not deliver the post-failover item within {:?}",
            FAILOVER_DEADLINE
        );
        tokio::time::sleep(RECOVERY_POLL).await;
    }

    h.stop().await;

    let bodies = received_bodies(&h, "queue-consumed").await;
    assert!(
        bodies.iter().any(|b| b == "post-failover-item"),
        "post-failover item must be among the delivered bodies: {bodies:?}"
    );
    assert!(
        bodies.iter().any(|b| b == "pre-failover-item"),
        "pre-failover item must also have been delivered: {bodies:?}"
    );
}

// ===========================================================================
// Pub/Sub consumer failover recovery
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn pubsub_consumer_resubscribes_after_sentinel_failover() {
    let _guard = TOPOLOGY_LOCK.lock().await;
    let h = sentinel_harness().await;
    let uri = sentinel_uri();

    let consumer = RouteBuilder::from(&format!("{uri}?command=SUBSCRIBE&channels=failover:chan"))
        .to("mock:pubsub-received")
        .route_id("sentinel-pubsub-consumer")
        .build()
        .unwrap();
    h.add_route(consumer).await.unwrap();
    h.start().await;

    let (_, before_port) = sentinel_master_addr().unwrap();
    let before_port_num: u16 = before_port.parse().unwrap();

    // Pre-failover delivery proves the subscription is live. The NUMSUB gate
    // makes sure the publish cannot race the SUBSCRIBE handshake.
    wait_channel_subscribed(before_port_num, "failover:chan").await;
    publish_to_master(before_port_num, "failover:chan", "pre-failover-msg");
    support::wait::wait_until(
        "pre-failover pubsub message delivered",
        Duration::from_secs(10),
        Duration::from_millis(100),
        || async { Ok(!received_bodies(&h, "pubsub-received").await.is_empty()) },
    )
    .await
    .unwrap();

    let deadline = Instant::now() + FAILOVER_DEADLINE;
    trigger_failover();
    let new_port = wait_master_switched(&before_port, deadline).await;
    // Break the consumer's subscribed connection on the old node. Without
    // this, replicated PUBLISHes would keep reaching the stale subscriber
    // on the demoted replica and the test could pass without any
    // resubscription ever happening.
    kill_node_clients(before_port_num);

    // Pub/Sub delivery is best-effort (a publish while the consumer is
    // between subscriptions is lost), so publish repeatedly until the
    // resubscribed consumer delivers one of the post-failover messages.
    let mut attempt = 0usize;
    loop {
        let message = format!("post-failover-msg-{attempt}");
        publish_to_master(new_port, "failover:chan", &message);
        let bodies = received_bodies(&h, "pubsub-received").await;
        if bodies.iter().any(|b| b.starts_with("post-failover-msg-")) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "pubsub consumer did not deliver a post-failover message within {:?}",
            FAILOVER_DEADLINE
        );
        tokio::time::sleep(RECOVERY_POLL).await;
        attempt += 1;
    }

    h.stop().await;

    let bodies = received_bodies(&h, "pubsub-received").await;
    assert!(
        bodies.iter().any(|b| b.starts_with("post-failover-msg-")),
        "a post-failover message must be among the delivered bodies: {bodies:?}"
    );
}
