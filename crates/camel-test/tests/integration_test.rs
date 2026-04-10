//! Integration tests for the rust-camel framework.
//!
//! These tests exercise the full pipeline: CamelContext → Consumer → Processors → Producer,
//! verifying that exchanges flow end-to-end correctly.
//!
//! All assertions use the `MockComponent` shared registry instead of manual
//! capture closures.

use camel_api::Value;
use camel_api::aggregator::AggregatorConfig;
use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_lines};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_file::FileComponent;
use camel_component_http::HttpComponent;
use camel_component_log::LogComponent;
use camel_test::CamelTestContext;

// ---------------------------------------------------------------------------
// Test 1: Timer → Mock (verify exchanges received)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timer_to_mock() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("test-route-1")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(3).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let first = &exchanges[0];
    assert_eq!(
        first.input.header("CamelTimerName"),
        Some(&serde_json::Value::String("tick".into()))
    );
    assert_eq!(
        first.input.header("CamelTimerCounter"),
        Some(&serde_json::Value::Number(1.into()))
    );
}

// ---------------------------------------------------------------------------
// Test 2: Timer → Filter → Mock (verify filtering behavior)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timer_filter_mock() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // Filter: only allow even-numbered ticks through to mock:result.
    // With the typestate FilterBuilder, the filter scope only executes
    // for matching exchanges. Non-matching exchanges skip the scope entirely.
    // Timer fires 4 times (counters 1, 2, 3, 4), but only even counters
    // (2, 4) pass the filter, so only 2 exchanges reach mock:result.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=4")
        .route_id("test-route-2")
        .filter(|ex| {
            ex.input
                .header("CamelTimerCounter")
                .and_then(|v| v.as_u64())
                .map(|n| n % 2 == 0)
                .unwrap_or(false)
        })
        .to("mock:result")
        .end_filter()
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(2).await;

    // Verify that only even-numbered exchanges passed through.
    let exchanges = endpoint.get_received_exchanges().await;
    let counters: Vec<u64> = exchanges
        .iter()
        .map(|ex| {
            ex.input
                .header("CamelTimerCounter")
                .and_then(|v| v.as_u64())
                .expect("CamelTimerCounter header missing")
        })
        .collect();
    assert_eq!(
        counters,
        vec![2, 4],
        "only even counters should pass filter"
    );
}

// ---------------------------------------------------------------------------
// Test N: Filter EIP — matching exchanges reach inner mock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn filter_matching_exchanges_reach_inner_mock() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // 4 exchanges, alternate active=true/false (n=0,1,2,3 → even=true).
    // Only active=true exchanges should reach mock:matched.
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter_clone = std::sync::Arc::clone(&counter);

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=4")
        .route_id("test-route-3")
        .process(move |mut ex: camel_api::Exchange| {
            let c = std::sync::Arc::clone(&counter_clone);
            async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ex.input
                    .set_header("active", Value::Bool(n.is_multiple_of(2)));
                Ok(ex)
            }
        })
        .filter(|ex| ex.input.header("active") == Some(&Value::Bool(true)))
        .to("mock:matched")
        .end_filter()
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("matched").unwrap();
    endpoint.assert_exchange_count(2).await;
}

// ---------------------------------------------------------------------------
// Test N+1: Filter EIP — non-matching exchanges skip inner, reach outer mock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn filter_non_matching_continue_outer_pipeline() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter_clone = std::sync::Arc::clone(&counter);

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=4")
        .route_id("test-route-4")
        .process(move |mut ex: camel_api::Exchange| {
            let c = std::sync::Arc::clone(&counter_clone);
            async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ex.input
                    .set_header("active", Value::Bool(n.is_multiple_of(2)));
                Ok(ex)
            }
        })
        .filter(|ex| ex.input.header("active") == Some(&Value::Bool(true)))
        .to("mock:inner")
        .end_filter()
        .to("mock:outer") // always reached by all 4 exchanges
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    h.stop().await;

    let inner = h.mock().get_endpoint("inner").unwrap();
    let outer = h.mock().get_endpoint("outer").unwrap();

    inner.assert_exchange_count(2).await; // only active=true exchanges
    outer.assert_exchange_count(4).await; // ALL exchanges continue past filter
}

// ---------------------------------------------------------------------------
// Test 3: Timer → SetHeader → Mock (verify headers set)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timer_set_header_mock() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("test-route-5")
        .set_header("environment", Value::String("test".into()))
        .set_header("version", Value::Number(1.into()))
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(3).await;

    let exchanges = endpoint.get_received_exchanges().await;
    for ex in exchanges.iter() {
        assert_eq!(
            ex.input.header("environment"),
            Some(&Value::String("test".into())),
            "Each exchange should have 'environment' header"
        );
        assert_eq!(
            ex.input.header("version"),
            Some(&Value::Number(1.into())),
            "Each exchange should have 'version' header"
        );
        // Timer's own headers should still be present
        assert!(
            ex.input.header("CamelTimerName").is_some(),
            "Timer headers should still be present"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 4: Timer → SetHeader → Log (verify full pipeline with log producer)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timer_to_log() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_component(LogComponent::new())
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=2")
        .route_id("test-route-6")
        .set_header("source", Value::String("integration-test".into()))
        .to("log:test?showHeaders=true")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Just verify it doesn't panic — the log producer writes to tracing
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    h.stop().await;
}

// ---------------------------------------------------------------------------
// Test 5: Multiple routes in a single context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_routes() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route_a = RouteBuilder::from("timer:routeA?period=50&repeatCount=2")
        .route_id("test-route-7")
        .set_header("route", Value::String("A".into()))
        .to("mock:resultA")
        .build()
        .unwrap();

    let route_b = RouteBuilder::from("timer:routeB?period=50&repeatCount=3")
        .route_id("test-route-8")
        .set_header("route", Value::String("B".into()))
        .to("mock:resultB")
        .build()
        .unwrap();

    h.add_route(route_a).await.unwrap();
    h.add_route(route_b).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    h.stop().await;

    let endpoint_a = h.mock().get_endpoint("resultA").unwrap();
    let endpoint_b = h.mock().get_endpoint("resultB").unwrap();

    endpoint_a.assert_exchange_count(2).await;
    endpoint_b.assert_exchange_count(3).await;

    let a_exchanges = endpoint_a.get_received_exchanges().await;
    let b_exchanges = endpoint_b.get_received_exchanges().await;

    assert_eq!(
        a_exchanges[0].input.header("route"),
        Some(&Value::String("A".into()))
    );
    assert_eq!(
        b_exchanges[0].input.header("route"),
        Some(&Value::String("B".into()))
    );
}

// ---------------------------------------------------------------------------
// Error-handling integration tests (Tasks 4 + 5)
// ---------------------------------------------------------------------------

use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, CircuitBreakerConfig};
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Duration;

/// A processor that always fails with the given message.
fn failing_step(msg: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |_ex| {
        Box::pin(async move { Err(CamelError::ProcessorError(msg.into())) })
    })
}

// ---------------------------------------------------------------------------
// Test 6: DLC receives the failed exchange
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dlc_receives_failed_exchange() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-9")
        .process_fn(failing_step("intentional"))
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.stop().await;

    let dlc = h.mock().get_endpoint("dlc").unwrap();
    let exchanges = dlc.get_received_exchanges().await;
    assert!(!exchanges.is_empty());
    assert!(exchanges[0].has_error());
}

