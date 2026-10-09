//! Task 3.3 — passive queue-exists check at consumer start, against a real
//! broker.
//!
//! A consumer start passively declares the configured queue on a dedicated
//! short-lived probe channel BEFORE any consume channel is created. A missing
//! queue fails fast, naming the queue, with the runtime startup signal left
//! FAILED (never ready); a pre-declared queue proceeds to ready. The probe 404
//! is channel-local, so the shared connection/generation is untouched and a
//! sibling consumer on the same manager still consumes.
//!
//! Broker-dependent tests are gated by `RABBITMQ_ITEST=1` (see
//! `tests/common/mod.rs`): unset prints a notice and returns early, while
//! `RABBITMQ_ITEST=1` with no docker panics `infra-unavailable` (no silent
//! skip). The topology tests share process-wide state, so they run under one
//! serialization lock.

mod common;

use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange};
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{
    Consumer, ConsumerContext, ExchangeEnvelope, NetworkRetryPolicy, StartupSignal,
};
use camel_component_rabbitmq::config::{RabbitBrokerConfig, RabbitEndpointConfig};
use camel_component_rabbitmq::{RabbitConnectionManager, RabbitConsumer};
use lapin::options::{
    BasicGetOptions, BasicPublishOptions, ExchangeDeclareOptions, QueueBindOptions,
    QueueDeclareOptions,
};
use lapin::types::{FieldTable, ShortString};
use lapin::{Channel, ExchangeKind};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

/// Deadline for consumer start/stop and broker round trips.
const RPC_BOUND: Duration = Duration::from_secs(30);
/// Deadline for a delivery the test is actively waiting for.
const RECV_BOUND: Duration = Duration::from_secs(20);

/// Serializes the topology tests for their whole body (shared process-wide
/// manager/sink state keeps attribution unambiguous).
static TOPOLOGY_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Unique-per-process suffix so parallel tests and re-runs never collide.
fn unique(tag: &str) -> String {
    format!("{tag}-{}-{}", std::process::id(), common::nanos())
}

/// Short, local-retry policy so the fixture URL connects within the test bound.
/// `max_attempts: 0` is unlimited (the component reconnect default).
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

fn config_for(queue: &str) -> RabbitEndpointConfig {
    RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue}"))
        .expect("topology URI must parse")
}

/// Bounded capture of the manager's live `(connection, generation)` pair.
async fn live_connection(manager: &Arc<RabbitConnectionManager>) -> (Arc<lapin::Connection>, u64) {
    timeout(
        Duration::from_secs(5),
        manager.connection_within(Duration::from_secs(2)),
    )
    .await
    .expect("the connection check must return within the bound")
    .expect("the broker connection must be live")
}

/// Start a real consumer that is expected to succeed and become ready.
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
        .expect("consumer.start must succeed for an existing queue");
    timeout(RPC_BOUND, receiver.await_ready())
        .await
        .expect("readiness must resolve within the bound")
        .expect("readiness must be Ok after a successful start");

    (consumer, rx, cancel)
}

/// Stop a consumer and confirm the loop joined within the bound.
async fn stop_consumer(consumer: &mut RabbitConsumer) {
    timeout(RPC_BOUND, consumer.stop())
        .await
        .expect("consumer.stop must return within the bound")
        .expect("consumer.stop must succeed");
}

/// Receive one in-flight envelope, bounded.
async fn recv_envelope(rx: &mut mpsc::Receiver<ExchangeEnvelope>) -> ExchangeEnvelope {
    timeout(RECV_BOUND, rx.recv())
        .await
        .expect("a delivery must arrive within the bound")
        .expect("the delivery channel must stay open")
}

/// Answer a held envelope `Ok` with its own exchange.
fn answer_ok(envelope: ExchangeEnvelope) {
    if let Some(reply_tx) = envelope.reply_tx {
        let _ = reply_tx.send(Ok(envelope.exchange));
    }
}

