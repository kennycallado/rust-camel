//! End-to-end route tests for the Tower middleware hooks (mission 510,
//! Task 3.3).
//!
//! Each test drives a REAL `CamelContext` route through the same harness as
//! `tests/log_eip_test.rs` (timer/direct producers, mock receivers). The three
//! hooks are pinned at their authored granularities:
//!
//! - `to_layered` — producer granularity (one send wrapped);
//! - `layered` — section granularity (`Stop` inside the section short-circuits
//!   the rest of the route);
//! - `route_layer` — whole-route granularity (the wrap ENGAGES: a timeout
//!   fires; the route error handler retries the WHOLE wrapped route).
//!
//! Runtime stays under 5s: timer periods are 50ms and the only intentional
//! slow processors (400ms) run once each in the timeout-envelope test.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{BoxProcessor, CamelError, Exchange, ProcessorLayer};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use tower::ServiceExt;
use tower::limit::ConcurrencyLimitLayer;
use tower::timeout::TimeoutLayer;

/// Runtime observability stub for `Endpoint::create_producer`.
fn test_rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoOpComponentContext)
}

/// Drive one exchange straight into a `direct:` route's compiled pipeline.
///
/// A single producer call traverses the whole route (inline dispatch), so the
/// returned `Result` is the pipeline outcome verbatim. Direct dispatch is
/// intentionally single-flight per entry: the inline admission mutex serializes
/// concurrent entries, which is why the suite never asserts route-level
/// concurrency through one direct endpoint.
async fn drive_direct(
    ctx: &CamelContext,
    uri: &str,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let component = ctx
        .registry()
        .get("direct")
        .expect("direct component must be registered");
    let producer_ctx = ctx.producer_context();
    let endpoint = component
        .create_endpoint(uri, ctx)
        .expect("direct endpoint creation must succeed for a registered route");
    let producer = endpoint
        .create_producer(test_rt(), &producer_ctx)
        .expect("direct producer creation must succeed");
    producer.oneshot(exchange).await
}

/// Walk the public `source()` chain (three hops max) for the Tower timeout
/// marker, matching how a consumer classifies `From<BoxError>` errors.
fn source_chain_reaches_elapsed(err: &CamelError) -> bool {
    let mut current = std::error::Error::source(err);
    for _ in 0..3 {
        let Some(node) = current else { return false };
        if node
            .downcast_ref::<tower::timeout::error::Elapsed>()
            .is_some()
        {
            return true;
        }
        current = std::error::Error::source(node);
    }
    false
}

/// A layer that changes nothing: it only proves the whole-route wrap arm
/// compiles and runs while the route-level handler retries that single unit.
#[derive(Clone, Copy)]
struct NoopLayer;

impl ProcessorLayer for NoopLayer {
    fn wrap(&self, inner: BoxProcessor) -> Result<BoxProcessor, CamelError> {
        Ok(inner)
    }
}

// ---------------------------------------------------------------------------
// Producer granularity: `to_layered` delivers a single exchange through the
// layer without blocking it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn route_to_layered_delivers_through_layer() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:layered?period=50&repeatCount=1")
        .route_id("layered-producer-route")
        .to_layered("mock:out", ConcurrencyLimitLayer::new(4))
        .build()
        .unwrap();

    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();

    // Bounded settle window for a repeatCount=1 timer route; not a polling loop.
    tokio::time::sleep(Duration::from_millis(300)).await; // allow-test-sleep: fixed timer settle window
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("out").expect("mock:out registered");
    endpoint.assert_exchange_count(1).await;
}

// ---------------------------------------------------------------------------
// Section granularity: a `Stop` inside a `layered` section stops the route
// before the downstream send.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn route_stop_inside_layered_section_stops_route() {
    // The stop discards the exchange before it reaches the mock, and
    // branch-cloned copies fork body state — so the observable state lives in
    // a shared log, not on the exchange.
    let log = Arc::new(Mutex::new(Vec::<String>::new()));

    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let before_log = Arc::clone(&log);
    let inside_log = Arc::clone(&log);
    let after_log = Arc::clone(&log);

    let route = RouteBuilder::from("timer:stop?period=50&repeatCount=1")
        .route_id("layered-stop-route")
        .process(move |ex: Exchange| {
            let log = Arc::clone(&before_log);
            async move {
                log.lock().expect("log mutex").push("before".to_string());
                Ok(ex)
            }
        })
        .layered(ConcurrencyLimitLayer::new(4), move |section| {
            let inside_log = Arc::clone(&inside_log);
            let after_log = Arc::clone(&after_log);
            section
                .process(move |ex: Exchange| {
                    let log = Arc::clone(&inside_log);
                    async move {
                        log.lock().expect("log mutex").push("inside".to_string());
                        Ok(ex)
                    }
                })
                .stop()
                .process(move |ex: Exchange| {
                    let log = Arc::clone(&after_log);
                    async move {
                        log.lock().expect("log mutex").push("after".to_string());
                        Ok(ex)
                    }
                })
        })
        .to("mock:out")
        .build()
        .unwrap();

    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();

    // Bounded settle window for a repeatCount=1 timer route; not a polling loop.
    tokio::time::sleep(Duration::from_millis(300)).await; // allow-test-sleep: fixed timer settle window
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("out").expect("mock:out registered");
    endpoint.assert_exchange_count(0).await;

    let entries = log.lock().expect("log mutex").clone();
    assert_eq!(
        entries,
        vec!["before".to_string(), "inside".to_string()],
        "the pre-stop step must run, the post-stop step must not"
    );
}

