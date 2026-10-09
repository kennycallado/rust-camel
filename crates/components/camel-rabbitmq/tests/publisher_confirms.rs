//! Task 3.1 — publisher-confirm bound and fast-path integration tests.
//!
//! Broker-dependent tests are gated by `RABBITMQ_ITEST=1` (see
//! `tests/common/mod.rs`): unset prints a notice and returns early, while
//! `RABBITMQ_ITEST=1` with no docker panics `infra-unavailable` (no silent
//! skip). The confirm bound is exercised by `docker pause`-ing the broker:
//! the socket stays open so the publish write fits, but the confirm never
//! arrives, so the configured `confirmTimeout` is what fails the exchange.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use camel_api::{Body, BoxProcessor, Exchange, Message};
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{Component, NoOpComponentContext, ProducerContext, RuntimeObservability};
use camel_component_rabbitmq::RabbitMqComponent;
use camel_component_rabbitmq::config::{RabbitBrokerConfig, RabbitComponentConfig};
use lapin::ExchangeKind;
use lapin::options::{BasicGetOptions, ExchangeDeclareOptions};
use lapin::types::{FieldTable, ShortString};
use tower::Service;

use common::{nanos, require_fixture};

/// Declare a named exchange with NO bindings on a raw fixture connection, so a
/// publish to it is unroutable and the broker exercises `basic.return` (a
/// missing exchange would instead close the channel with a 404, which is a
/// different failure path). The caller keeps both handles alive.
async fn declare_unbound_exchange(url: &str, name: &str) -> (lapin::Connection, lapin::Channel) {
    let connection = lapin::Connection::connect(url, lapin::ConnectionProperties::default())
        .await
        .expect("fixture connection for exchange declare");
    let channel = connection
        .create_channel()
        .await
        .expect("fixture channel for exchange declare");
    channel
        .exchange_declare(
            ShortString::from(name),
            ExchangeKind::Direct,
            ExchangeDeclareOptions {
                durable: false,
                auto_delete: true,
                ..ExchangeDeclareOptions::default()
            },
            FieldTable::default(),
        )
        .await
        .expect("fixture exchange declare");
    (connection, channel)
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

/// Publish to the default exchange and read it back with raw lapin
/// `basic_get`: the normal confirm path resolves promptly and the payload
/// lands on the queue.
#[tokio::test]
async fn confirm_success_fast_path() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let queue = unique("confirm-ok");
    let (_connection, channel) = fx.declare_queue(&queue).await;

    let component = component_for(&fx.amqp_url);
    let endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}&confirmTimeout=1000"),
            &NoOpComponentContext,
        )
        .expect("endpoint creation against the fixture must succeed");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut producer = endpoint
        .create_producer(rt, &ProducerContext::default())
        .expect("producer creation must succeed");

    let start = Instant::now();
    producer
        .call(Exchange::new(Message::new(Body::Text("fast".to_string()))))
        .await
        .expect("a normal publish must be confirmed");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "the confirm must resolve promptly, took {elapsed:?}"
    );

    let got = channel
        .basic_get(
            ShortString::from(queue.as_str()),
            BasicGetOptions { no_ack: true },
        )
        .await
        .expect("basic_get must not error")
        .expect("the confirmed message must be on the queue");
    assert_eq!(got.data, b"fast", "raw payload must round-trip");
}

/// Gate scenario "confirm timeout fails the exchange": with the broker paused
/// a publish's confirm never arrives, so the exchange fails at the configured
/// `confirmTimeout` (not a disconnected-connection error), naming the target
/// and the confirm wait.
#[tokio::test]
async fn confirm_timeout_fails_exchange() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let queue = unique("confirm-timeout");
    let (_connection, _channel) = fx.declare_queue(&queue).await;

    let component = component_for(&fx.amqp_url);
    let endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}&confirmTimeout=1000"),
            &NoOpComponentContext,
        )
        .expect("endpoint creation against the fixture must succeed");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut producer = endpoint
        .create_producer(rt, &ProducerContext::default())
        .expect("producer creation must succeed");

    // Warm up: the publisher channel (and its confirm registration) is
    // allocated lazily on the first publish. It MUST exist before the broker
    // stalls, otherwise the second publish fails on the disconnected-
    // connection wait instead of exercising the confirm bound.
    producer
        .call(Exchange::new(Message::new(Body::Text(
            "warmup".to_string(),
        ))))
        .await
        .expect("the warmup publish must succeed and allocate the confirm channel");

    fx.pause();
    let start = Instant::now();
    let result = producer
        .call(Exchange::new(Message::new(Body::Text(
            "stalled".to_string(),
        ))))
        .await;
    let elapsed = start.elapsed();
    // Always release the freeze before asserting so a failed assertion still
    // leaves the broker usable; `Drop` also retries best-effort (task 3.1).
    fx.unpause();

    let error = result.expect_err("a stalled confirm must fail the exchange");
    let message = error.to_string();
    assert!(
        message.contains("confirm"),
        "the error must name the confirm wait, got: {message}"
    );
    assert!(
        message.contains("exchange"),
        "the error must name the exchange, got: {message}"
    );
    assert!(
        message.contains(&queue),
        "the error must name the routing key target, got: {message}"
    );
    assert!(
        elapsed >= Duration::from_millis(900),
        "the publish must actually wait the confirm bound, took {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "the publish must fail inside the bounded window, took {elapsed:?}"
    );
}

