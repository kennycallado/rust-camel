//! Task 4.4 — RabbitMQ request/reply docker integration tier.
//!
//! The live RPC path (InOut producer + consumer-side reply publishing) is
//! exercised against a real `rabbitmq:3.13-alpine` broker. Broker-dependent
//! tests are gated by `RABBITMQ_ITEST=1` (see `tests/common/mod.rs`): unset
//! prints the explicit notice and returns early, while `RABBITMQ_ITEST=1`
//! without docker panics `infra-unavailable` (never a silent skip).
//!
//! The four live scenarios are the exact plan names:
//! `reply_round_trip` (table over `mandatory=false|true`),
//! `reply_timeout_docker`, `late_reply_dropped_docker`, and
//! `failed_route_sends_no_reply_docker`. No `pub(crate)` correlation-table
//! seam is touched: the tests observe only the public component surface plus
//! the process-wide recording tracing sink already shared by the tier.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use camel_api::{Body, CamelError, Exchange, Message};
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{
    Component, Consumer, ConsumerContext, Endpoint, ExchangeEnvelope, NoOpComponentContext,
    ProducerContext, RuntimeObservability, StartupSignal,
};
use camel_component_rabbitmq::RabbitMqComponent;
use camel_component_rabbitmq::config::{RabbitBrokerConfig, RabbitComponentConfig};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tower::Service;

use common::{nanos, require_fixture};

/// Consumer start/stop and broker setup bound.
const START_BOUND: Duration = Duration::from_secs(30);
/// Outer bound on every RPC call site: the producer's own `replyTimeout` is
/// not sufficient (a stalled setup must still be reaped).
const OUTER_BOUND: Duration = Duration::from_secs(10);
/// Deadline for a delivery the test is actively waiting for.
const RECV_BOUND: Duration = Duration::from_secs(20);
/// Bounded abort/reap of a spawned replier task on the failure path.
const REAP_BOUND: Duration = Duration::from_secs(2);

/// Aborts the wrapped task on drop so a failing/panicking test never leaks an
/// orphan replier task (the broker container is torn down by the fixture).
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Start a real consumer over `endpoint` and return it with the envelope
/// receiver its engine feeds (same harness shape as `publish_roundtrip.rs`).
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
    timeout(START_BOUND, consumer.start(ctx))
        .await
        .expect("consumer.start must return within the bound")
        .expect("consumer.start must succeed against the fixture");
    timeout(START_BOUND, receiver.await_ready())
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

/// Stop a consumer, bounded.
async fn stop_consumer(consumer: &mut Box<dyn Consumer>) {
    timeout(START_BOUND, consumer.stop())
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

fn noop_rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoopRuntimeObservability)
}

/// Materialize a body as text for assertions (reply bodies are `Bytes`).
fn text_of(body: &Body) -> String {
    match body {
        Body::Text(text) => text.clone(),
        Body::Bytes(bytes) => String::from_utf8_lossy(bytes).to_string(),
        other => format!("{other:?}"),
    }
}

/// The inbound `correlationId` a replier observes on the request.
fn observed_correlation(env: &ExchangeEnvelope) -> String {
    env.exchange
        .input
        .headers
        .get("correlationId")
        .and_then(|value| value.as_str())
        .expect("an InOut request must carry a correlationId header")
        .to_string()
}

/// A route result carrying an OUT message with `body` and a free-form marker.
fn routed_reply(body: String, marker: &str) -> Exchange {
    let mut message = Message::new(Body::Text(body));
    message.set_header("x-which", serde_json::json!(marker));
    let mut routed = Exchange::new(Message::new(Body::Empty));
    routed.output = Some(message);
    routed
}

