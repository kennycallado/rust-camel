//! Task 2.5 — stale delivery-tag suppression and redelivery after a broker
//! restart, against a real broker.
//!
//! The route is the test itself: it receives each [`ExchangeEnvelope`] from the
//! consumer's mpsc channel and answers it `Ok`. Holding an envelope unanswered
//! blocks the route mid-flight, which is how these tests keep a delivery
//! unacknowledged across a broker restart.
//!
//! Broker-dependent tests are gated by `RABBITMQ_ITEST=1` (see
//! `tests/common/mod.rs`): unset prints a notice and returns early, while
//! `RABBITMQ_ITEST=1` with no docker panics `infra-unavailable` (no silent
//! skip).

mod common;

use std::sync::Arc;
use std::time::Duration;

use camel_api::Body;
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{
    Consumer, ConsumerContext, ExchangeEnvelope, NetworkRetryPolicy, StartupSignal,
};
use camel_component_rabbitmq::config::{RabbitBrokerConfig, RabbitEndpointConfig};
use camel_component_rabbitmq::{RabbitConnectionManager, RabbitConsumer};
use lapin::Channel;
use lapin::options::{BasicPublishOptions, QueueDeleteOptions};
use lapin::types::ShortString;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

/// Deadline for a delivery the test is actively waiting for.
const RECV_BOUND: Duration = Duration::from_secs(20);
/// Deadline for consumer start/stop and broker round trips.
const RPC_BOUND: Duration = Duration::from_secs(30);

/// Serializes the three reconnect tests. They share the process-wide log sink,
/// so running them concurrently would make generation-pair attribution
/// ambiguous; each test holds this for its whole body.
static RECONNECT_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Unique-per-process suffix so parallel tests and re-runs never collide.
fn unique(tag: &str) -> String {
    format!("{tag}-{}-{}", std::process::id(), common::nanos())
}

/// Short, local-retry policy so a broker restart reconnects within the test
/// bound. `max_attempts: 0` is unlimited (the component reconnect default).
fn retry_policy() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 0,
        initial_delay: Duration::from_millis(50),
        multiplier: 1.0,
        max_delay: Duration::from_millis(50),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    }
}

/// Real-lapin manager pointed at the fixture URL.
fn manager_for(url: &str) -> Arc<RabbitConnectionManager> {
    let broker = RabbitBrokerConfig {
        url: url.to_string(),
        username: None,
        password: None,
        vhost: None,
    };
    Arc::new(RabbitConnectionManager::from_broker_config(
        &broker,
        retry_policy(),
    ))
}