// ---------------------------------------------------------------------------
// Test 7: Retry recovers before DLC
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_recovers_before_dlc() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let attempt = Arc::new(AtomicU32::new(0));
    let attempt_clone = Arc::clone(&attempt);
    let processor = BoxProcessor::from_fn(move |ex| {
        let a = Arc::clone(&attempt_clone);
        Box::pin(async move {
            let n = a.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(CamelError::ProcessorError("not yet".into()))
            } else {
                Ok(ex)
            }
        })
    });

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let eh = ErrorHandlerConfig::dead_letter_channel("mock:dlc")
        .on_exception(|_| true)
        .retry(3)
        .with_backoff(Duration::from_millis(1), 1.0, Duration::from_millis(10))
        .build();

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-10")
        .process_fn(processor)
        .error_handler(eh)
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    h.stop().await;

    // DLC should be empty — retry succeeded.
    if let Some(ep) = h.mock().get_endpoint("dlc") {
        assert_eq!(ep.get_received_exchanges().await.len(), 0);
    }
}

// ---------------------------------------------------------------------------
// Test 8: onException with specific handled_by endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn on_exception_handled_by_specific_endpoint() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let eh = ErrorHandlerConfig::dead_letter_channel("mock:default-dlc")
        .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
        .handled_by("mock:processor-errors")
        .build();

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-11")
        .process_fn(failing_step("processor error"))
        .error_handler(eh)
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.stop().await;

    // Specific handler received it.
    let specific = h.mock().get_endpoint("processor-errors").unwrap();
    assert!(!specific.get_received_exchanges().await.is_empty());

    // Default DLC did NOT receive it.
    if let Some(ep) = h.mock().get_endpoint("default-dlc") {
        assert_eq!(ep.get_received_exchanges().await.len(), 0);
    }
}

// ---------------------------------------------------------------------------
// Test 9: Global error handler fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn global_error_handler_fallback() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:global-dlc"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-12")
        .process_fn(failing_step("global test"))
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.stop().await;

    let dlc = h.mock().get_endpoint("global-dlc").unwrap();
    assert!(!dlc.get_received_exchanges().await.is_empty());
}

// ---------------------------------------------------------------------------
// Test 10: Per-route handler overrides global
// ---------------------------------------------------------------------------

#[tokio::test]
async fn per_route_overrides_global() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:global-dlc"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-13")
        .process_fn(failing_step("per-route test"))
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:route-dlc"))
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.stop().await;

    // Per-route DLC got it.
    let route_dlc = h.mock().get_endpoint("route-dlc").unwrap();
    assert!(!route_dlc.get_received_exchanges().await.is_empty());

    // Global DLC got nothing.
    if let Some(ep) = h.mock().get_endpoint("global-dlc") {
        assert_eq!(ep.get_received_exchanges().await.len(), 0);
    }
}

// ---------------------------------------------------------------------------
// Test 11: direct: error bubbles to calling route's DLC
// ---------------------------------------------------------------------------

#[tokio::test]
async fn direct_error_bubbles_to_caller() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // Subroute: direct:sub → failing processor (no error handler).
    let sub_route = RouteBuilder::from("direct:sub")
        .route_id("test-route-14")
        .process_fn(failing_step("subroute failure"))
        .build()
        .unwrap();
    h.add_route(sub_route).await.unwrap();

    // Calling route: timer → direct:sub → DLC catches the bubble.
    let main_route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-15")
        .to("direct:sub")
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:caller-dlc"))
        .build()
        .unwrap();
    h.ctx()
        .lock()
        .await
        .add_route_definition(main_route)
        .await
        .unwrap();

    h.start().await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    h.stop().await;

    let dlc = h.mock().get_endpoint("caller-dlc").unwrap();
    let exchanges = dlc.get_received_exchanges().await;
    assert!(!exchanges.is_empty());
    assert!(exchanges[0].has_error());
}

// ---------------------------------------------------------------------------
// Test 12: direct: error contained in subroute (does not reach caller)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn direct_error_contained_in_subroute() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // Subroute has its own DLC → absorbs the error.
    let sub_route = RouteBuilder::from("direct:sub2")
        .route_id("test-route-16")
        .process_fn(failing_step("contained failure"))
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:sub-dlc"))
        .build()
        .unwrap();
    h.add_route(sub_route).await.unwrap();

    // Calling route continues after subroute (error was absorbed).
    let main_route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-17")
        .to("direct:sub2")
        .to("mock:caller-received")
        .build()
        .unwrap();
    h.ctx()
        .lock()
        .await
        .add_route_definition(main_route)
        .await
        .unwrap();

    h.start().await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    h.stop().await;

    // Sub DLC got the error.
    let sub_dlc = h.mock().get_endpoint("sub-dlc").unwrap();
    assert!(!sub_dlc.get_received_exchanges().await.is_empty());

    // Caller continued — mock:caller-received got the exchange (with error marked).
    let caller = h.mock().get_endpoint("caller-received").unwrap();
    assert!(!caller.get_received_exchanges().await.is_empty());
}

// ---------------------------------------------------------------------------
// Test 13: No error handler — route continues without crashing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_error_handler_logs_and_continues() {
    let h = CamelTestContext::builder().with_timer().build().await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("test-route-18")
        .process_fn(failing_step("no handler"))
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.stop().await;
    // No panic — test passes if we get here.
}

// ---------------------------------------------------------------------------
// Test 14: Circuit breaker with error handler (end-to-end)
// ---------------------------------------------------------------------------

/// Verifies circuit breaker + error handler interaction:
///
/// - The pipeline is composed as `ErrorHandler(CircuitBreaker(Steps))`.
/// - The first `failure_threshold` errors pass through the circuit breaker
///   (Closed state), hit the error handler, and are routed to the DLC.
/// - After the threshold, the circuit opens. `poll_ready()` returns
///   `CamelError::CircuitOpen`, and the pipeline task backs off (1s sleep)
///   then retries. Since `open_duration` is 60s (much longer than the test),
///   the circuit stays open for the rest of the test.
/// - Meanwhile, the timer consumer keeps producing exchanges into the channel,
///   but the pipeline is stuck in the backoff loop and never processes them.
/// - Result: DLC receives exactly `failure_threshold` exchanges; `mock:sink`
///   receives zero.
#[tokio::test]
async fn circuit_breaker_with_error_handler() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=5")
        .route_id("test-route-19")
        .process_fn(failing_step("cb test failure"))
        .circuit_breaker(
            CircuitBreakerConfig::new()
                .failure_threshold(2)
                .open_duration(Duration::from_secs(60)),
        )
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .to("mock:sink")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Give enough time for all 5 timer ticks to fire (5 × 50ms = 250ms).
    // The circuit opens after 2 failures. The pipeline backs off in a
    // retry loop (1s sleep) rather than breaking, but since open_duration
    // is 60s, no further exchanges are processed during this window.
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.stop().await;

    // DLC received exactly the 2 exchanges that passed through the CB
    // before it opened — each marked with an error.
    let dlc = h.mock().get_endpoint("dlc").unwrap();
    let dlc_exchanges = dlc.get_received_exchanges().await;
    assert_eq!(
        dlc_exchanges.len(),
        2,
        "DLC should receive exactly failure_threshold (2) exchanges"
    );
    for ex in &dlc_exchanges {
        assert!(ex.has_error(), "Each DLC exchange should carry an error");
    }

    // mock:sink received nothing — every exchange failed before reaching it.
    if let Some(sink) = h.mock().get_endpoint("sink") {
        assert_eq!(
            sink.get_received_exchanges().await.len(),
            0,
            "mock:sink should receive zero exchanges"
        );
    }
}