/// One live round trip: a real replier consumer echoes `Hello <body>` and the
/// requester's InOut call resolves with that reply.
async fn round_trip_variant(
    fx: &common::RabbitFixture,
    mandatory: bool,
    component: &RabbitMqComponent,
) {
    let queue = unique(&format!("rpc-roundtrip-m{mandatory}"));
    // Declare the queue so the request is a confirmed, routable publish.
    let (_conn, _channel) = fx.declare_queue(&queue).await;

    let replier_endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}"),
            &NoOpComponentContext,
        )
        .expect("replier endpoint creation must succeed");
    let requester_endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}&mandatory={mandatory}&replyTimeout=5000"),
            &NoOpComponentContext,
        )
        .expect("requester endpoint creation must succeed");

    let (mut consumer, rx, _cancel) = start_consumer(&*replier_endpoint, "roundtrip-replier").await;

    let observed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let observed_task = Arc::clone(&observed);
    let mut replier = AbortOnDrop(tokio::spawn(async move {
        let mut rx = rx;
        let env = recv_envelope(&mut rx).await;
        *observed_task.lock().expect("observed mutex") = Some(observed_correlation(&env));
        let reply = routed_reply(
            format!("Hello {}", text_of(&env.exchange.input.body)),
            "echo",
        );
        let _ = env
            .reply_tx
            .expect("the engine always attaches a reply sender")
            .send(Ok(reply));
    }));

    let mut producer = requester_endpoint
        .create_producer(noop_rt(), &ProducerContext::default())
        .expect("requester producer creation must succeed");

    let request = Exchange::new_in_out(Message::new(Body::Text("world".to_string())));
    let resolved = timeout(OUTER_BOUND, producer.call(request))
        .await
        .expect("the round trip must resolve within the outer bound")
        .expect("a live replier must resolve the InOut call");

    let output = resolved
        .output
        .as_ref()
        .expect("an InOut reply must populate the OUT message");
    assert_eq!(
        text_of(&output.body),
        "Hello world",
        "the requester must resolve with the replier's echoed body (mandatory={mandatory})"
    );
    assert_eq!(
        output.headers.get("x-which"),
        Some(&serde_json::json!("echo")),
        "the reply's free-form OUT header must survive to the requester"
    );
    let request_correlation = observed
        .lock()
        .expect("observed mutex")
        .clone()
        .expect("the replier must have observed the request correlation");
    assert_eq!(
        output
            .headers
            .get("correlationId")
            .and_then(|value| value.as_str()),
        Some(request_correlation.as_str()),
        "the reply must echo the ORIGINAL request correlation id"
    );

    timeout(REAP_BOUND, &mut replier.0)
        .await
        .expect("the replier task must finish within the reap bound")
        .expect("the replier task must not panic");
    stop_consumer(&mut consumer).await;
}

/// Scenario (1): a real requester producer and a real replier consumer
/// round-trip an InOut exchange; the reply echoes the original correlation id
/// and carries the route's OUT header. Table over `mandatory=false|true` so
/// the P4 mandatory reply lane is exercised live on the shared reply channel
/// (previously unit-only).
#[tokio::test]
async fn reply_round_trip() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let component = component_for(&fx.amqp_url);
    for mandatory in [false, true] {
        round_trip_variant(&fx, mandatory, &component).await;
    }
}

/// Scenario (2): with a declared queue but no replier, an InOut request fails
/// with a reply-timeout error within the configured bound. The queue is
/// declared so the request is a confirmed, routable publish (not an
/// unroutable/missing-exchange failure).
#[tokio::test]
async fn reply_timeout_docker() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let queue = unique("rpc-timeout");
    let (_conn, _channel) = fx.declare_queue(&queue).await;

    let component = component_for(&fx.amqp_url);
    let endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}&replyTimeout=2000"),
            &NoOpComponentContext,
        )
        .expect("endpoint creation must succeed");
    let mut producer = endpoint
        .create_producer(noop_rt(), &ProducerContext::default())
        .expect("producer creation must succeed");

    let started = Instant::now();
    let result = timeout(
        OUTER_BOUND,
        producer.call(Exchange::new_in_out(Message::new(Body::Text(
            "no-reply".to_string(),
        )))),
    )
    .await
    .expect("the request must fail within the outer bound");
    let elapsed = started.elapsed();

    let error = result.expect_err("a request with no replier must time out");
    eprintln!("reply_timeout_docker: elapsed={elapsed:?} error={error}");
    assert!(
        error.to_string().contains("reply"),
        "the failure must be the reply-timeout error, got: {error}"
    );
    assert!(
        elapsed >= Duration::from_secs(1),
        "the request must actually wait for the replyTimeout bound, waited {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "the reply timeout must fire within 3 s, waited {elapsed:?}"
    );
}