/// Regression (task 3.1 review fix): concurrent confirm timeouts must complete
/// within their bound even while a third cloned publish prepares a FRESH
/// channel on a paused broker.
///
/// The channel cache is a plain `std::sync::Mutex` whose critical sections are
/// tiny synchronous snapshots/conditional updates — the guard is never held
/// across `.await`, so the fresh `channel.open` + `confirm.select` run with the
/// lock released. The old `tokio::sync::Mutex` was held across those unbounded
/// awaits, so a stalled preparation wedged every concurrent timeout's cache
/// invalidation.
#[tokio::test]
async fn concurrent_confirm_timeouts_complete_within_bound() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let queue = unique("confirm-concurrent");
    let (_connection, _channel) = fx.declare_queue(&queue).await;

    let component = component_for(&fx.amqp_url);
    let endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}&confirmTimeout=1000"),
            &NoOpComponentContext,
        )
        .expect("endpoint creation against the fixture must succeed");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut producer = endpoint
        .create_producer(rt, &ProducerContext::default())
        .expect("producer creation must succeed");

    // Warm up so the confirm channel is cached before the broker stalls.
    producer
        .call(Exchange::new(Message::new(Body::Text(
            "warmup".to_string(),
        ))))
        .await
        .expect("warmup publish must succeed and cache the confirm channel");

    fx.pause();

    // Inner per-call bound for the publish itself. The join at each call site
    // is bounded separately below — that is the bounded wait CALLSITE, per
    // ADR-0069 §13.2; this bound only guards the inner publish.
    const CALL_BOUND: Duration = Duration::from_secs(6);
    // Join-site bound: CALL_BOUND (6 s) + close allowance (2 s).
    const JOIN_BOUND: Duration = Duration::from_secs(8);
    // Bound for reaping an aborted task (cancellation completes promptly, but
    // not synchronously).
    const REAP_BOUND: Duration = Duration::from_secs(2);
    // A confirm-timeout publish returns in confirmTimeout (1 s) + bounded
    // cleanup (1 s) ~ 2 s. Slack for CI scheduling, still far below the 6 s
    // deadlock signature.
    const TIMEOUT_PLUS_CLEANUP: Duration = Duration::from_millis(3_000);

    let publish_once = |producer: BoxProcessor| async move {
        let mut producer = producer;
        let start = Instant::now();
        let result = tokio::time::timeout(
            CALL_BOUND,
            producer.call(Exchange::new(Message::new(Body::Text(
                "stalled".to_string(),
            )))),
        )
        .await;
        (start.elapsed(), result)
    };

    // Bounded ticker (never an unbounded settling sleep): A at ~0 ms, B at
    // ~600 ms (A's confirm still pending), C at ~1200 ms — after A's confirm
    // timeout has invalidated the cache while B's old confirm is still pending,
    // so C must prepare a fresh channel on the paused broker.
    let mut ticker = tokio::time::interval(Duration::from_millis(600));
    ticker.tick().await;
    let a = tokio::spawn(publish_once(producer.clone()));
    ticker.tick().await;
    let b = tokio::spawn(publish_once(producer.clone()));
    ticker.tick().await;
    let c = tokio::spawn(publish_once(producer.clone()));

    let (elapsed_a, result_a) = tokio::time::timeout(JOIN_BOUND, a)
        .await
        .expect("task A must complete within JOIN_BOUND")
        .expect("task A must not panic");
    let (elapsed_b, result_b) = tokio::time::timeout(JOIN_BOUND, b)
        .await
        .expect("task B must complete within JOIN_BOUND")
        .expect("task B must not panic");

    // C is deliberately still stalled preparing a channel on the paused broker
    // while A and B completed — proof the fresh creation path never held the
    // cache lock. Abort it before teardown.
    assert!(
        !c.is_finished(),
        "C must still be stalled preparing a channel when A/B complete"
    );
    c.abort();
    let c_reaped = tokio::time::timeout(REAP_BOUND, c).await;
    let c_cancelled = matches!(&c_reaped, Ok(Err(error)) if error.is_cancelled());
    assert!(
        c_cancelled,
        "aborted task C must be reaped as cancelled within REAP_BOUND: {c_reaped:?}"
    );
    fx.unpause();

    let a_completed = result_a.as_ref().is_ok_and(|inner| inner.is_err());
    assert!(
        a_completed,
        "A's confirm timeout must complete, not be blocked by C: {result_a:?}"
    );
    assert!(
        elapsed_a < TIMEOUT_PLUS_CLEANUP,
        "A must complete within confirmTimeout + cleanup, took {elapsed_a:?}"
    );

    let b_completed = result_b.as_ref().is_ok_and(|inner| inner.is_err());
    assert!(
        b_completed,
        "B's confirm timeout must complete while C prepares a channel: {result_b:?}"
    );
    assert!(
        elapsed_b < TIMEOUT_PLUS_CLEANUP,
        "B must complete within confirmTimeout + cleanup, took {elapsed_b:?}"
    );
}