// ---------------------------------------------------------------------------
// Splitter EIP integration tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test 15: Split with timer and mock (end-to-end)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn split_with_timer_and_mock() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // Timer fires once, body = "line1\nline2\nline3"
    // Split by lines, each fragment goes to mock:per-line
    // After split, aggregated result goes to mock:final
    let route = RouteBuilder::from("timer:split-test?period=100&repeatCount=1")
        .route_id("test-route-20")
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.body = camel_api::body::Body::Text("line1\nline2\nline3".to_string());
            Ok(ex)
        })
        .split(SplitterConfig::new(split_body_lines()).aggregation(AggregationStrategy::CollectAll))
        .to("mock:per-line")
        .end_split()
        .to("mock:final")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    h.stop().await;

    // Each line should have been sent to mock:per-line
    let per_line = h.mock().get_endpoint("per-line").unwrap();
    let per_line_count = per_line.get_received_exchanges().await.len();
    assert_eq!(
        per_line_count, 3,
        "Expected 3 per-line exchanges, got {per_line_count}"
    );

    // The aggregated result should have been sent to mock:final
    let final_ep = h.mock().get_endpoint("final").unwrap();
    let final_exchanges = final_ep.get_received_exchanges().await;
    assert_eq!(
        final_exchanges.len(),
        1,
        "Expected 1 final exchange, got {}",
        final_exchanges.len()
    );

    // CollectAll produces a JSON array of the fragment bodies
    let expected = serde_json::json!(["line1", "line2", "line3"]);
    match &final_exchanges[0].input.body {
        camel_api::body::Body::Json(v) => assert_eq!(
            *v, expected,
            "CollectAll should produce JSON array of fragment bodies"
        ),
        other => panic!("Expected JSON body from CollectAll, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 16: Split with error handler (stop_on_exception + DLC)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn split_with_error_handler() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:split-err?period=100&repeatCount=1")
        .route_id("test-route-21")
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.body = camel_api::body::Body::Text("a\nb".to_string());
            Ok(ex)
        })
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .split(SplitterConfig::new(split_body_lines()).stop_on_exception(true))
        .process_fn(failing_step("fragment boom"))
        .end_split()
        .to("mock:sink")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    h.stop().await;

    // The split error should propagate up to the route's error handler (DLC)
    let dlc = h.mock().get_endpoint("dlc").unwrap();
    let dlc_count = dlc.get_received_exchanges().await.len();
    assert_eq!(
        dlc_count, 1,
        "Expected exactly 1 DLC exchange (stop_on_exception), got {dlc_count}"
    );

    // The sink should NOT have received anything (error stops pipeline)
    let sink = h.mock().get_endpoint("sink").unwrap();
    let sink_count = sink.get_received_exchanges().await.len();
    assert_eq!(sink_count, 0, "Expected 0 sink exchanges, got {sink_count}");
}

// ---------------------------------------------------------------------------
// File component integration tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test 17: File consumer → Mock (read files from directory)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_consumer_to_mock() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_str().unwrap();

    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    std::fs::write(dir.path().join("b.txt"), "beta").unwrap();

    let h = CamelTestContext::builder()
        .with_component(FileComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from(&format!(
        "file:{dir_path}?noop=true&initialDelay=0&delay=100"
    ))
    .route_id("test-file-consumer")
    .to("mock:result")
    .build()
    .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        exchanges.len() >= 2,
        "Should have read at least 2 files, got {}",
        exchanges.len()
    );

    for ex in &exchanges {
        assert!(ex.input.header("CamelFileName").is_some());
        assert!(ex.input.header("CamelFileNameOnly").is_some());
    }
}

// ---------------------------------------------------------------------------
// Test 18: Timer → File producer (write files)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timer_to_file_producer() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_str().unwrap();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_component(FileComponent::new())
        .build()
        .await;

    let route = RouteBuilder::from("timer:write-test?period=50&repeatCount=2")
        .route_id("test-route-22")
        .set_header("CamelFileName", Value::String("output.txt".into()))
        .to(format!("file:{dir_path}?fileExist=Append"))
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    h.stop().await;

    assert!(
        dir.path().join("output.txt").exists(),
        "File should have been written"
    );
}

// ---------------------------------------------------------------------------
// Test 19: File consumer → Transform → File producer (file-to-file pipeline)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_to_file_pipeline() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();
    let input_path = input_dir.path().to_str().unwrap();
    let output_path = output_dir.path().to_str().unwrap();

    std::fs::write(input_dir.path().join("source.txt"), "hello world").unwrap();

    let h = CamelTestContext::builder()
        .with_component(FileComponent::new())
        .build()
        .await;

    let route = RouteBuilder::from(&format!(
        "file:{input_path}?noop=true&initialDelay=0&delay=100"
    ))
    .route_id("test-file-pipeline")
    .to(format!("file:{output_path}"))
    .build()
    .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    h.stop().await;

    assert!(
        output_dir.path().join("source.txt").exists(),
        "File should have been copied to output directory"
    );
    let content = std::fs::read_to_string(output_dir.path().join("source.txt")).unwrap();
    assert_eq!(content, "hello world");
}

// ---------------------------------------------------------------------------
// HTTP component integration tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test 20: HTTP component registration and endpoint creation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_component_registration_and_endpoint_creation() {
    use camel_component_api::Component;

    let component = HttpComponent::new();

    // Verify component scheme
    assert_eq!(component.scheme(), "http");

    // Verify endpoint can be created with config
    let endpoint = component
        .create_endpoint(
            "http://example.com/api?httpMethod=POST&connectTimeout=5000",
            &camel_component_api::NoOpComponentContext,
        )
        .unwrap();
    assert!(endpoint.uri().contains("httpMethod=POST"));
}

// ---------------------------------------------------------------------------
// Test 21: HTTP query params are forwarded (Bug #3 verification)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_query_params_forwarding_config() {
    use camel_component_http::HttpEndpointConfig;
    use camel_endpoint::UriConfig;

    // Verify config parsing forwards non-Camel query params
    let config = HttpEndpointConfig::from_uri(
        "http://api.example.com/v1/users?apiKey=secret123&httpMethod=GET&token=abc456",
    )
    .unwrap();

    // apiKey and token should be preserved for forwarding
    assert!(
        config.query_params.contains_key("apiKey"),
        "apiKey should be preserved"
    );
    assert!(
        config.query_params.contains_key("token"),
        "token should be preserved"
    );
    assert_eq!(config.query_params.get("apiKey").unwrap(), "secret123");
    assert_eq!(config.query_params.get("token").unwrap(), "abc456");

    // httpMethod should NOT be in query_params (it's a Camel option)
    assert!(
        !config.query_params.contains_key("httpMethod"),
        "httpMethod should not be forwarded"
    );
}

// ===========================================================================
// HTTP end-to-end integration tests (wiremock)
//
// These tests exercise the full pipeline path:
//   Timer/Direct → HttpProducer → real HTTP request → wiremock → Mock
//
// Unlike tests 20-21 which only verify config parsing, these prove that
// the HTTP component actually sends requests and maps responses correctly
// when wired into a CamelContext pipeline.
// ===========================================================================

