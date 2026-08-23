//! E2E regression for the TracingProcessor tower-contract fix (rc-qoq3):
//! with tracing enabled, a `to: direct:` InOut hop must complete, and the
//! same traced pipeline must serve a second exchange.
//!
//! Before the fix, `TracingProcessor::call` readied a *clone* of the inner
//! producer. For `DirectProducer` that meant re-acquiring a semaphore permit
//! the original instance already held, so the first (or, at best, the second)
//! InOut exchange through a traced `to: direct:` step hung forever. The
//! defect is exporter-independent: a noop provider with tracing enabled is
//! enough to reproduce it.
//!
//! Mirrors the wiring of `direct_top_level_test.rs` (CamelContext, route
//! registration, direct-producer drive) and `tracer_test.rs` (tracing
//! enablement).

use std::time::Duration;

use camel_api::{CamelError, Exchange, Message, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_test::CamelTestContext;
use tower::ServiceExt;

const HEADER_KEY: &str = "echo-mark";
const HEADER_VALUE: &str = "from-echo";

fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(camel_component_api::NoOpComponentContext)
}

/// True once `route_id` reports the `Started` status.
async fn route_started(h: &CamelTestContext, route_id: &str) -> bool {
    let ctx = h.ctx().lock().await;
    matches!(
        ctx.runtime_route_status(route_id).await,
        Ok(Some(status)) if status == "Started"
    )
}

/// Poll until every route reports `Started`, mirroring the startup-wait
/// pattern of `direct_top_level_test.rs`.
async fn wait_for_started(h: &CamelTestContext, route_ids: &[&str]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut all_started = true;
        for id in route_ids {
            if !route_started(h, id).await {
                all_started = false;
                break;
            }
        }
        if all_started {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "routes {route_ids:?} did not reach Started within 5s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Drive one InOut exchange through the `entry` pipeline via a direct
/// producer (`direct_top_level_test.rs` drive pattern: a fresh producer per
/// attempt, retried on fast errors until `retry_window` elapses, so startup
/// registration races cannot masquerade as regressions). Returns the reply
/// exchange on success.
async fn drive_entry_in_out(
    h: &CamelTestContext,
    body: &str,
    retry_window: Duration,
) -> Result<Exchange, CamelError> {
    let deadline = tokio::time::Instant::now() + retry_window;
    loop {
        let producer = {
            let ctx = h.ctx().lock().await;
            let producer_ctx = ctx.producer_context();
            let registry = ctx.registry();
            let component = registry
                .get("direct")
                .expect("direct component not registered");
            let endpoint = component
                .create_endpoint("direct:entry", &*ctx)
                .expect("failed to create direct endpoint");
            endpoint
                .create_producer(test_rt(), &producer_ctx)
                .expect("failed to create direct producer")
        };
        match producer
            .oneshot(Exchange::new_in_out(Message::new(body)))
            .await
        {
            Ok(reply) => return Ok(reply),
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn otel_enabled_direct_hop_completes_and_repeats() {
    let h = CamelTestContext::builder().with_direct().build().await;
    h.ctx().lock().await.set_tracing(true).await;

    // echo consumes from direct:echo and stamps one header; entry hops to it
    // with a `to: direct:` step. Lower startup_order starts the consumer
    // before the route that depends on it.
    let entry = RouteBuilder::from("direct:entry")
        .route_id("entry")
        .startup_order(200)
        .to("direct:echo")
        .build()
        .unwrap();
    let echo = RouteBuilder::from("direct:echo")
        .route_id("echo")
        .startup_order(50)
        .set_header(HEADER_KEY, Value::String(HEADER_VALUE.into()))
        .build()
        .unwrap();

    h.add_route(entry).await.unwrap();
    h.add_route(echo).await.unwrap();
    h.start().await;

    wait_for_started(&h, &["entry", "echo"]).await;

    // Exchange A: must complete and carry echo's header effect.
    let reply_a = tokio::time::timeout(
        Duration::from_secs(5),
        drive_entry_in_out(&h, "first", Duration::from_secs(4)),
    )
    .await
    .expect("first InOut exchange through traced entry pipeline timed out")
    .expect("first InOut exchange through traced entry pipeline failed");
    assert_eq!(
        reply_a
            .input
            .headers
            .get(HEADER_KEY)
            .and_then(|v| v.as_str()),
        Some(HEADER_VALUE),
        "echo's set_header effect must be present in the InOut reply"
    );

    // Exchange B: the permit-wedge defect would hang the second exchange.
    let reply_b = tokio::time::timeout(
        Duration::from_secs(5),
        drive_entry_in_out(&h, "second", Duration::from_secs(4)),
    )
    .await
    .expect("second InOut exchange through traced entry pipeline timed out")
    .expect("second InOut exchange through traced entry pipeline failed");
    assert_eq!(reply_b.input.body.as_text(), Some("second"));

    h.stop().await;
}