/// Gate scenario "unroutable mandatory publish fails": publishing mandatory to
/// a routing key with no bound queue makes the broker return the message
/// (basic.return) instead of routing it. The returned message rides THIS
/// publish's confirm (`Confirmation::Ack(Some(..))` / `Nack(Some(..))`), so the
/// exchange fails naming the exchange and routing key. The declared-but-unbound
/// exchange proves the failure comes from the return path, not a 404 on a
/// missing exchange.
#[tokio::test]
async fn unroutable_mandatory_fails() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let exchange = unique("unroutable-ex");
    let routing_key = unique("unroutable-rk");
    let (_connection, _channel) = declare_unbound_exchange(&fx.amqp_url, &exchange).await;

    let component = component_for(&fx.amqp_url);
    let endpoint = component
        .create_endpoint(
            &format!(
                "rabbitmq:{exchange}?routingKey={routing_key}&mandatory=true&confirmTimeout=1000"
            ),
            &NoOpComponentContext,
        )
        .expect("endpoint creation against the fixture must succeed");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut producer = endpoint
        .create_producer(rt, &ProducerContext::default())
        .expect("producer creation must succeed");

    let error = producer
        .call(Exchange::new(Message::new(Body::Text("lost".to_string()))))
        .await
        .expect_err("a mandatory publish to an unbound exchange must fail the exchange");
    let message = error.to_string();
    assert!(
        message.contains(&exchange),
        "the error must name the exchange, got: {message}"
    );
    assert!(
        message.contains(&routing_key),
        "the error must name the routing key, got: {message}"
    );
    assert!(
        message.contains("unroutable"),
        "the error must come from the broker basic.return (unroutable), not a 404 channel error, got: {message}"
    );
}

/// `mandatory=false` (the default) drops the unroutable message silently: the
/// broker confirms the publish even though no queue was bound, so the exchange
/// is `Ok`. Documents the default.
#[tokio::test]
async fn mandatory_off_drops_silently() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let exchange = unique("drop-ex");
    let routing_key = unique("drop-rk");
    let (_connection, _channel) = declare_unbound_exchange(&fx.amqp_url, &exchange).await;

    let component = component_for(&fx.amqp_url);
    let endpoint = component
        .create_endpoint(
            &format!("rabbitmq:{exchange}?routingKey={routing_key}&confirmTimeout=1000"),
            &NoOpComponentContext,
        )
        .expect("endpoint creation against the fixture must succeed");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut producer = endpoint
        .create_producer(rt, &ProducerContext::default())
        .expect("producer creation must succeed");

    producer
        .call(Exchange::new(Message::new(Body::Text(
            "dropped".to_string(),
        ))))
        .await
        .expect("mandatory=false must drop an unroutable message and confirm Ok");
}
