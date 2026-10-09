//! Task 1.6 — RabbitMQ docker-tier publish integration tests.
//!
//! Broker-dependent tests are gated by `RABBITMQ_ITEST=1` (see
//! `tests/common/mod.rs`): unset prints a notice and returns early, while
//! `RABBITMQ_ITEST=1` with no docker panics `infra-unavailable` (no silent
//! skip). The gate itself is pure and unit-tested here.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, Exchange, Message};
use camel_component_api::test_support::{NoopRuntimeObservability, RecordingRuntimeObservability};
use camel_component_api::{
    Component, Consumer, ConsumerContext, Endpoint, ExchangeEnvelope, NoOpComponentContext,
    ProducerContext, RuntimeObservability, StartupSignal,
};
use camel_component_rabbitmq::RabbitMqComponent;
use camel_component_rabbitmq::config::{RabbitBrokerConfig, RabbitComponentConfig};
use lapin::options::BasicGetOptions;
use lapin::types::{AMQPValue, ShortString};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tower::Service;

use common::{Gate, gate, nanos, require_fixture};

/// Deadline for consumer start/stop and broker round trips.
const RPC_BOUND: Duration = Duration::from_secs(30);
/// Deadline for a delivery the test is actively waiting for.
const RECV_BOUND: Duration = Duration::from_secs(20);

/// Start a real consumer over `endpoint` and return it with the receiver its
/// engine feeds (same harness shape as `consume_delivery.rs`).
async fn start_consumer(
    endpoint: &dyn Endpoint,
    route_id: &str,
) -> (
    Box<dyn Consumer>,
    mpsc::Receiver<ExchangeEnvelope>,
    CancellationToken,
) {
    let (tx, rx) = mpsc::channel(4);
    let cancel = CancellationToken::new();
    let (signal, receiver) = StartupSignal::pair();
    let ctx = ConsumerContext::new(tx, cancel.clone(), route_id.to_string()).with_startup(signal);
    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut consumer = endpoint
        .create_consumer(rt)
        .expect("consumer creation must succeed");
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

/// Answer a held envelope `Ok` and stop the consumer, bounded.
async fn answer_ok_and_stop(consumer: &mut Box<dyn Consumer>, envelope: ExchangeEnvelope) {
    if let Some(reply_tx) = envelope.reply_tx {
        let _ = reply_tx.send(Ok(envelope.exchange));
    }
    timeout(RPC_BOUND, consumer.stop())
        .await
        .expect("consumer.stop must return within the bound")
        .expect("consumer.stop must succeed");
}

/// Unique-per-process suffix so parallel tests and re-runs never collide.
fn unique(tag: &str) -> String {
    format!("{tag}-{}-{}", std::process::id(), nanos())
}

/// One-broker component pointed at the fixture URL.
fn component_for(url: &str) -> RabbitMqComponent {
    let mut brokers = HashMap::new();
    brokers.insert(
        "fixture".to_string(),
        RabbitBrokerConfig {
            url: url.to_string(),
            username: None,
            password: None,
            vhost: None,
        },
    );
    RabbitMqComponent::new(RabbitComponentConfig {
        brokers,
        reconnect: None,
    })
}

fn recorded_successes(rt: &RecordingRuntimeObservability) -> usize {
    rt.ops()
        .iter()
        .filter(|(component, operation, outcome)| {
            component == "rabbitmq" && operation == "publish" && outcome == "success"
        })
        .count()
}

/// Publish to the default exchange, then read the message back with raw lapin
/// `basic_get`: payload, free-form header and persistent delivery mode all
/// round-trip, and exactly one success publish is recorded.
#[tokio::test]
async fn publish_then_basic_get_round_trip() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let queue = unique("roundtrip");
    // `_connection` stays bound for the whole test (drop would close the AMQP
    // socket the returned channel rides on).
    let (_connection, channel) = fx.declare_queue(&queue).await;

    let component = component_for(&fx.amqp_url);
    let endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}"),
            &NoOpComponentContext,
        )
        .expect("endpoint creation against the fixture must succeed");

    let rt = RecordingRuntimeObservability::new(true);
    let rt_dyn: Arc<dyn RuntimeObservability> = rt.clone();
    let mut producer = endpoint
        .create_producer(rt_dyn, &ProducerContext::default())
        .expect("producer creation must succeed");

    let mut message = Message::new(Body::Text("hello".to_string()));
    message.set_header("x-custom", serde_json::json!("abc"));
    producer
        .call(Exchange::new(message))
        .await
        .expect("publish to the default exchange must succeed");

    let got = channel
        .basic_get(
            ShortString::from(queue.as_str()),
            BasicGetOptions { no_ack: true },
        )
        .await
        .expect("basic_get must not error")
        .expect("the published message must be on the queue");

    assert_eq!(got.data, b"hello", "raw payload must round-trip");
    assert_eq!(
        *got.properties.delivery_mode(),
        Some(2),
        "persistent default must set delivery mode 2"
    );
    let table = got
        .properties
        .headers()
        .as_ref()
        .expect("published message must carry a header table");
    match table.inner().get("x-custom") {
        Some(AMQPValue::LongString(value)) => {
            assert_eq!(
                value.as_bytes(),
                b"abc",
                "x-custom must round-trip as a LongString"
            );
        }
        other => panic!("expected LongString header x-custom, got {other:?}"),
    }

    // Task 2.4: extend this test with one consume cycle so the round trip is
    // asserted end to end (producer -> queue -> consumer). The first message
    // was consumed by `basic_get`, so publish a second copy for the cycle.
    let mut second = Message::new(Body::Text("hello".to_string()));
    second.set_header("x-custom", serde_json::json!("abc"));
    producer
        .call(Exchange::new(second))
        .await
        .expect("second publish must succeed");

    let (mut consumer, mut consumer_rx, _cancel) =
        start_consumer(&*endpoint, "roundtrip-consumer").await;
    let envelope = recv_envelope(&mut consumer_rx).await;
    assert_eq!(
        envelope.exchange.input.headers.get("x-custom"),
        Some(&serde_json::json!("abc")),
        "x-custom must survive a full producer->consumer cycle"
    );
    assert_eq!(
        envelope.exchange.input.headers.get("rabbitmq.redelivered"),
        Some(&serde_json::json!(false)),
        "rabbitmq.redelivered must be present on the consumed exchange"
    );
    answer_ok_and_stop(&mut consumer, envelope).await;

    assert_eq!(
        recorded_successes(&rt),
        2,
        "two success publishes must be recorded; ops: {:?}",
        rt.ops()
    );
}