/// Start a real consumer and return it with the receiver its engine feeds.
async fn start_consumer(
    config: RabbitEndpointConfig,
    manager: Arc<RabbitConnectionManager>,
    route_id: &str,
) -> (
    RabbitConsumer,
    mpsc::Receiver<ExchangeEnvelope>,
    CancellationToken,
) {
    let (tx, rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();
    let (signal, receiver) = StartupSignal::pair();
    let ctx = ConsumerContext::new(tx, cancel.clone(), route_id.to_string()).with_startup(signal);
    let mut consumer = RabbitConsumer::new(config, manager, Arc::new(NoopRuntimeObservability));

    timeout(RPC_BOUND, consumer.start(ctx))
        .await
        .expect("consumer.start must return within the bound")
        .expect("consumer.start must succeed against the fixture");
    timeout(RPC_BOUND, receiver.await_ready())
        .await
        .expect("readiness must resolve within the bound")
        .expect("readiness must be Ok after a successful start");

    (consumer, rx, cancel)
}

/// Receive one in-flight envelope, bounded.
async fn recv_envelope(rx: &mut mpsc::Receiver<ExchangeEnvelope>) -> ExchangeEnvelope {
    timeout(RECV_BOUND, rx.recv())
        .await
        .expect("a delivery must arrive within the bound")
        .expect("the delivery channel must stay open")
}

/// Stop a consumer and confirm the loop joined within the bound.
async fn stop_consumer(consumer: &mut RabbitConsumer) {
    timeout(RPC_BOUND, consumer.stop())
        .await
        .expect("consumer.stop must return within the bound")
        .expect("consumer.stop must succeed");
}

/// Answer a held envelope `Ok` with its own exchange. A dropped receiver —
/// the engine already observed stop and abandoned its wait — is expected and
/// never unwrapped.
fn answer_ok(envelope: ExchangeEnvelope) {
    if let Some(reply_tx) = envelope.reply_tx {
        let _ = reply_tx.send(Ok(envelope.exchange));
    }
}

/// Assert the exchange payload is exactly `expected` bytes.
fn assert_payload(exchange: &camel_api::Exchange, expected: &[u8]) {
    match &exchange.input.body {
        Body::Bytes(bytes) => assert_eq!(bytes.as_ref(), expected, "unexpected payload"),
        other => panic!("expected Body::Bytes, got {other:?}"),
    }
}

/// Publish `payload` straight to `queue` via the default exchange, awaiting the
/// broker confirm.
async fn publish(channel: &Channel, queue: &str, payload: &[u8]) {
    let confirm = channel
        .basic_publish(
            ShortString::default(),
            ShortString::from(queue),
            BasicPublishOptions::default(),
            payload,
            lapin::BasicProperties::default().with_delivery_mode(2),
        )
        .await
        .expect("fixture basic_publish must be accepted");
    timeout(RPC_BOUND, confirm)
        .await
        .expect("publish confirm must arrive within the bound")
        .expect("publish confirm must be delivered");
}

/// Bounded poll until the manager publishes a generation newer than `before`.
///
/// No sleeps: the loop yields between reads, and the whole poll is bounded.
/// `current_generation()` may briefly read `0` while a reconnect writer holds
/// the state lock, so the predicate is `> before`, not `== before + 1`.
async fn wait_for_newer_generation(manager: &Arc<RabbitConnectionManager>, before: u64) -> u64 {
    timeout(RECV_BOUND, async {
        loop {
            let current = manager.current_generation();
            if current > before {
                return current;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the manager must reconnect within the bound")
}

/// Bounded capture of the manager's live `(connection, generation)` pair, used
/// to prove the fresh message rode the same reconnected connection.
async fn live_connection(manager: &Arc<RabbitConnectionManager>) -> (Arc<lapin::Connection>, u64) {
    timeout(
        Duration::from_secs(5),
        manager.connection_within(Duration::from_secs(2)),
    )
    .await
    .expect("the connection check must return within the bound")
    .expect("the reconnected connection must be live")
}

/// A delivery held mid-route across a broker restart is acked with a stale tag
/// after reconnect: the stale ack must be dropped (no protocol error closes the
/// reconnected channel), the broker redelivers the still-unacked message, and a
/// fresh message is consumed on the same reconnected connection.
#[tokio::test]
async fn stale_tag_after_broker_restart() {
    let _serial = tokio::time::timeout(Duration::from_secs(240), RECONNECT_TESTS.lock())
        .await
        .expect("the reconnect-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let capture = common::log_capture();
    let snapshot = capture.snapshot_len();

    let queue = unique("stale-tag");
    let (_connection, channel) = fx.declare_queue(&queue).await;
    publish(&channel, &queue, b"stale-tag-payload").await;

    let manager = manager_for(&fx.amqp_url);
    let config = RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue}"))
        .expect("stale-tag URI must parse");
    let (mut consumer, mut rx, _cancel) =
        start_consumer(config, Arc::clone(&manager), "stale-tag-route").await;

    // Hold one envelope mid-route; its delivery tag belongs to the pre-restart
    // connection generation.
    let held = recv_envelope(&mut rx).await;
    assert_payload(&held.exchange, b"stale-tag-payload");
    let generation_before = manager.current_generation();

    // Broker restart drops the connection; the manager reconnects while the
    // route is still held, issuing a new generation.
    fx.restart();
    wait_for_newer_generation(&manager, generation_before).await;
    // Capture the reconnected connection identity: the fresh message below must
    // be consumed on THIS connection, not a further reconnect.
    let (reconnected_connection, reconnected_generation) = live_connection(&manager).await;

    // Answer the pre-restart delivery AFTER reconnect: its tag is stale, so the
    // ack must be dropped rather than sent on the new channel.
    answer_ok(held);

    // The broker redelivers the still-unacked message on the reconnected
    // consumer (at-least-once).
    let redelivered = recv_envelope(&mut rx).await;
    assert_payload(&redelivered.exchange, b"stale-tag-payload");
    assert_eq!(
        redelivered
            .exchange
            .input
            .headers
            .get("rabbitmq.redelivered"),
        Some(&serde_json::json!(true)),
        "the redelivered message must carry rabbitmq.redelivered=true"
    );
    // The stale tag was actually dropped: the engine logged the debug marker
    // when it declined to ack the pre-restart delivery. Scoped to this test's
    // snapshot window; the serialization lock keeps sibling tests out of it.
    assert!(
        capture.count_containing_since(snapshot, &["dropping stale delivery tag after reconnect"])
            >= 1,
        "the engine must drop the stale delivery tag after reconnect"
    );
    answer_ok(redelivered);

    // A valid message consumed on the SAME reconnected connection proves the
    // stale tag was not applied to it (an applied stale tag would have closed
    // that channel). The original fixture channel died with the broker, so use
    // a fresh one.
    let (_connection2, channel2) = fx.declare_queue(&queue).await;
    publish(&channel2, &queue, b"fresh-after-reconnect").await;
    let fresh = recv_envelope(&mut rx).await;
    assert_payload(&fresh.exchange, b"fresh-after-reconnect");
    answer_ok(fresh);

    stop_consumer(&mut consumer).await;

    // The fresh message rode the same reconnected connection: identical Arc
    // identity and generation (not merely a "healthy manager").
    let (final_connection, final_generation) = live_connection(&manager).await;
    assert!(
        Arc::ptr_eq(&reconnected_connection, &final_connection),
        "the fresh message must be consumed on the same reconnected connection"
    );
    assert_eq!(
        final_generation, reconnected_generation,
        "no extra reconnect may replace the connection while consuming the fresh message"
    );

    assert_eq!(
        capture.count_containing_since(snapshot, &["PRECONDITION_FAILED", "UNKNOWN_DELIVERY_TAG"]),
        0,
        "no AMQP protocol error may be observed across the broker restart"
    );
}

/// A consumer stopped with one delivery held mid-route leaves it unacked; the
/// broker requeues it and a restarted consumer processes the same payload
/// again.
#[tokio::test]
async fn redelivery_after_mid_flight_stop() {
    let _serial = tokio::time::timeout(Duration::from_secs(240), RECONNECT_TESTS.lock())
        .await
        .expect("the reconnect-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let queue = unique("mid-flight-stop");
    let (_connection, channel) = fx.declare_queue(&queue).await;
    publish(&channel, &queue, b"mid-flight-payload").await;

    let manager = manager_for(&fx.amqp_url);
    let config = RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue}"))
        .expect("mid-flight URI must parse");
    let (mut consumer, mut rx, _cancel) =
        start_consumer(config.clone(), Arc::clone(&manager), "mid-flight-route").await;

    let held = recv_envelope(&mut rx).await;
    assert_payload(&held.exchange, b"mid-flight-payload");

    // Stop without answering: the in-flight delivery is abandoned (no
    // disposition) and closing the consumer channel requeues it.
    stop_consumer(&mut consumer).await;
    // A late Ok for the abandoned delivery must not panic.
    answer_ok(held);

    // Restart: the broker redelivers the same payload.
    let (mut consumer2, mut rx2, _cancel2) =
        start_consumer(config, Arc::clone(&manager), "mid-flight-route-2").await;
    let redelivered = recv_envelope(&mut rx2).await;
    assert_payload(&redelivered.exchange, b"mid-flight-payload");
    assert_eq!(
        redelivered
            .exchange
            .input
            .headers
            .get("rabbitmq.redelivered"),
        Some(&serde_json::json!(true)),
        "a requeued delivery must carry rabbitmq.redelivered=true"
    );
    answer_ok(redelivered);
    stop_consumer(&mut consumer2).await;
}

/// A broker restart issues exactly one new connection generation: the engine
/// must not tear down the manager's already-reconnected connection (which would
/// bump the generation again).
#[tokio::test]
async fn generation_increments_after_broker_restart() {
    let _serial = tokio::time::timeout(Duration::from_secs(240), RECONNECT_TESTS.lock())
        .await
        .expect("the reconnect-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let queue = unique("generation");
    let (_connection, _channel) = fx.declare_queue(&queue).await;

    let manager = manager_for(&fx.amqp_url);
    let config = RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue}"))
        .expect("generation URI must parse");
    let (mut consumer, mut rx, _cancel) =
        start_consumer(config, Arc::clone(&manager), "generation-route").await;

    let before = manager.current_generation();
    assert!(
        before > 0,
        "a started consumer must have a live connection generation"
    );

    fx.restart();

    // A fresh marker published after the restart is delivered only once the
    // reconnected consumer has taken a fresh channel and re-registered.
    let (_connection2, channel2) = fx.declare_queue(&queue).await;
    publish(&channel2, &queue, b"generation-marker").await;
    let marker = recv_envelope(&mut rx).await;
    assert_payload(&marker.exchange, b"generation-marker");
    answer_ok(marker);

    let after = manager.current_generation();
    assert_eq!(
        after,
        before + 1,
        "a broker restart must issue exactly one new generation"
    );

    stop_consumer(&mut consumer).await;
}

/// A channel-local consumer cancel (a deleted queue → `basic.cancel`) must not
/// tear down the shared connection: the manager keeps its connection and
/// generation, and a sibling consumer on that same connection keeps consuming
/// past its prefetch window.
///
/// This is the isolation regression for the defect where every `StreamEnded`
/// demoted the still-healthy connection, bumping the generation and stranding
/// every sibling engine's delivery acks as stale (prefetch deadlock).
#[tokio::test]
async fn channel_local_cancel_preserves_sibling_consumption() {
    let _serial = tokio::time::timeout(Duration::from_secs(240), RECONNECT_TESTS.lock())
        .await
        .expect("the reconnect-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let capture = common::log_capture();

    let queue_a = unique("local-cancel-a");
    let queue_b = unique("local-cancel-b");
    let (_conn_a, channel_a) = fx.declare_queue(&queue_a).await;
    let (_conn_b, channel_b) = fx.declare_queue(&queue_b).await;

    // Two independent consumers on ONE shared manager (and thus one connection).
    let manager = manager_for(&fx.amqp_url);
    let config_a =
        RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue_a}&prefetch=1"))
            .expect("local-cancel A URI must parse");
    let config_b =
        RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue_b}&prefetch=1"))
            .expect("local-cancel B URI must parse");
    let (mut consumer_a, mut rx_a, _cancel_a) =
        start_consumer(config_a, Arc::clone(&manager), "local-cancel-a-route").await;
    let (mut consumer_b, mut rx_b, _cancel_b) =
        start_consumer(config_b, Arc::clone(&manager), "local-cancel-b-route").await;

    let before_gen = manager.current_generation();
    let (before_conn, _) = live_connection(&manager).await;
    let snapshot = capture.snapshot_len();

    // Delete queue A: the broker sends `basic.cancel` to consumer A on its still
    // connected channel. This is a LOCAL termination — the shared connection
    // stays healthy and MUST NOT be torn down.
    channel_a
        .queue_delete(
            ShortString::from(queue_a.as_str()),
            QueueDeleteOptions::default(),
        )
        .await
        .expect("fixture queue_delete must be accepted");

    // Await the engine's explicit local-termination marker (or, on the defect, a
    // generation bump) before probing the sibling, so the assertion is not
    // vacuous.
    let local_termination = timeout(RECV_BOUND, async {
        loop {
            if capture.count_containing_since(snapshot, &["RabbitMQ consumer stream ended"]) >= 1 {
                return true;
            }
            if manager.current_generation() != before_gen {
                return false;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("engine A must observe the local cancel within the bound");
    assert!(
        local_termination,
        "a local consumer cancel must not reconnect the shared manager (generation changed)"
    );
    assert_eq!(
        manager.current_generation(),
        before_gen,
        "a local consumer cancel must not bump the shared generation"
    );

    // Recreate queue A so its engine's local 404 retry loop re-subscribes on the
    // SAME connection/generation; a publish then proves the local re-open path.
    let (_conn_a2, channel_a2) = fx.declare_queue(&queue_a).await;
    publish(&channel_a2, &queue_a, b"a-after-local-cancel").await;
    let a_msg = recv_envelope(&mut rx_a).await;
    assert_payload(&a_msg.exchange, b"a-after-local-cancel");
    answer_ok(a_msg);

    // Sibling B must keep consuming past its prefetch=1 window. Had the local
    // cancel bumped the generation, B's acks would be stale and dropped, so B
    // would stall after the first unacknowledged delivery.
    for marker in [&b"b-1"[..], &b"b-2"[..], &b"b-3"[..]] {
        publish(&channel_b, &queue_b, marker).await;
        let envelope = recv_envelope(&mut rx_b).await;
        assert_payload(&envelope.exchange, marker);
        answer_ok(envelope);
    }

    // The shared connection/generation is exactly the one the sibling consumed
    // on: same Arc identity, same generation.
    let (after_conn, after_gen) = live_connection(&manager).await;
    assert_eq!(
        after_gen, before_gen,
        "the shared generation must be untouched by a local sibling cancel"
    );
    assert!(
        Arc::ptr_eq(&before_conn, &after_conn),
        "the shared connection must not be replaced by a local sibling cancel"
    );

    stop_consumer(&mut consumer_a).await;
    stop_consumer(&mut consumer_b).await;

    assert_eq!(
        capture.count_containing_since(snapshot, &["PRECONDITION_FAILED", "UNKNOWN_DELIVERY_TAG"]),
        0,
        "no AMQP protocol error may be observed across a local cancel"
    );
}