// ---------------------------------------------------------------------------
// Test 22: HTTP GET end-to-end — timer fires, HTTP producer GETs wiremock,
//          response body lands in mock endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_get_e2e() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/greeting"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello from wiremock"))
        .expect(1)
        .mount(&server)
        .await;

    // Timer produces a non-empty body so resolve_method would pick POST.
    // Force GET via httpMethod param (same as Apache Camel usage).
    let http_uri = format!(
        "http://127.0.0.1:{}/api/greeting?httpMethod=GET&allowPrivateIps=true",
        server.address().port()
    );

    let h = CamelTestContext::builder()
        .with_timer()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-23")
        .to(&http_uri)
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.stop().await;

    // Verify mock captured the exchange with response body
    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];

    // Response body should be the wiremock response (arrives as Body::Bytes)
    match &ex.input.body {
        camel_api::body::Body::Bytes(b) => {
            assert_eq!(std::str::from_utf8(b).unwrap(), "hello from wiremock");
        }
        other => panic!("expected Body::Bytes, got {:?}", other),
    }

    // CamelHttpResponseCode header should be 200
    assert_eq!(
        ex.input.header("CamelHttpResponseCode"),
        Some(&serde_json::Value::Number(200.into()))
    );
}

// ---------------------------------------------------------------------------
// Test 23: HTTP POST with body — set_header sets method, body is forwarded
//          to wiremock, response captured by mock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_post_with_body_e2e() {
    use wiremock::matchers::{body_string, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/data"))
        .and(body_string("payload-from-camel"))
        .respond_with(ResponseTemplate::new(201).set_body_string("created"))
        .expect(1)
        .mount(&server)
        .await;

    let http_uri = format!(
        "http://127.0.0.1:{}/api/data?httpMethod=POST&allowPrivateIps=true",
        server.address().port()
    );

    let h = CamelTestContext::builder()
        .with_timer()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-24")
        .map_body(|_body| camel_api::body::Body::Text("payload-from-camel".into()))
        .to(&http_uri)
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];

    // Status 201
    assert_eq!(
        ex.input.header("CamelHttpResponseCode"),
        Some(&serde_json::Value::Number(201.into()))
    );

    // Response body
    match &ex.input.body {
        camel_api::body::Body::Bytes(b) => {
            assert_eq!(std::str::from_utf8(b).unwrap(), "created");
        }
        other => panic!("expected Body::Bytes, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Test 24: HTTP response headers are mapped into exchange headers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_response_headers_mapped_e2e() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/headers"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-custom-header", "custom-value")
                .insert_header("x-request-id", "req-42")
                .set_body_string("ok"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let http_uri = format!(
        "http://127.0.0.1:{}/api/headers?httpMethod=GET&allowPrivateIps=true",
        server.address().port()
    );

    let h = CamelTestContext::builder()
        .with_timer()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-25")
        .to(&http_uri)
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];

    // Custom response headers should be mapped into exchange headers
    assert_eq!(
        ex.input.header("x-custom-header"),
        Some(&serde_json::Value::String("custom-value".into()))
    );
    assert_eq!(
        ex.input.header("x-request-id"),
        Some(&serde_json::Value::String("req-42".into()))
    );
}

// ---------------------------------------------------------------------------
// Test 25: HTTP 500 with throwExceptionOnFailure=true triggers error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_error_handling_e2e() {
    use camel_api::error_handler::ErrorHandlerConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/fail"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
        .expect(1)
        .mount(&server)
        .await;

    // throwExceptionOnFailure=true is the default; force GET since timer has body
    let http_uri = format!(
        "http://127.0.0.1:{}/api/fail?httpMethod=GET&allowPrivateIps=true",
        server.address().port()
    );

    let h = CamelTestContext::builder()
        .with_timer()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-26")
        .to(&http_uri)
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.stop().await;

    // The exchange should have been routed to the DLC, not mock:result
    let dlc = h.mock().get_endpoint("dlc").unwrap();
    let dlc_exchanges = dlc.get_received_exchanges().await;
    assert_eq!(
        dlc_exchanges.len(),
        1,
        "DLC should receive the failed exchange"
    );
    assert!(
        dlc_exchanges[0].has_error(),
        "Exchange should carry an error"
    );

    // mock:result should NOT have received anything
    if let Some(result) = h.mock().get_endpoint("result") {
        assert_eq!(
            result.get_received_exchanges().await.len(),
            0,
            "mock:result should not receive the failed exchange"
        );
    }
}

// ---------------------------------------------------------------------------
// Aggregator EIP integration tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test 26 (Aggregator): collect_all — 9 timer fires, orderId cycles A/B/C,
//          completionSize=3, expect 3 batches each with Body::Json([...])
// ---------------------------------------------------------------------------

#[tokio::test]
async fn aggregator_collect_all() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter_clone = std::sync::Arc::clone(&counter);

    let route = RouteBuilder::from("timer:agg-test?period=1&repeatCount=9")
        .route_id("test-route-27")
        .process(move |mut ex: camel_api::Exchange| {
            let c = std::sync::Arc::clone(&counter_clone);
            async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let key = ["A", "B", "C"][(n % 3) as usize];
                ex.input
                    .headers
                    .insert("orderId".to_string(), serde_json::json!(key));
                ex.input.body = camel_api::body::Body::Text(format!("item-{n}"));
                Ok(ex)
            }
        })
        .aggregate(
            AggregatorConfig::correlate_by("orderId")
                .complete_when_size(3)
                .build(),
        )
        .to("mock:aggregated")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    h.stop().await;

    // 9 exchanges total: 6 pending (Body::Empty) + 3 completed (Body::Json).
    // The aggregator emits all exchanges; pending ones have Body::Empty.
    let endpoint = h.mock().get_endpoint("aggregated").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    let completed: Vec<_> = exchanges
        .iter()
        .filter(|e| e.property("CamelAggregatorPending").is_none())
        .collect();
    assert_eq!(
        completed.len(),
        3,
        "expected 3 completed batches, got {}",
        completed.len()
    );

    for ex in &completed {
        let camel_api::body::Body::Json(v) = &ex.input.body else {
            panic!("expected Body::Json, got {:?}", ex.input.body);
        };
        let arr = v.as_array().expect("expected JSON array");
        assert_eq!(arr.len(), 3, "each batch should have 3 items");
    }
}

// ---------------------------------------------------------------------------
// Test 27 (Aggregator): custom_strategy — 4 timer fires with key "X",
//          custom fold concatenates bodies with "+", completionSize=4,
//          expect 1 exchange with body "0+1+2+3"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn aggregator_custom_strategy() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter_clone = std::sync::Arc::clone(&counter);

    let fold_fn: camel_api::aggregator::AggregationFn =
        std::sync::Arc::new(|mut acc: camel_api::Exchange, next: camel_api::Exchange| {
            let a = acc.input.body.as_text().unwrap_or("").to_string();
            let b = next.input.body.as_text().unwrap_or("").to_string();
            acc.input.body = camel_api::body::Body::Text(format!("{a}+{b}"));
            acc
        });

    let route = RouteBuilder::from("timer:agg-custom?period=1&repeatCount=4")
        .route_id("test-route-28")
        .process(move |mut ex: camel_api::Exchange| {
            let c = std::sync::Arc::clone(&counter_clone);
            async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ex.input
                    .headers
                    .insert("key".to_string(), serde_json::json!("X"));
                ex.input.body = camel_api::body::Body::Text(n.to_string());
                Ok(ex)
            }
        })
        .aggregate(
            AggregatorConfig::correlate_by("key")
                .complete_when_size(4)
                .strategy(camel_api::aggregator::AggregationStrategy::Custom(fold_fn))
                .build(),
        )
        .to("mock:custom-agg")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("custom-agg").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    // 4 exchanges total: 3 pending (Body::Empty) + 1 completed (Body::Text "0+1+2+3").
    let completed: Vec<_> = exchanges
        .iter()
        .filter(|e| e.property("CamelAggregatorPending").is_none())
        .collect();
    assert_eq!(
        completed.len(),
        1,
        "expected 1 completed aggregate, got {}",
        completed.len()
    );
    assert_eq!(completed[0].input.body.as_text(), Some("0+1+2+3"));
}

