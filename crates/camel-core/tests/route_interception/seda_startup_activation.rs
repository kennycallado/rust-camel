//! SEDA consumer startup activation handshake (rc-dbrkr).
//!
//! Spec: openspec/changes/seda-startup-activation/specs/seda-component/spec.md
//!
//! SEDA consumers must use the Explicit startup mode: `ctx.start()` returns
//! only after the consumer's activation state is published, so a producer
//! send that starts immediately after startup succeeds without the
//! probe/retry synchronization the old Immediate mode forced on callers.

use std::time::Duration;

use camel_api::{Exchange, Message};
use camel_core::RouteDefinition;
use camel_core::route::BuilderStep;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::common::{TEST_TIMEOUT, boot_context, raw_seda_producer, test_rt};

fn consumer_route(from: &str, route_id: &str) -> RouteDefinition {
    RouteDefinition::new(from, vec![BuilderStep::To("mock:arrival".into())]).with_route_id(route_id)
}

/// Scenario: no pre-activation message-loss window. The send executes
/// immediately after `ctx.start()` returned `Ok`, in the same task, with no
/// probe or retry synchronization; the pre-enqueue "no active consumers"
/// gate must pass on the first attempt.
#[tokio::test]
async fn send_immediately_after_start_passes_gate_first_try() {
    let (mut ctx, mock) = boot_context().await;
    ctx.add_route_definition(consumer_route("seda:out", "consumer-route"))
        .await
        .expect("consumer route must register");
    ctx.start().await.expect("context start failed");

    // First send after start: must enqueue without observing the
    // pre-activation gate.
    let producer = raw_seda_producer(&ctx, "seda:out");
    producer
        .oneshot(Exchange::new(Message::new("hello")))
        .await
        .expect("first send after start must enqueue (no pre-activation window)");

    let arrival = mock
        .get_endpoint("arrival")
        .expect("mock endpoint 'arrival' must exist");
    arrival.await_exchanges(1, TEST_TIMEOUT).await;
    arrival.assert_exchange_count(1).await;
}

/// Scenario: startup handshake awaits consumer activation (Fanout arm).
/// Same contract for a `multipleConsumers=true` endpoint: the subscriber is
/// registered before `ctx.start()` returns, so the first send is enqueued.
#[tokio::test]
async fn fanout_send_immediately_after_start_passes_gate_first_try() {
    let (mut ctx, mock) = boot_context().await;
    ctx.add_route_definition(consumer_route(
        "seda:fan?multipleConsumers=true",
        "consumer-route",
    ))
    .await
    .expect("consumer route must register");
    ctx.start().await.expect("context start failed");

    let producer = raw_seda_producer(&ctx, "seda:fan?multipleConsumers=true");
    producer
        .oneshot(Exchange::new(Message::new("hello fan")))
        .await
        .expect("first send after start must enqueue (no pre-activation window)");

    let arrival = mock
        .get_endpoint("arrival")
        .expect("mock endpoint 'arrival' must exist");
    arrival.await_exchanges(1, TEST_TIMEOUT).await;
    arrival.assert_exchange_count(1).await;
}

/// Race canary for the pre-activation window (rc-dbrkr). On a multi-thread
/// runtime, `ctx.start()` under the old Immediate startup mode returned
/// before the spawned consumer task stored `active = true`, so a send
/// issued immediately after startup raced the activation and could hit the
/// pre-enqueue "no active consumers" gate. Each iteration boots a fresh
/// context on a fresh runtime and sends with no synchronization; any gate
/// rejection is a failure. Under the Explicit handshake the gate passing on
/// the first attempt is guaranteed, not probabilistic.
#[test]
fn stress_start_return_implies_active_consumer() {
    const ITERATIONS: usize = 50;
    for i in 0..ITERATIONS {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime");
        let outcome = rt.block_on(async {
            let (mut ctx, _mock) = boot_context().await;
            ctx.add_route_definition(consumer_route("seda:race", "consumer-route"))
                .await
                .expect("consumer route must register");
            ctx.start().await.expect("context start failed");
            let producer = raw_seda_producer(&ctx, "seda:race");
            producer
                .oneshot(Exchange::new(Message::new("racy send")))
                .await
        });
        rt.shutdown_timeout(Duration::from_secs(1));
        assert!(
            outcome.is_ok(),
            "iteration {i}: send after start() Ok hit the pre-activation gate: {:?}",
            outcome.err()
        );
    }
}

/// Scenario: startup failure propagates before readiness. A consumer whose
/// `start()` returns `Err` before signalling readiness (here: a foreign
/// consumer already holds the Single-mode endpoint's receiver) must surface
/// as a route-start failure, and startup must not hang.
#[tokio::test]
async fn consumer_start_error_surfaces_as_route_start_failure() {
    use camel_component_api::ConsumerContext;

    let (mut ctx, _mock) = boot_context().await;

    // A foreign consumer already holds the Single-mode endpoint's receiver.
    let component = ctx
        .registry()
        .get("seda")
        .expect("seda component not registered");
    let endpoint = component
        .create_endpoint("seda:dup", &ctx)
        .expect("failed to create seda endpoint");
    let mut foreign = endpoint
        .create_consumer(test_rt())
        .expect("failed to create seda consumer");
    let (route_tx, _route_rx) = tokio::sync::mpsc::channel(16);
    foreign
        .start(ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "foreign-holder".to_string(),
        ))
        .await
        .expect("foreign consumer must start");

    ctx.add_route_definition(consumer_route("seda:dup", "route-one"))
        .await
        .expect("route must register");

    let start = tokio::time::timeout(TEST_TIMEOUT, ctx.start()).await;
    assert!(start.is_ok(), "ctx.start() must terminate, not hang");
    // Route-start failure semantics on this path: the failed consumer start
    // drives the route into the Failed state (delivered through the runtime
    // command bus). The contract under test is that the pre-readiness error
    // surfaces as a route-start failure without hanging startup.
    let status = ctx
        .runtime_route_status("route-one")
        .await
        .expect("route status query must succeed");
    assert_eq!(
        status.as_deref(),
        Some("Failed"),
        "consumer start error must surface as a route-start failure"
    );
}