/// Assert the exchange payload is exactly `expected` bytes.
fn assert_payload(exchange: &Exchange, expected: &[u8]) {
    match &exchange.input.body {
        Body::Bytes(bytes) => assert_eq!(bytes.as_ref(), expected, "unexpected payload"),
        other => panic!("expected Body::Bytes, got {other:?}"),
    }
}

/// Publish `payload` straight to `queue` via the default exchange, awaiting the
/// broker confirm. The whole publish + confirm is bounded at the call site, so
/// neither the initial `basic_publish` await nor the confirm wait can park the
/// test.
async fn publish(channel: &Channel, queue: &str, payload: &[u8]) {
    timeout(RPC_BOUND, async {
        channel
            .basic_publish(
                ShortString::default(),
                ShortString::from(queue),
                BasicPublishOptions::default(),
                payload,
                lapin::BasicProperties::default().with_delivery_mode(2),
            )
            .await
            .expect("fixture basic_publish must be accepted")
            .await
            .expect("publish confirm must be delivered");
    })
    .await
    .expect("publish + confirm must complete within the bound");
}

/// Publish `payload` to a NAMED `exchange` under `routing_key`, awaiting the
/// broker confirm. The whole publish + confirm is bounded at the call site, so
/// neither the initial `basic_publish` await nor the confirm wait can park the
/// test.
async fn publish_to_exchange(channel: &Channel, exchange: &str, routing_key: &str, payload: &[u8]) {
    timeout(RPC_BOUND, async {
        channel
            .basic_publish(
                ShortString::from(exchange),
                ShortString::from(routing_key),
                BasicPublishOptions::default(),
                payload,
                lapin::BasicProperties::default().with_delivery_mode(2),
            )
            .await
            .expect("fixture basic_publish must be accepted")
            .await
            .expect("publish confirm must be delivered");
    })
    .await
    .expect("publish + confirm must complete within the bound");
}

/// A raw fixture connection + channel, used to observe or contradict the
/// topology the component declared. The connect is bounded, so the helper is
/// not an unbounded wait site.
async fn raw_channel(url: &str) -> (lapin::Connection, Channel) {
    let connection = timeout(
        RPC_BOUND,
        lapin::Connection::connect(url, lapin::ConnectionProperties::default()),
    )
    .await
    .expect("raw fixture connection must return within the bound")
    .expect("raw fixture connection must connect");
    let channel = timeout(RPC_BOUND, connection.create_channel())
        .await
        .expect("raw fixture channel must return within the bound")
        .expect("raw fixture channel must be created");
    (connection, channel)
}

/// Raw `exchange_declare` returning the broker verdict, for option
/// non-vacuity probes (a conflicting kind answers 406 PRECONDITION_FAILED).
async fn raw_declare_exchange(
    channel: &Channel,
    name: &str,
    kind: ExchangeKind,
    durable: bool,
) -> Result<(), lapin::Error> {
    timeout(
        RPC_BOUND,
        channel.exchange_declare(
            ShortString::from(name),
            kind,
            ExchangeDeclareOptions {
                durable,
                ..ExchangeDeclareOptions::default()
            },
            FieldTable::default(),
        ),
    )
    .await
    .expect("raw exchange_declare must return within the bound")
}