// ---------------------------------------------------------------------------
// Test 28 (Aggregator): scatter-gather — timer fires 3 times, each fire
//          is aggregated by timer name, completionSize=3,
//          expect 1 exchange at mock:scatter-gather
// ---------------------------------------------------------------------------

#[tokio::test]
async fn aggregator_scatter_gather() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // Timer fires 3 times with the same timer name "scatter".
    // The aggregator groups by CamelTimerName (all 3 share key "scatter")
    // and emits when completionSize=3 is reached.
    let route = RouteBuilder::from("timer:scatter?period=10&repeatCount=3")
        .route_id("test-route-29")
        .aggregate(
            AggregatorConfig::correlate_by("CamelTimerName")
                .complete_when_size(3)
                .build(),
        )
        .to("mock:scatter-gather")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    h.stop().await;

    // 3 exchanges: 2 pending + 1 completed (Body::Json array of 3).
    let endpoint = h.mock().get_endpoint("scatter-gather").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    let completed: Vec<_> = exchanges
        .iter()
        .filter(|e| e.property("CamelAggregatorPending").is_none())
        .collect();
    assert_eq!(
        completed.len(),
        1,
        "expected 1 completed aggregate, got {}",
        completed.len()
    );
}

// ---------------------------------------------------------------------------
// Test 29 (Aggregator): complete on inactivity timeout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn aggregator_agg_timeout_emits_after_inactivity() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:agg-timeout?period=50&repeatCount=1")
        .route_id("agg-timeout-route")
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.set_header("key", Value::String("A".into()));
            Ok(ex)
        })
        .aggregate(
            AggregatorConfig::correlate_by("key")
                .complete_on_timeout(std::time::Duration::from_millis(200))
                .build(),
        )
        .to("mock:agg-timeout-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Wait for timeout to fire (200ms + margin)
    tokio::time::sleep(std::time::Duration::from_millis(450)).await;

    h.stop().await;

    let endpoint = h.mock().get_endpoint("agg-timeout-result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

// ---------------------------------------------------------------------------
// Test 30 (Aggregator): force completion on stop flushes pending group
// ---------------------------------------------------------------------------

#[tokio::test]
async fn aggregator_agg_force_completion_on_stop() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:agg-force?period=50&repeatCount=1")
        .route_id("agg-force-route")
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.set_header("key", Value::String("A".into()));
            Ok(ex)
        })
        .aggregate(
            AggregatorConfig::correlate_by("key")
                .complete_when_size(100)
                .force_completion_on_stop(true)
                .build(),
        )
        .to("mock:agg-force-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Ensure one timer exchange is produced and remains pending.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    h.stop().await;

    let endpoint = h.mock().get_endpoint("agg-force-result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

// ---------------------------------------------------------------------------
// set_body / set_body_fn / set_header_fn integration tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test 29: set_body("enriched") replaces body end-to-end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn route_set_body_static() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:set-body-static?period=50&repeatCount=1")
        .route_id("test-route-30")
        .set_body("enriched")
        .to("mock:set-body-static")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("set-body-static").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges[0].input.body.as_text(),
        Some("enriched"),
        "set_body should replace body with static string"
    );
}

// ---------------------------------------------------------------------------
// Test 30: set_body_fn reads exchange and transforms body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn route_set_body_fn() {
    use camel_api::body::Body;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:set-body-fn?period=50&repeatCount=1")
        .route_id("test-route-31")
        // First set a known body so set_body_fn has something to read.
        .set_body("hello")
        .set_body_fn(|ex: &camel_api::Exchange| {
            let text = ex.input.body.as_text().unwrap_or("");
            Body::Text(text.to_uppercase())
        })
        .to("mock:set-body-fn")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("set-body-fn").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges[0].input.body.as_text(),
        Some("HELLO"),
        "set_body_fn should uppercase the body read from the exchange"
    );
}

// ---------------------------------------------------------------------------
// Test 31: set_header_fn reads body and writes it into a header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn route_set_header_fn() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:set-header-fn?period=50&repeatCount=1")
        .route_id("test-route-32")
        // Set a known body first so set_header_fn can echo it.
        .set_body("ping")
        .set_header_fn("echo", |ex: &camel_api::Exchange| {
            Value::String(ex.input.body.as_text().unwrap_or("").into())
        })
        .to("mock:set-header-fn")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("set-header-fn").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges[0].input.header("echo"),
        Some(&Value::String("ping".into())),
        "set_header_fn should copy body text into the 'echo' header"
    );
}

// ---------------------------------------------------------------------------
// Test 26: HTTP query params forwarded in actual request to wiremock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_query_params_forwarded_e2e() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .and(query_param("apiKey", "secret123"))
        .and(query_param("lang", "rust"))
        .respond_with(ResponseTemplate::new(200).set_body_string("found"))
        .expect(1)
        .mount(&server)
        .await;

    // Non-Camel params (apiKey, lang) are forwarded; httpMethod is consumed by Camel
    let http_uri = format!(
        "http://127.0.0.1:{}/api/search?httpMethod=GET&allowPrivateIps=true&apiKey=secret123&lang=rust",
        server.address().port()
    );

    let h = CamelTestContext::builder()
        .with_timer()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-route-33")
        .to(&http_uri)
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    match &exchanges[0].input.body {
        camel_api::body::Body::Bytes(b) => {
            assert_eq!(std::str::from_utf8(b).unwrap(), "found");
        }
        other => panic!("expected Body::Bytes, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Stop EIP: .stop() inside filter halts outer pipeline for matching exchanges
// ---------------------------------------------------------------------------

/// Apache Camel semantics: exchanges matching the filter predicate hit .stop()
/// and do NOT continue to the outer pipeline. Non-matching exchanges skip the
/// filter block entirely and DO continue to the outer pipeline.
///
/// Route:
///   timer (4 exchanges, n=0..3, even=active=true)
///     .filter(active=true)
///       .to("mock:inner")   ← only active=true exchanges
///       .stop()             ← active=true exchanges stop here
///     .end_filter()
///     .to("mock:outer")     ← only active=false exchanges reach here
#[tokio::test]
async fn stop_inside_filter_prevents_outer_pipeline() {
    use std::time::Duration;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter_clone = std::sync::Arc::clone(&counter);

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=4")
        .route_id("test-route-34")
        .process(move |mut ex: camel_api::Exchange| {
            let c = std::sync::Arc::clone(&counter_clone);
            async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ex.input
                    .set_header("active", Value::Bool(n.is_multiple_of(2)));
                Ok(ex)
            }
        })
        .filter(|ex| ex.input.header("active") == Some(&Value::Bool(true)))
        .to("mock:inner")
        .stop() // active=true exchanges stop here
        .end_filter()
        .to("mock:outer") // only active=false exchanges should reach here
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    h.stop().await;

    let inner = h.mock().get_endpoint("inner").unwrap();
    let outer = h.mock().get_endpoint("outer").unwrap();

    inner.assert_exchange_count(2).await; // active=true: exchanges 0 and 2
    outer.assert_exchange_count(2).await; // active=false: exchanges 1 and 3 (stopped exchanges never reach outer)
}

// ---------------------------------------------------------------------------
// Multicast EIP integration tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test: Multicast sends exchange to multiple endpoints
// ---------------------------------------------------------------------------
// This test verifies that a route with Multicast correctly sends copies
// of the exchange to each endpoint defined in the multicast scope.

#[tokio::test]
async fn multicast_sends_to_multiple_endpoints() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // Multicast: timer → [mock:a, mock:b, mock:c] → mock:final
    // Each endpoint in the multicast should receive the exchange.
    let route = RouteBuilder::from("timer:multicast-test?period=50&repeatCount=1")
        .route_id("test-route-35")
        .multicast()
        .to("mock:a")
        .to("mock:b")
        .to("mock:c")
        .end_multicast()
        .to("mock:final")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    h.stop().await;

    // Each multicast endpoint should have received 1 exchange
    let endpoint_a = h.mock().get_endpoint("a").unwrap();
    let endpoint_b = h.mock().get_endpoint("b").unwrap();
    let endpoint_c = h.mock().get_endpoint("c").unwrap();
    let endpoint_final = h.mock().get_endpoint("final").unwrap();

    endpoint_a.assert_exchange_count(1).await;
    endpoint_b.assert_exchange_count(1).await;
    endpoint_c.assert_exchange_count(1).await;
    endpoint_final.assert_exchange_count(1).await;
}