// ---------------------------------------------------------------------------
// Whole-route granularity: `route_layer` ENGAGES the wrap.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn route_layer_timeout_envelopes_route() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(DirectComponent::new());
    ctx.register_component(mock.clone());

    // Control route: same slow pipeline, no route layer.
    let control = RouteBuilder::from("direct:ctrl")
        .route_id("timeout-control-route")
        .process(|ex: Exchange| async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Ok(ex)
        })
        .to("mock:ctrl")
        .build()
        .unwrap();

    // Layered route: the whole route is wrapped; 400ms > 100ms deadline.
    let layered = RouteBuilder::from("direct:slow")
        .route_id("timeout-layered-route")
        .process(|ex: Exchange| async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Ok(ex)
        })
        .to("mock:slow")
        .route_layer(TimeoutLayer::new(Duration::from_millis(100)))
        .build()
        .unwrap();

    ctx.add_route_definition(control).await.unwrap();
    ctx.add_route_definition(layered).await.unwrap();
    ctx.start().await.unwrap();

    let control_result = drive_direct(
        &ctx,
        "direct:ctrl",
        Exchange::new(camel_api::Message::new("control")),
    )
    .await;
    control_result.expect("the unlayered control route must complete");

    let layered_result = drive_direct(
        &ctx,
        "direct:slow",
        Exchange::new(camel_api::Message::new("layered")),
    )
    .await;
    let err = layered_result.expect_err("the 100ms route layer must time out the 400ms route");
    assert!(
        source_chain_reaches_elapsed(&err),
        "expected a timeout-classified CamelError, got: {err:?}"
    );

    ctx.stop().await.unwrap();

    let ctrl = mock.get_endpoint("ctrl").expect("mock:ctrl registered");
    ctrl.assert_exchange_count(1).await;
    let slow = mock.get_endpoint("slow").expect("mock:slow registered");
    slow.assert_exchange_count(0).await;
}

// ---------------------------------------------------------------------------
// Whole-route granularity: the route error handler retries the WHOLE wrapped
// route as one unit.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn route_error_handler_retries_whole_wrapped_route() {
    // Retry breaks step-level counting (`run_steps` forks the pre-attempt
    // exchange with `branch_clone`), so the invocation log is shared state and
    // the failing step keys its visit count off a property the TEST stamped
    // before sending — properties set by the failing step itself would not
    // survive the fork.
    let invocations = Arc::new(Mutex::new(Vec::<u64>::new()));
    let sequence = Arc::new(AtomicU64::new(0));
    let visits = Arc::new(Mutex::new(HashMap::<u64, usize>::new()));

    let retry_config = ErrorHandlerConfig::log_only()
        .on_exception(|_| true)
        .retry(2)
        .with_backoff(Duration::from_millis(1), 1.0, Duration::from_millis(1))
        .build();

    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(DirectComponent::new());
    ctx.register_component(mock.clone());

    let invocation_log = Arc::clone(&invocations);
    let invocation_sequence = Arc::clone(&sequence);
    let failure_visits = Arc::clone(&visits);

    let route = RouteBuilder::from("direct:retry")
        .route_id("whole-route-retry")
        .process(move |ex: Exchange| {
            let log = Arc::clone(&invocation_log);
            let seq = Arc::clone(&invocation_sequence);
            async move {
                let n = seq.fetch_add(1, Ordering::SeqCst);
                log.lock().expect("invocation log mutex").push(n);
                Ok(ex)
            }
        })
        .process(move |ex: Exchange| {
            let visits = Arc::clone(&failure_visits);
            async move {
                let probe = ex
                    .property("probe-id")
                    .and_then(|value| value.as_u64())
                    .expect("the test stamps probe-id before sending");
                let mut seen = visits.lock().expect("visit map mutex");
                let count = seen.entry(probe).or_insert(0);
                *count += 1;
                if *count == 1 {
                    Err(CamelError::ProcessorError("first visit fails".into()))
                } else {
                    Ok(ex)
                }
            }
        })
        .to("mock:out")
        .route_layer(NoopLayer)
        .error_handler(retry_config)
        .build()
        .unwrap();

    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();

    let mut exchange = Exchange::new(camel_api::Message::new("retry"));
    exchange.set_property("probe-id", 42_u64);

    let result = drive_direct(&ctx, "direct:retry", exchange).await;
    result.expect("the retried whole route must succeed on its second attempt");

    let endpoint = mock.get_endpoint("out").expect("mock:out registered");
    endpoint.assert_exchange_count(1).await;

    let entries = invocations.lock().expect("invocation log mutex").clone();
    assert_eq!(
        entries.len(),
        2,
        "the pre-step must re-run — only a whole-route retry does that"
    );

    ctx.stop().await.unwrap();
}

// ---------------------------------------------------------------------------
// `to_layered` producer resolution failures abort route-add (not swallowed).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn layered_producer_readiness_error_surfaces() {
    // `mock` is deliberately NOT registered: producer resolution must fail
    // through the wrap arm instead of silently degrading the send.
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(DirectComponent::new());

    let route = RouteBuilder::from("direct:in")
        .route_id("unresolved-layered-producer")
        .to_layered("mock:x", ConcurrencyLimitLayer::new(2))
        .build()
        .unwrap();

    let err = ctx
        .add_route_definition(route)
        .await
        .expect_err("an unregistered component must abort route-add");
    // Route-add wraps compile errors as `RouteError`; the resolution failure
    // itself must still be visible in the message (not swallowed by the wrap).
    assert!(
        matches!(&err, CamelError::RouteError(message) if message.contains("Component not found: mock")),
        "expected a component-resolution RouteError, got: {err:?}"
    );
}