/// Raw `queue_declare` returning the broker verdict, for option non-vacuity
/// probes (a conflicting durable flag answers 406 PRECONDITION_FAILED).
async fn raw_declare_queue(
    channel: &Channel,
    name: &str,
    durable: bool,
) -> Result<(), lapin::Error> {
    timeout(
        RPC_BOUND,
        channel.queue_declare(
            ShortString::from(name),
            QueueDeclareOptions {
                durable,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        ),
    )
    .await
    .expect("raw queue_declare must return within the bound")
    .map(|_| ())
}

/// Answer a held envelope `Err`, failing the route so the broker dead-letters.
fn answer_err(envelope: ExchangeEnvelope, error: CamelError) {
    if let Some(reply_tx) = envelope.reply_tx {
        let _ = reply_tx.send(Err(error));
    }
}

/// One-shot pull (`no_ack=true`), bounded.
async fn basic_get(
    channel: &Channel,
    queue: &str,
    no_ack: bool,
) -> Option<lapin::message::BasicGetMessage> {
    timeout(
        RPC_BOUND,
        channel.basic_get(ShortString::from(queue), BasicGetOptions { no_ack }),
    )
    .await
    .expect("basic_get must return within the bound")
    .expect("basic_get must not error")
}

/// Wait (bounded) for a message to appear on `queue`, consuming it.
async fn wait_for_message(channel: &Channel, queue: &str) -> lapin::message::BasicGetMessage {
    timeout(RECV_BOUND, async {
        loop {
            if let Some(message) = basic_get(channel, queue, true).await {
                return message;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the message must arrive within the bound")
}

/// A consumer start against a queue that does not exist must fail fast naming
/// the queue, with the runtime startup signal FAILED and never ready. The probe
/// 404 is channel-local: the shared connection/generation is unchanged and a
/// sibling consumer on the same manager still consumes.
#[tokio::test]
async fn missing_queue_fails_fast() {
    let _serial = tokio::time::timeout(Duration::from_secs(180), TOPOLOGY_TESTS.lock())
        .await
        .expect("the topology-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };

    let missing = unique("topology-missing");
    let manager = manager_for(&fx.amqp_url);
    let (before_conn, before_gen) = live_connection(&manager).await;

    let (tx, _rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();
    let (signal, receiver) = StartupSignal::pair();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "missing-topology-route".to_string())
        .with_startup(signal);
    let mut consumer = RabbitConsumer::new(
        config_for(&missing),
        Arc::clone(&manager),
        Arc::new(NoopRuntimeObservability),
    );

    let start_result = timeout(RPC_BOUND, consumer.start(ctx))
        .await
        .expect("consumer.start must return within the bound");
    let error = start_result.expect_err("start must fail for a missing queue");
    assert!(
        error.to_string().contains(&missing),
        "the start error must name the missing queue: {error}"
    );
    // The missing queue fails on the dedicated passive PROBE, never on a
    // consume registration: the consumer must not `basic.consume` a queue it
    // has not verified exists.
    assert!(
        !error.to_string().contains("basic_consume"),
        "a missing queue must fail on the passive probe, not on basic_consume: {error}"
    );

    // The runtime startup signal resolves FAILED, never Ready.
    let ready = timeout(RPC_BOUND, receiver.await_ready())
        .await
        .expect("the startup receiver must resolve within the bound");
    let ready_error = ready.expect_err("the startup signal must be FAILED, not Ready");
    assert!(
        ready_error.to_string().contains(&missing),
        "the failed startup signal must name the missing queue: {ready_error}"
    );

    // Natural isolation: the probe 404 is channel-local, so the shared
    // connection and generation are unchanged.
    let (after_conn, after_gen) = live_connection(&manager).await;
    assert!(
        Arc::ptr_eq(&before_conn, &after_conn),
        "a probe 404 must not replace the shared connection"
    );
    assert_eq!(
        before_gen, after_gen,
        "a probe 404 must not advance the shared generation"
    );

    // A sibling consumer on a real queue of the SAME manager still consumes.
    let sibling_queue = unique("topology-sibling");
    let (_conn, channel) = fx.declare_queue(&sibling_queue).await;
    let (mut sibling, mut sibling_rx, sibling_cancel) = start_consumer(
        config_for(&sibling_queue),
        Arc::clone(&manager),
        "sibling-route",
    )
    .await;

    publish(&channel, &sibling_queue, b"after-missing").await;
    let envelope = recv_envelope(&mut sibling_rx).await;
    assert_payload(&envelope.exchange, b"after-missing");
    answer_ok(envelope);

    stop_consumer(&mut sibling).await;
    sibling_cancel.cancel();
}

/// A queue that exists at start passes the passive check and the consumer
/// reaches ready.
#[tokio::test]
async fn existing_queue_proceeds() {
    let _serial = tokio::time::timeout(Duration::from_secs(180), TOPOLOGY_TESTS.lock())
        .await
        .expect("the topology-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };

    let queue = unique("topology-existing");
    let (_conn, _channel) = fx.declare_queue(&queue).await;
    let manager = manager_for(&fx.amqp_url);

    let (mut consumer, _rx, cancel) =
        start_consumer(config_for(&queue), manager, "existing-topology-route").await;

    stop_consumer(&mut consumer).await;
    cancel.cancel();
}

/// `autoDeclare=true` with defaults actively declares a NAMED exchange, a
/// durable queue, and a binding, then consumes. The default exchange case is an
/// additional phase under this same function: the empty path must SKIP
/// `exchange_declare` and `queue_bind` (the broker answers 403 for the reserved
/// default exchange) while still declaring the queue.
#[tokio::test]
async fn autodeclare_creates_topology() {
    let _serial = tokio::time::timeout(Duration::from_secs(180), TOPOLOGY_TESTS.lock())
        .await
        .expect("the topology-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };

    let manager = manager_for(&fx.amqp_url);

    // Named-exchange scenario: queue absent, defaults (direct/durable).
    let exchange = unique("auto-exchange");
    let queue = unique("auto-queue");
    let config = RabbitEndpointConfig::from_uri(&format!(
        "rabbitmq:{exchange}?queue={queue}&routingKey={queue}&autoDeclare=true"
    ))
    .expect("autoDeclare URI must parse");

    let (mut consumer, mut rx, cancel) =
        start_consumer(config, Arc::clone(&manager), "auto-topology-route").await;

    let (_conn, channel) = raw_channel(&fx.amqp_url).await;

    // The active declare created a durable queue: a passive declare succeeds,
    // and a contradictory non-durable declare answers PRECONDITION (proves
    // `durableQueue` defaults to true).
    timeout(
        RPC_BOUND,
        channel.queue_declare(
            ShortString::from(queue.as_str()),
            QueueDeclareOptions {
                passive: true,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        ),
    )
    .await
    .expect("passive declare must return within the bound")
    .expect("the auto-declared queue must exist");

    // The binding routes: publish through the declared exchange and consume.
    publish_to_exchange(&channel, &exchange, &queue, b"auto-routed").await;
    let envelope = recv_envelope(&mut rx).await;
    assert_payload(&envelope.exchange, b"auto-routed");
    answer_ok(envelope);

    let conflict = raw_declare_queue(&channel, &queue, false).await;
    let error = conflict.expect_err("a non-durable redeclare must conflict with the durable queue");
    assert!(
        error.to_string().contains("PRECONDITION"),
        "the broker must answer PRECONDITION for a durable mismatch: {error}"
    );

    stop_consumer(&mut consumer).await;
    cancel.cancel();

    // Additional case: the default exchange ("") cannot be declared or bound;
    // autoDeclare must skip both and still declare the queue.
    let default_queue = unique("auto-default-queue");
    let default_config = RabbitEndpointConfig::from_uri(&format!(
        "rabbitmq:default?queue={default_queue}&autoDeclare=true"
    ))
    .expect("default-exchange autoDeclare URI must parse");
    let (mut default_consumer, mut default_rx, default_cancel) =
        start_consumer(default_config, Arc::clone(&manager), "auto-default-route").await;

    let (_conn2, channel2) = raw_channel(&fx.amqp_url).await;
    publish(&channel2, &default_queue, b"auto-default").await;
    let envelope = recv_envelope(&mut default_rx).await;
    assert_payload(&envelope.exchange, b"auto-default");
    answer_ok(envelope);

    stop_consumer(&mut default_consumer).await;
    default_cancel.cancel();
}

/// A pre-existing non-durable queue plus `autoDeclare=true` with the default
/// `durableQueue=true` fails route start carrying the broker's PRECONDITION
/// text. The probe 406 is channel-local: the startup signal is FAILED, the
/// shared connection/generation is unchanged, and a sibling consumer on the
/// same manager still consumes.
#[tokio::test]
async fn conflicting_declare_fails_start() {
    let _serial = tokio::time::timeout(Duration::from_secs(180), TOPOLOGY_TESTS.lock())
        .await
        .expect("the topology-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };

    let exchange = unique("conflict-exchange");
    let queue = unique("conflict-queue");
    let (_conn, channel) = raw_channel(&fx.amqp_url).await;
    raw_declare_queue(&channel, &queue, false)
        .await
        .expect("the non-durable pre-create must succeed");

    let manager = manager_for(&fx.amqp_url);
    let (before_conn, before_gen) = live_connection(&manager).await;

    let config = RabbitEndpointConfig::from_uri(&format!(
        "rabbitmq:{exchange}?queue={queue}&routingKey={queue}&autoDeclare=true"
    ))
    .expect("conflicting autoDeclare URI must parse");

    let (tx, _rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();
    let (signal, receiver) = StartupSignal::pair();
    let ctx =
        ConsumerContext::new(tx, cancel.clone(), "conflict-route".to_string()).with_startup(signal);
    let mut consumer = RabbitConsumer::new(
        config,
        Arc::clone(&manager),
        Arc::new(NoopRuntimeObservability),
    );

    let start_result = timeout(RPC_BOUND, consumer.start(ctx))
        .await
        .expect("consumer.start must return within the bound");
    let error = start_result.expect_err("a durable/non-durable conflict must fail start");
    assert!(
        error.to_string().contains("PRECONDITION"),
        "the start error must carry the broker's PRECONDITION text: {error}"
    );
    assert!(
        error.to_string().contains(&queue),
        "the start error must name the conflicting queue: {error}"
    );

    let ready = timeout(RPC_BOUND, receiver.await_ready())
        .await
        .expect("the startup receiver must resolve within the bound");
    let ready_error = ready.expect_err("the startup signal must be FAILED, not Ready");
    assert!(
        ready_error.to_string().contains("PRECONDITION"),
        "the failed startup signal must carry the broker's PRECONDITION text: {ready_error}"
    );

    // The probe 406 is channel-local: the shared connection/generation is
    // unchanged and a sibling consumer on the SAME manager still consumes.
    let (after_conn, after_gen) = live_connection(&manager).await;
    assert!(
        Arc::ptr_eq(&before_conn, &after_conn),
        "a conflicting declare must not replace the shared connection"
    );
    assert_eq!(
        before_gen, after_gen,
        "a conflicting declare must not advance the shared generation"
    );

    let sibling_queue = unique("conflict-sibling");
    let (_sconn, sibling_channel) = fx.declare_queue(&sibling_queue).await;
    let (mut sibling, mut sibling_rx, sibling_cancel) = start_consumer(
        config_for(&sibling_queue),
        Arc::clone(&manager),
        "conflict-sibling-route",
    )
    .await;

    publish(&sibling_channel, &sibling_queue, b"after-conflict").await;
    let envelope = recv_envelope(&mut sibling_rx).await;
    assert_payload(&envelope.exchange, b"after-conflict");
    answer_ok(envelope);

    stop_consumer(&mut sibling).await;
    sibling_cancel.cancel();
    cancel.cancel();
}

/// `exchangeType=fanout` is read: a raw `exchange_declare(kind=Direct)` on the
/// same name answers 406 PRECONDITION.
#[tokio::test]
async fn autodeclare_honors_exchange_type() {
    let _serial = tokio::time::timeout(Duration::from_secs(180), TOPOLOGY_TESTS.lock())
        .await
        .expect("the topology-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };

    let exchange = unique("type-exchange");
    let queue = unique("type-queue");
    let config = RabbitEndpointConfig::from_uri(&format!(
        "rabbitmq:{exchange}?queue={queue}&routingKey={queue}&autoDeclare=true&exchangeType=fanout"
    ))
    .expect("exchangeType URI must parse");

    let (mut consumer, _rx, cancel) =
        start_consumer(config, manager_for(&fx.amqp_url), "exchange-type-route").await;

    let (_conn, channel) = raw_channel(&fx.amqp_url).await;
    let error = raw_declare_exchange(&channel, &exchange, ExchangeKind::Direct, true)
        .await
        .expect_err("a direct redeclare must conflict with the fanout exchange");
    assert!(
        error.to_string().contains("PRECONDITION"),
        "the exchange kind conflict must be a PRECONDITION: {error}"
    );

    stop_consumer(&mut consumer).await;
    cancel.cancel();
}

/// `durableQueue=false` is read: a raw `queue_declare(durable=true)` on the
/// same queue answers 406 PRECONDITION.
#[tokio::test]
async fn autodeclare_honors_durable_false() {
    let _serial = tokio::time::timeout(Duration::from_secs(180), TOPOLOGY_TESTS.lock())
        .await
        .expect("the topology-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };

    let queue = unique("nondurable-queue");
    let config = RabbitEndpointConfig::from_uri(&format!(
        "rabbitmq:default?queue={queue}&autoDeclare=true&durableQueue=false"
    ))
    .expect("durableQueue URI must parse");

    let (mut consumer, _rx, cancel) =
        start_consumer(config, manager_for(&fx.amqp_url), "durable-false-route").await;

    let (_conn, channel) = raw_channel(&fx.amqp_url).await;
    let error = raw_declare_queue(&channel, &queue, true)
        .await
        .expect_err("a durable redeclare must conflict with the non-durable queue");
    assert!(
        error.to_string().contains("PRECONDITION"),
        "the durable conflict must be a PRECONDITION: {error}"
    );

    stop_consumer(&mut consumer).await;
    cancel.cancel();
}

/// `queueArguments={"x-dead-letter-exchange":"<dlx>"}` reaches the actively
/// declared queue: a failed route is rejected to the DLX and the payload lands
/// on the dead-letter queue (the 2.2 flow, with the argument created by 3.4).
#[tokio::test]
async fn queue_arguments_passthrough() {
    let _serial = tokio::time::timeout(Duration::from_secs(180), TOPOLOGY_TESTS.lock())
        .await
        .expect("the topology-test serialization lock must be acquired within the bound");
    let Some(fx) = common::require_fixture() else {
        return;
    };

    let exchange = unique("args-exchange");
    let main = unique("args-main");
    let routing_key = unique("args-key");
    let dlx = unique("args-dlx");
    let dlq = unique("args-dlq");

    let (_conn, channel) = raw_channel(&fx.amqp_url).await;
    raw_declare_exchange(&channel, &dlx, ExchangeKind::Direct, true)
        .await
        .expect("the DLX declare must succeed");
    raw_declare_queue(&channel, &dlq, true)
        .await
        .expect("the DLQ declare must succeed");
    timeout(
        RPC_BOUND,
        channel.queue_bind(
            ShortString::from(dlq.as_str()),
            ShortString::from(dlx.as_str()),
            ShortString::from(routing_key.as_str()),
            QueueBindOptions::default(),
            FieldTable::default(),
        ),
    )
    .await
    .expect("the DLQ bind must return within the bound")
    .expect("the DLQ bind must succeed");

    let config = RabbitEndpointConfig::from_uri(&format!(
        "rabbitmq:{exchange}?queue={main}&routingKey={routing_key}&autoDeclare=true&queueArguments={{\"x-dead-letter-exchange\":\"{dlx}\"}}"
    ))
    .expect("queueArguments URI must parse");

    let (mut consumer, mut rx, cancel) =
        start_consumer(config, manager_for(&fx.amqp_url), "args-route").await;

    publish_to_exchange(&channel, &exchange, &routing_key, b"poison").await;
    let envelope = recv_envelope(&mut rx).await;
    assert_payload(&envelope.exchange, b"poison");
    answer_err(
        envelope,
        CamelError::ProcessorError("route failed".to_string()),
    );

    let dead = wait_for_message(&channel, &dlq).await;
    assert_eq!(
        dead.data, b"poison",
        "the queue argument must have wired the dead-letter exchange"
    );

    stop_consumer(&mut consumer).await;
    cancel.cancel();
}