// ---------------------------------------------------------------------------
// Test: Multicast sets metadata properties (index, complete)
// ---------------------------------------------------------------------------
// This test verifies that MulticastService correctly sets the CAMEL_MULTICAST_INDEX
// and CAMEL_MULTICAST_COMPLETE properties on each cloned exchange.

#[tokio::test]
async fn multicast_metadata_properties() {
    use camel_processor::CAMEL_MULTICAST_INDEX;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // Multicast with 3 endpoints - each should receive exchange with correct index
    let route = RouteBuilder::from("timer:multicast-meta?period=50&repeatCount=1")
        .route_id("test-route-36")
        .multicast()
        .to("mock:meta-a")
        .to("mock:meta-b")
        .to("mock:meta-c")
        .end_multicast()
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    h.stop().await;

    // Verify that each endpoint received an exchange with the correct index property
    let exchanges_a = h
        .mock()
        .get_endpoint("meta-a")
        .unwrap()
        .get_received_exchanges()
        .await;
    let exchanges_b = h
        .mock()
        .get_endpoint("meta-b")
        .unwrap()
        .get_received_exchanges()
        .await;
    let exchanges_c = h
        .mock()
        .get_endpoint("meta-c")
        .unwrap()
        .get_received_exchanges()
        .await;

    assert_eq!(exchanges_a.len(), 1);
    assert_eq!(exchanges_b.len(), 1);
    assert_eq!(exchanges_c.len(), 1);

    // The MulticastService should set CAMEL_MULTICAST_INDEX on each exchange
    // This will FAIL with the current placeholder implementation because it
    // doesn't use MulticastService at all
    let idx_a = exchanges_a[0].property(CAMEL_MULTICAST_INDEX);
    let idx_b = exchanges_b[0].property(CAMEL_MULTICAST_INDEX);
    let idx_c = exchanges_c[0].property(CAMEL_MULTICAST_INDEX);

    assert!(idx_a.is_some(), "meta-a should have CAMEL_MULTICAST_INDEX");
    assert!(idx_b.is_some(), "meta-b should have CAMEL_MULTICAST_INDEX");
    assert!(idx_c.is_some(), "meta-c should have CAMEL_MULTICAST_INDEX");

    // Verify correct index values
    assert_eq!(idx_a, Some(&Value::from(0i64)));
    assert_eq!(idx_b, Some(&Value::from(1i64)));
    assert_eq!(idx_c, Some(&Value::from(2i64)));
}

// ---------------------------------------------------------------------------
// Test: Multicast parallel CollectAll aggregation
// ---------------------------------------------------------------------------
// This test verifies that parallel multicast with CollectAll strategy produces
// a JSON array body containing the results from all endpoints.

#[tokio::test]
async fn multicast_parallel_collect_all() {
    use camel_api::body::Body;
    use camel_api::multicast::MulticastStrategy;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:multicast-parallel?period=50&repeatCount=1")
        .route_id("test-route-37")
        .multicast()
        .parallel(true)
        .aggregation(MulticastStrategy::CollectAll)
        .to("mock:p-a")
        .to("mock:p-b")
        .end_multicast()
        .to("mock:p-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    h.stop().await;

    // Each parallel endpoint received one exchange
    let endpoint_a = h.mock().get_endpoint("p-a").unwrap();
    let endpoint_b = h.mock().get_endpoint("p-b").unwrap();
    let endpoint_result = h.mock().get_endpoint("p-result").unwrap();

    endpoint_a.assert_exchange_count(1).await;
    endpoint_b.assert_exchange_count(1).await;
    endpoint_result.assert_exchange_count(1).await;

    // The result endpoint should have received a JSON array body (CollectAll)
    let result_exchanges = endpoint_result.get_received_exchanges().await;
    assert_eq!(result_exchanges.len(), 1);
    assert!(
        matches!(&result_exchanges[0].input.body, Body::Json(v) if v.is_array()),
        "expected JSON array body from CollectAll aggregation, got {:?}",
        result_exchanges[0].input.body
    );
}

// ---------------------------------------------------------------------------
// Test: HTTP concurrent pipeline processes multiple requests simultaneously
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_concurrent_pipeline() {
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("http://0.0.0.0:18080/concurrent-test")
        .route_id("test-route-38")
        .process(|ex| async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(ex)
        })
        .to("mock:concurrent-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fire 5 requests concurrently
    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    let start = std::time::Instant::now();
    for i in 0..5 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client
                .get(format!("http://127.0.0.1:18080/concurrent-test?i={i}"))
                .send()
                .await
                .unwrap()
        }));
    }

    for handle in handles {
        let resp = handle.await.unwrap();
        assert_eq!(resp.status(), 200);
    }
    let elapsed = start.elapsed();

    h.stop().await;

    // With concurrent pipeline: ~100ms (all 5 run in parallel).
    // With sequential pipeline: ~500ms (one at a time).
    // Use 350ms as threshold — generous margin but catches sequential.
    assert!(
        elapsed < std::time::Duration::from_millis(350),
        "Expected concurrent execution (<350ms), but took {:?}. \
         Pipeline may be running sequentially.",
        elapsed
    );

    let endpoint = h.mock().get_endpoint("concurrent-result").unwrap();
    endpoint.assert_exchange_count(5).await;
}

// ---------------------------------------------------------------------------
// Test: HTTP route with .sequential() override processes one at a time
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_sequential_override() {
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    // Same slow processor, but forced sequential via .sequential()
    let route = RouteBuilder::from("http://0.0.0.0:18081/sequential-test")
        .route_id("test-route-39")
        .sequential()
        .process(|ex| async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(ex)
        })
        .to("mock:sequential-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fire 3 requests concurrently
    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    let start = std::time::Instant::now();
    for i in 0..3 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client
                .get(format!("http://127.0.0.1:18081/sequential-test?i={i}"))
                .send()
                .await
                .unwrap()
        }));
    }

    for handle in handles {
        let resp = handle.await.unwrap();
        assert_eq!(resp.status(), 200);
    }
    let elapsed = start.elapsed();

    h.stop().await;

    // Sequential: ~300ms (3 * 100ms). Must be clearly above concurrent threshold.
    assert!(
        elapsed >= std::time::Duration::from_millis(250),
        "Expected sequential execution (>=250ms), but took {:?}. \
         Pipeline may be running concurrently despite .sequential() override.",
        elapsed
    );

    let endpoint = h.mock().get_endpoint("sequential-result").unwrap();
    endpoint.assert_exchange_count(3).await;
}