/// Task 2.4 `header_round_trip_docker`: a real producer -> queue -> consumer
/// cycle carries the free-form header and the redelivered flag end to end.
#[tokio::test]
async fn header_round_trip_docker() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let queue = unique("header-roundtrip");
    let (_connection, _channel) = fx.declare_queue(&queue).await;

    let component = component_for(&fx.amqp_url);
    let endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}"),
            &NoOpComponentContext,
        )
        .expect("endpoint creation against the fixture must succeed");

    let rt = RecordingRuntimeObservability::new(true);
    let rt_dyn: Arc<dyn RuntimeObservability> = rt.clone();
    let mut producer = endpoint
        .create_producer(rt_dyn, &ProducerContext::default())
        .expect("producer creation must succeed");

    let mut message = Message::new(Body::Text("header-roundtrip".to_string()));
    message.set_header("x-custom", serde_json::json!("abc"));
    producer
        .call(Exchange::new(message))
        .await
        .expect("publish must succeed");

    let (mut consumer, mut consumer_rx, _cancel) =
        start_consumer(&*endpoint, "header-roundtrip-consumer").await;
    let envelope = recv_envelope(&mut consumer_rx).await;
    assert_eq!(
        envelope.exchange.input.headers.get("x-custom"),
        Some(&serde_json::json!("abc")),
        "x-custom must survive the full round trip"
    );
    assert_eq!(
        envelope.exchange.input.headers.get("rabbitmq.redelivered"),
        Some(&serde_json::json!(false)),
        "rabbitmq.redelivered must be present on the consumed exchange"
    );
    answer_ok_and_stop(&mut consumer, envelope).await;
}

/// Publishing to a non-existent exchange fails (404 surfaces through the
/// confirm wait) and records exactly one failure publish.
#[tokio::test]
async fn publish_to_missing_exchange_counts_failure() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let exchange = unique("ghost").replace('-', ".");
    let component = component_for(&fx.amqp_url);
    let endpoint = component
        .create_endpoint(
            &format!("rabbitmq:{exchange}?routingKey=k"),
            &NoOpComponentContext,
        )
        .expect("endpoint creation must succeed");

    let rt = RecordingRuntimeObservability::new(true);
    let rt_dyn: Arc<dyn RuntimeObservability> = rt.clone();
    let mut producer = endpoint
        .create_producer(rt_dyn, &ProducerContext::default())
        .expect("producer creation must succeed");

    let result = producer
        .call(Exchange::new(Message::new(Body::Text("lost".to_string()))))
        .await;
    assert!(
        result.is_err(),
        "publishing to a missing exchange must fail"
    );
    assert_eq!(
        rt.ops(),
        vec![(
            "rabbitmq".to_string(),
            "publish".to_string(),
            "failure".to_string()
        )],
        "exactly one failure publish must be recorded"
    );
}

/// The gate is binary and the notice names the activation variable.
#[test]
fn gate_rules_binary() {
    match gate(None, || true) {
        Gate::Skip(notice) => assert!(
            notice.contains("RABBITMQ_ITEST=1"),
            "skip notice must name RABBITMQ_ITEST=1, got: {notice}"
        ),
        Gate::Run => panic!("an unset RABBITMQ_ITEST must never run broker tests"),
    }
    assert!(
        matches!(gate(Some("1"), || true), Gate::Run),
        "an activated gate with a healthy docker probe must run"
    );
}

/// An activated gate without docker must panic `infra-unavailable`, never
/// silently skip. Pure closure: no host mutation.
#[test]
#[should_panic(expected = "infra-unavailable")]
fn gate_with_docker_missing_panics() {
    let _ = gate(Some("1"), || false);
}