/// Scenario (3): a held first reply arrives AFTER its request timed out; a
/// follow-up InOut on the SAME producer must resolve with its own fresh reply,
/// never the late one. The replier task holds the first envelope until the
/// requester has errored, then releases it; a real tracing-marker barrier
/// proves the late reply was published and dropped before the follow-up is
/// sent (no arbitrary settling sleep).
#[tokio::test]
async fn late_reply_dropped_docker() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let queue = unique("rpc-late");
    let (_conn, _channel) = fx.declare_queue(&queue).await;

    let component = component_for(&fx.amqp_url);
    let replier_endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}"),
            &NoOpComponentContext,
        )
        .expect("replier endpoint creation must succeed");
    let requester_endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}&replyTimeout=1000"),
            &NoOpComponentContext,
        )
        .expect("requester endpoint creation must succeed");

    let (mut consumer, rx, _cancel) = start_consumer(&*replier_endpoint, "late-replier").await;

    let capture = common::log_capture();
    let snapshot = capture.snapshot_len();
    let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let observed_task = Arc::clone(&observed);
    let (release_tx, release_rx) = oneshot::channel::<()>();

    let mut replier = AbortOnDrop(tokio::spawn(async move {
        let mut rx = rx;
        // First request: record it, then HOLD the reply until released.
        let first = recv_envelope(&mut rx).await;
        observed_task
            .lock()
            .expect("observed mutex")
            .push(observed_correlation(&first));
        timeout(RECV_BOUND, release_rx)
            .await
            .expect("the release signal must arrive within the bound")
            .expect("the release sender must not be dropped");
        // An obviously different OLD body, released only after the timeout.
        let old = routed_reply("OLD-REPLY".to_string(), "old");
        let _ = first
            .reply_tx
            .expect("the engine always attaches a reply sender")
            .send(Ok(old));
        // Follow-up request: reply with ITS OWN fresh body immediately.
        let second = recv_envelope(&mut rx).await;
        observed_task
            .lock()
            .expect("observed mutex")
            .push(observed_correlation(&second));
        let fresh = routed_reply(
            format!("Hello {}", text_of(&second.exchange.input.body)),
            "new",
        );
        let _ = second
            .reply_tx
            .expect("the engine always attaches a reply sender")
            .send(Ok(fresh));
    }));

    let mut producer = requester_endpoint
        .create_producer(noop_rt(), &ProducerContext::default())
        .expect("requester producer creation must succeed");

    // First request times out while its reply is held.
    let first_started = Instant::now();
    let first = timeout(
        OUTER_BOUND,
        producer.call(Exchange::new_in_out(Message::new(Body::Text(
            "old".to_string(),
        )))),
    )
    .await
    .expect("the first request must fail within the outer bound");
    let first_elapsed = first_started.elapsed();
    let first_error = first.expect_err("the held first request must time out");
    assert!(
        first_error.to_string().contains("reply"),
        "the first request must fail with the reply-timeout error, got: {first_error}"
    );

    // Release the held OLD reply, then wait for the real drop marker: the reply
    // loop emits it once the late delivery is observed and dropped. This is the
    // deterministic barrier before the follow-up (no settling sleep).
    release_tx.send(()).expect("the release signal must send");
    let barrier_started = Instant::now();
    timeout(OUTER_BOUND, async {
        loop {
            if capture.count_containing_since(snapshot, &["rabbitmq late reply dropped"]) >= 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the late reply must be published and dropped within the bound");
    let barrier_elapsed = barrier_started.elapsed();

    // Follow-up on the SAME producer; the reply state must still be usable and
    // must correlate to the follow-up UUID, not the late first reply.
    let second = timeout(
        OUTER_BOUND,
        producer.call(Exchange::new_in_out(Message::new(Body::Text(
            "new".to_string(),
        )))),
    )
    .await
    .expect("the follow-up must resolve within the outer bound")
    .expect("the follow-up must resolve with its own fresh reply");
    let output = second
        .output
        .as_ref()
        .expect("the follow-up reply must populate the OUT message");
    assert_eq!(
        text_of(&output.body),
        "Hello new",
        "the follow-up must resolve with ITS OWN body, never the late reply"
    );
    assert_eq!(
        output.headers.get("x-which"),
        Some(&serde_json::json!("new")),
        "the follow-up must carry the fresh reply's header, not the late reply's"
    );

    let correlations = observed.lock().expect("observed mutex").clone();
    assert_eq!(
        correlations.len(),
        2,
        "the replier must have observed both requests"
    );
    assert_eq!(
        output
            .headers
            .get("correlationId")
            .and_then(|value| value.as_str()),
        Some(correlations[1].as_str()),
        "the follow-up response must correlate to the follow-up request"
    );
    assert_ne!(
        output
            .headers
            .get("correlationId")
            .and_then(|value| value.as_str()),
        Some(correlations[0].as_str()),
        "the late first reply must not misroute the follow-up request"
    );
    eprintln!(
        "late_reply_dropped_docker: first_elapsed={first_elapsed:?} \
         late_drop_barrier={barrier_elapsed:?} first_corr={} fresh_corr={} \
         fresh_body={}",
        correlations[0],
        correlations[1],
        text_of(&output.body)
    );

    timeout(REAP_BOUND, &mut replier.0)
        .await
        .expect("the replier task must finish within the reap bound")
        .expect("the replier task must not panic");
    stop_consumer(&mut consumer).await;
}

/// Scenario (4): a replier whose route fails sends no reply; the requester
/// times out and never resolves with a body.
#[tokio::test]
async fn failed_route_sends_no_reply_docker() {
    let Some(fx) = require_fixture() else {
        return;
    };
    let queue = unique("rpc-failed");
    let (_conn, _channel) = fx.declare_queue(&queue).await;

    let component = component_for(&fx.amqp_url);
    let replier_endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}"),
            &NoOpComponentContext,
        )
        .expect("replier endpoint creation must succeed");
    let requester_endpoint = component
        .create_endpoint(
            &format!("rabbitmq:default?queue={queue}&replyTimeout=1500"),
            &NoOpComponentContext,
        )
        .expect("requester endpoint creation must succeed");

    let (mut consumer, rx, _cancel) = start_consumer(&*replier_endpoint, "failed-replier").await;

    let mut replier = AbortOnDrop(tokio::spawn(async move {
        let mut rx = rx;
        let env = recv_envelope(&mut rx).await;
        // A business route failure: no OUT, no reply.
        let _ = env
            .reply_tx
            .expect("the engine always attaches a reply sender")
            .send(Err(CamelError::ProcessorError("route failed".to_string())));
    }));

    let mut producer = requester_endpoint
        .create_producer(noop_rt(), &ProducerContext::default())
        .expect("requester producer creation must succeed");

    let started = Instant::now();
    let result = timeout(
        OUTER_BOUND,
        producer.call(Exchange::new_in_out(Message::new(Body::Text(
            "will-fail".to_string(),
        )))),
    )
    .await
    .expect("the request must fail within the outer bound");
    let elapsed = started.elapsed();

    let error = result.expect_err("a failed route must send no reply, so the request times out");
    eprintln!("failed_route_sends_no_reply_docker: elapsed={elapsed:?} error={error}");
    assert!(
        error.to_string().contains("reply"),
        "the request must fail with the reply-timeout error, got: {error}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "the reply timeout must fire within 3 s, waited {elapsed:?}"
    );

    timeout(REAP_BOUND, &mut replier.0)
        .await
        .expect("the replier task must finish within the reap bound")
        .expect("the replier task must not panic");
    stop_consumer(&mut consumer).await;
}