// ---------------------------------------------------------------------------
// Test: HTTP route with .concurrent(2) limits parallelism to 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_concurrent_with_semaphore_limit() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let peak = Arc::new(AtomicUsize::new(0));
    let current = Arc::new(AtomicUsize::new(0));
    let peak_clone = peak.clone();
    let current_clone = current.clone();

    // Limit to 2 concurrent pipeline executions
    let route = RouteBuilder::from("http://0.0.0.0:18082/semaphore-test")
        .route_id("test-route-40")
        .concurrent(2)
        .process(move |ex| {
            let peak = peak_clone.clone();
            let current = current_clone.clone();
            async move {
                let val = current.fetch_add(1, Ordering::SeqCst) + 1;
                // Update peak
                peak.fetch_max(val, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                current.fetch_sub(1, Ordering::SeqCst);
                Ok(ex)
            }
        })
        .to("mock:semaphore-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fire 6 requests concurrently
    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    for i in 0..6 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client
                .get(format!("http://127.0.0.1:18082/semaphore-test?i={i}"))
                .send()
                .await
                .unwrap()
        }));
    }

    for handle in handles {
        let resp = handle.await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    h.stop().await;

    // Peak concurrency should be exactly 2 (semaphore limit)
    let peak_val = peak.load(Ordering::SeqCst);
    assert!(
        peak_val <= 2,
        "Expected peak concurrency <= 2, but got {}. Semaphore not working.",
        peak_val
    );

    let endpoint = h.mock().get_endpoint("semaphore-result").unwrap();
    endpoint.assert_exchange_count(6).await;
}

// ---------------------------------------------------------------------------
// Test: HTTP concurrent pipeline with circuit breaker (short duration)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_concurrent_with_circuit_breaker() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    // Use short open_duration (1s) so circuit closes quickly and requests can complete
    let route = RouteBuilder::from("http://0.0.0.0:18083/cb-test")
        .route_id("test-route-41")
        .process_fn(failing_step("concurrent cb failure"))
        .circuit_breaker(
            CircuitBreakerConfig::new()
                .failure_threshold(2)
                .open_duration(Duration::from_secs(1)),
        )
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .to("mock:sink")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send 5 requests concurrently
    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    for i in 0..5 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client
                .get(format!("http://127.0.0.1:18083/cb-test?i={i}"))
                .send()
                .await
                .unwrap()
        }));
    }

    // All requests should complete (may take time due to circuit open backoff)
    for handle in handles {
        let resp = handle.await.unwrap();
        // Status could be 200 (DLC handled) or 500 (pipeline error)
        // We just assert it returns
        assert!(
            resp.status() == 200 || resp.status() == 500,
            "Expected 200 or 500, got {}",
            resp.status()
        );
    }

    h.stop().await;

    // Due to race conditions in concurrent mode, the circuit breaker
    // may see 2-5 failures before opening (multiple requests may be
    // in-flight simultaneously when threshold is reached)
    let dlc = h.mock().get_endpoint("dlc").unwrap();
    let dlc_exchanges = dlc.get_received_exchanges().await;
    assert!(
        dlc_exchanges.len() >= 2,
        "DLC should receive at least 2 exchanges (failure_threshold), got {}",
        dlc_exchanges.len()
    );
    assert!(
        dlc_exchanges.len() <= 5,
        "DLC should receive at most 5 exchanges (total requests), got {}",
        dlc_exchanges.len()
    );

    // All DLC exchanges should have errors
    for ex in &dlc_exchanges {
        assert!(ex.has_error(), "Each DLC exchange should carry an error");
    }

    // Mock sink receives 0 exchanges (all fail before reaching it)
    if let Some(sink) = h.mock().get_endpoint("sink") {
        assert_eq!(
            sink.get_received_exchanges().await.len(),
            0,
            "mock:sink should receive zero exchanges"
        );
    }
}

// ---------------------------------------------------------------------------
// Test: HTTP concurrent shutdown drains in-flight exchanges
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_concurrent_shutdown_drains_inflight() {
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("http://0.0.0.0:18084/shutdown-test")
        .route_id("test-route-42")
        .process(|ex| async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(ex)
        })
        .to("mock:shutdown-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send 3 requests concurrently
    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    for i in 0..3 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client
                .get(format!("http://127.0.0.1:18084/shutdown-test?i={i}"))
                .send()
                .await
                .unwrap()
        }));
    }

    // Wait 50ms (some requests started but not completed)
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Stop should wait for in-flight exchanges to drain (default 30s timeout)
    h.stop().await;

    // All 3 requests should complete
    for handle in handles {
        let resp = handle.await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    // Mock should have received all 3 exchanges (drain completed)
    let endpoint = h.mock().get_endpoint("shutdown-result").unwrap();
    endpoint.assert_exchange_count(3).await;
}

// ---------------------------------------------------------------------------
// Test: HTTP concurrent error propagation to HTTP responses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_concurrent_error_propagation() {
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("http://0.0.0.0:18085/error-test")
        .route_id("test-route-43")
        .process(|ex| async move {
            // Check query param "fail"
            let should_fail = ex
                .input
                .header("CamelHttpQuery")
                .and_then(|v| v.as_str())
                .map(|q| q.contains("fail=true"))
                .unwrap_or(false);

            if should_fail {
                Err(CamelError::ProcessorError("deliberate".into()))
            } else {
                Ok(ex)
            }
        })
        .to("mock:error-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send 4 requests concurrently: 2 with fail=true, 2 with fail=false
    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    let mut fail_indices = Vec::new();

    for i in 0..4 {
        let should_fail = i % 2 == 0; // 0, 2 fail; 1, 3 succeed
        if should_fail {
            fail_indices.push(i);
        }
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            let url = if should_fail {
                format!("http://127.0.0.1:18085/error-test?i={i}&fail=true")
            } else {
                format!("http://127.0.0.1:18085/error-test?i={i}&fail=false")
            };
            let resp = client.get(&url).send().await.unwrap();
            (i, should_fail, resp)
        }));
    }

    // Assert response statuses
    for handle in handles {
        let (i, should_fail, resp) = handle.await.unwrap();
        if should_fail {
            assert_eq!(
                resp.status(),
                500,
                "Request {i} with fail=true should return 500"
            );
        } else {
            assert_eq!(
                resp.status(),
                200,
                "Request {i} with fail=false should return 200"
            );
        }
    }

    h.stop().await;

    // Mock sink receives exactly 2 exchanges (the successful ones)
    let endpoint = h.mock().get_endpoint("error-result").unwrap();
    endpoint.assert_exchange_count(2).await;
}

// ---------------------------------------------------------------------------
// Choice EIP tests
// ---------------------------------------------------------------------------

// Test A: when clause routes matching exchange
#[tokio::test]
async fn choice_when_routes_matching_exchange() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // 4 ticks: counters 1,2,3,4. Even → mock:even, odd → mock:odd.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=4")
        .route_id("test-route-44")
        .choice()
        .when(|ex| {
            ex.input
                .header("CamelTimerCounter")
                .and_then(|v| v.as_u64())
                .map(|n| n % 2 == 0)
                .unwrap_or(false)
        })
        .to("mock:even")
        .end_when()
        .when(|ex| {
            ex.input
                .header("CamelTimerCounter")
                .and_then(|v| v.as_u64())
                .map(|n| n % 2 != 0)
                .unwrap_or(false)
        })
        .to("mock:odd")
        .end_when()
        .end_choice()
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    h.stop().await;

    let even = h.mock().get_endpoint("even").unwrap();
    let odd = h.mock().get_endpoint("odd").unwrap();
    even.assert_exchange_count(2).await; // counters 2, 4
    odd.assert_exchange_count(2).await; // counters 1, 3
}

// Test B: otherwise fires when no when matches
#[tokio::test]
async fn choice_otherwise_fires_when_no_when_matches() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // Counter is always set — when predicate never true (impossible header).
    // All 3 ticks go to mock:fallback via otherwise.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("test-route-45")
        .choice()
        .when(|ex| ex.input.header("nonexistent").is_some())
        .to("mock:never")
        .end_when()
        .otherwise()
        .to("mock:fallback")
        .end_otherwise()
        .end_choice()
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    h.stop().await;

    let fallback = h.mock().get_endpoint("fallback").unwrap();
    fallback.assert_exchange_count(3).await;

    // The "never" endpoint must not have received anything.
    // Note: MockComponent registers endpoints during route resolution, so
    // get_endpoint("never") returns Some, but with 0 exchanges.
    if let Some(never) = h.mock().get_endpoint("never") {
        never.assert_exchange_count(0).await;
    }
}

// Test C: no match and no otherwise → exchange continues past choice
#[tokio::test]
async fn choice_no_match_no_otherwise_continues() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // when predicate never true. No otherwise. Exchange continues to mock:after.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("test-route-46")
        .choice()
        .when(|ex| ex.input.header("nonexistent").is_some())
        .to("mock:never")
        .end_when()
        .end_choice()
        .to("mock:after") // all 3 exchanges reach here
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    h.stop().await;

    let after = h.mock().get_endpoint("after").unwrap();
    after.assert_exchange_count(3).await;

    // Verify the "never" endpoint (registered during route resolution) received nothing.
    if let Some(never) = h.mock().get_endpoint("never") {
        never.assert_exchange_count(0).await;
    }
}

// Test D: short-circuit — only first matching when fires
#[tokio::test]
async fn choice_short_circuits_first_match() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // Both whens always match (|_| true). First should always win.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=4")
        .route_id("test-route-47")
        .choice()
        .when(|_ex| true)
        .to("mock:first")
        .end_when()
        .when(|_ex| true)
        .to("mock:second")
        .end_when()
        .end_choice()
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    h.stop().await;

    let first = h.mock().get_endpoint("first").unwrap();
    first.assert_exchange_count(4).await; // all 4 go to first

    // "second" must not have received anything.
    // Note: MockComponent registers endpoints during route resolution, so
    // get_endpoint("second") returns Some, but with 0 exchanges.
    if let Some(second) = h.mock().get_endpoint("second") {
        second.assert_exchange_count(0).await;
    }
}

// ---------------------------------------------------------------------------
// Test: Delay step waits at least configured duration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delay_step_waits_configured_duration() {
    use camel_api::body::Body;
    use camel_api::{Exchange, Message};
    use camel_component_direct::DirectComponent;
    use std::time::{Duration, Instant};
    use tower::util::ServiceExt;

    let direct = DirectComponent::new();
    let h = CamelTestContext::builder()
        .with_component(direct)
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:delay-in")
        .route_id("test-delay-step")
        .delay(Duration::from_millis(100))
        .to("mock:delay-out")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let producer = {
        let ctx = h.ctx().lock().await;
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry.get("direct").unwrap();
        let endpoint = component.create_endpoint("direct:delay-in", &*ctx).unwrap();
        endpoint.create_producer(&producer_ctx).unwrap()
    };

    let exchange = Exchange::new(Message::new(Body::Text("hello".to_string())));

    let started = Instant::now();
    let _ = producer.oneshot(exchange).await.unwrap();
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(100),
        "expected at least 100ms delay, got {:?}",
        elapsed
    );

    let endpoint = h.mock().get_endpoint("delay-out").unwrap();
    endpoint.await_exchanges(1, Duration::from_secs(2)).await;

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Test: XML body pipeline — direct:xml-in → convert_body_to(Text) → mock:xml-out
// ---------------------------------------------------------------------------

#[tokio::test]
async fn xml_body_pipeline() {
    use camel_api::body::Body;
    use camel_api::{Exchange, Message};
    use camel_component_direct::DirectComponent;
    use tower::util::ServiceExt;

    let direct = DirectComponent::new();
    let h = CamelTestContext::builder()
        .with_component(direct)
        .with_mock()
        .build()
        .await;

    // Route: direct:xml-in → convert_body_to(Text) → mock:xml-out
    let route = RouteBuilder::from("direct:xml-in")
        .route_id("test-xml-body-pipeline")
        .convert_body_to(camel_api::BodyType::Text)
        .to("mock:xml-out")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Give the route a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Create a producer to send an exchange with XML body
    let producer = {
        let ctx = h.ctx().lock().await;
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry.get("direct").unwrap();
        let endpoint = component.create_endpoint("direct:xml-in", &*ctx).unwrap();
        endpoint.create_producer(&producer_ctx).unwrap()
    };

    // Send exchange with Body::Xml
    let xml_content = "<root><msg>hello</msg></root>";
    let exchange = Exchange::new(Message::new(Body::Xml(xml_content.to_string())));
    let _ = producer.oneshot(exchange).await.unwrap();

    // Wait for the exchange to be processed
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    h.stop().await;

    // Assert mock received 1 exchange
    let endpoint = h.mock().get_endpoint("xml-out").unwrap();
    endpoint.assert_exchange_count(1).await;

    // Assert the received body is Body::Text (since we converted Xml→Text)
    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges[0].input.body.as_text(),
        Some(xml_content),
        "Body should be converted from Xml to Text"
    );
}

// ---------------------------------------------------------------------------
// Test: New mock assertion API — reference migration example
// ---------------------------------------------------------------------------

/// Demonstrates the new `await_exchanges` + `ExchangeAssert` API.
///
/// This is the reference example for migrating away from the old pattern:
///
/// ```
/// // OLD (fragile — fixed sleep):
/// tokio::time::sleep(Duration::from_millis(200)).await;
/// let received = mock.get_received_exchanges().await;
/// assert_eq!(received.len(), 3);
/// assert_eq!(received[0].input.body.as_text(), Some("timer://tick tick #1"));
///
/// // NEW (reliable — Notify-based):
/// ep.await_exchanges(3, Duration::from_millis(2000)).await;
/// ep.exchange(0).assert_body_text("timer://tick tick #1");
/// ```
///
/// Timer body format: `"timer://<name> tick #<n>"` (e.g. `"timer://tick tick #1"`).
#[tokio::test(flavor = "multi_thread")]
async fn mock_new_assertion_api() {
    use std::time::Duration;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("test-mock-new-assertion-api")
        .to("mock:assert-api-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let ep = h.mock().get_endpoint("assert-api-result").unwrap();

    // Wait for exactly 3 exchanges — no sleep needed.
    ep.await_exchanges(3, Duration::from_millis(2000)).await;

    // Timer body format: "timer://<name> tick #<n>"
    ep.exchange(0).assert_body_text("timer://tick tick #1");
    ep.exchange(1).assert_body_text("timer://tick tick #2");
    ep.exchange(2).assert_body_text("timer://tick tick #3");

    h.stop().await;
}
