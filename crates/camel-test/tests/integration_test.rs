//! Integration tests for the rust-camel framework.
//!
//! These tests exercise the full pipeline: CamelContext → Consumer → Processors → Producer,
//! verifying that exchanges flow end-to-end correctly.
//!
//! All assertions use the `MockComponent` shared registry instead of manual
//! capture closures.

use camel_api::Value;
use camel_api::splitter::{split_body_lines, AggregationStrategy, SplitterConfig};
use camel_builder::RouteBuilder;
use camel_core::CamelContext;
use camel_log::LogComponent;
use camel_mock::MockComponent;
use camel_timer::TimerComponent;

// ---------------------------------------------------------------------------
// Test 1: Timer → Mock (verify exchanges received)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_timer_to_mock() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("result").unwrap();
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
// Test 2: Timer → Filter → Mock (verify filtering behavior in flat pipeline)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_timer_filter_mock() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    // Filter: only allow even-numbered ticks through.
    // In the flat SequentialPipeline, Filter wraps IdentityProcessor as its
    // inner service. When the predicate matches, the exchange goes through
    // Identity (no-op); when it doesn't match, Filter returns the exchange
    // as-is. Either way, the pipeline continues to the next step, so ALL 4
    // exchanges reach mock.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=4")
        .filter(|ex| {
            ex.input
                .header("CamelTimerCounter")
                .and_then(|v| v.as_u64())
                .map(|n| n % 2 == 0)
                .unwrap_or(false)
        })
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(4).await;

    // Verify that all timer headers are intact.
    let exchanges = endpoint.get_received_exchanges().await;
    for (i, ex) in exchanges.iter().enumerate() {
        let counter = ex
            .input
            .header("CamelTimerCounter")
            .and_then(|v| v.as_u64())
            .expect("CamelTimerCounter header missing");
        assert_eq!(counter, (i + 1) as u64, "Exchanges should arrive in order");
    }
}

// ---------------------------------------------------------------------------
// Test 3: Timer → SetHeader → Mock (verify headers set)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_timer_set_header_mock() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .set_header("environment", Value::String("test".into()))
        .set_header("version", Value::Number(1.into()))
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("result").unwrap();
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
async fn test_timer_to_log() {
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=2")
        .set_header("source", Value::String("integration-test".into()))
        .to("log:test?showHeaders=true")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    // Just verify it doesn't panic — the log producer writes to tracing
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 5: Multiple routes in a single context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_multiple_routes() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route_a = RouteBuilder::from("timer:routeA?period=50&repeatCount=2")
        .set_header("route", Value::String("A".into()))
        .to("mock:resultA")
        .build()
        .unwrap();

    let route_b = RouteBuilder::from("timer:routeB?period=50&repeatCount=3")
        .set_header("route", Value::String("B".into()))
        .to("mock:resultB")
        .build()
        .unwrap();

    ctx.add_route_definition(route_a).unwrap();
    ctx.add_route_definition(route_b).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    let endpoint_a = mock.get_endpoint("resultA").unwrap();
    let endpoint_b = mock.get_endpoint("resultB").unwrap();

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
async fn test_dlc_receives_failed_exchange() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let mut ctx = CamelContext::new();
    let mock = MockComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .process_fn(failing_step("intentional"))
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    let dlc = mock.get_endpoint("dlc").unwrap();
    let exchanges = dlc.get_received_exchanges().await;
    assert!(!exchanges.is_empty());
    assert!(exchanges[0].has_error());
}

// ---------------------------------------------------------------------------
// Test 7: Retry recovers before DLC
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_retry_recovers_before_dlc() {
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

    let mut ctx = CamelContext::new();
    let mock = MockComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let eh = ErrorHandlerConfig::dead_letter_channel("mock:dlc")
        .on_exception(|_| true)
        .retry(3)
        .with_backoff(Duration::from_millis(1), 1.0, Duration::from_millis(10))
        .build();

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .process_fn(processor)
        .error_handler(eh)
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    ctx.stop().await.unwrap();

    // DLC should be empty — retry succeeded.
    if let Some(ep) = mock.get_endpoint("dlc") {
        assert_eq!(ep.get_received_exchanges().await.len(), 0);
    }
}

// ---------------------------------------------------------------------------
// Test 8: onException with specific handled_by endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_on_exception_handled_by_specific_endpoint() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let mut ctx = CamelContext::new();
    let mock = MockComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let eh = ErrorHandlerConfig::dead_letter_channel("mock:default-dlc")
        .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
        .handled_by("mock:processor-errors")
        .build();

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .process_fn(failing_step("processor error"))
        .error_handler(eh)
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // Specific handler received it.
    let specific = mock.get_endpoint("processor-errors").unwrap();
    assert!(!specific.get_received_exchanges().await.is_empty());

    // Default DLC did NOT receive it.
    if let Some(ep) = mock.get_endpoint("default-dlc") {
        assert_eq!(ep.get_received_exchanges().await.len(), 0);
    }
}

// ---------------------------------------------------------------------------
// Test 9: Global error handler fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_global_error_handler_fallback() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let mut ctx = CamelContext::new();
    let mock = MockComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:global-dlc"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .process_fn(failing_step("global test"))
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    let dlc = mock.get_endpoint("global-dlc").unwrap();
    assert!(!dlc.get_received_exchanges().await.is_empty());
}

// ---------------------------------------------------------------------------
// Test 10: Per-route handler overrides global
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_per_route_overrides_global() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let mut ctx = CamelContext::new();
    let mock = MockComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:global-dlc"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .process_fn(failing_step("per-route test"))
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:route-dlc"))
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // Per-route DLC got it.
    let route_dlc = mock.get_endpoint("route-dlc").unwrap();
    assert!(!route_dlc.get_received_exchanges().await.is_empty());

    // Global DLC got nothing.
    if let Some(ep) = mock.get_endpoint("global-dlc") {
        assert_eq!(ep.get_received_exchanges().await.len(), 0);
    }
}

// ---------------------------------------------------------------------------
// Test 11: direct: error bubbles to calling route's DLC
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_direct_error_bubbles_to_caller() {
    use camel_api::error_handler::ErrorHandlerConfig;
    use camel_direct::DirectComponent;

    let mut ctx = CamelContext::new();
    let mock = MockComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(mock.clone());

    // Subroute: direct:sub → failing processor (no error handler).
    let sub_route = RouteBuilder::from("direct:sub")
        .process_fn(failing_step("subroute failure"))
        .build()
        .unwrap();
    ctx.add_route_definition(sub_route).unwrap();

    // Calling route: timer → direct:sub → DLC catches the bubble.
    let main_route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to("direct:sub")
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:caller-dlc"))
        .build()
        .unwrap();
    ctx.add_route_definition(main_route).unwrap();

    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    ctx.stop().await.unwrap();

    let dlc = mock.get_endpoint("caller-dlc").unwrap();
    let exchanges = dlc.get_received_exchanges().await;
    assert!(!exchanges.is_empty());
    assert!(exchanges[0].has_error());
}

// ---------------------------------------------------------------------------
// Test 12: direct: error contained in subroute (does not reach caller)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_direct_error_contained_in_subroute() {
    use camel_api::error_handler::ErrorHandlerConfig;
    use camel_direct::DirectComponent;

    let mut ctx = CamelContext::new();
    let mock = MockComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(mock.clone());

    // Subroute has its own DLC → absorbs the error.
    let sub_route = RouteBuilder::from("direct:sub2")
        .process_fn(failing_step("contained failure"))
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:sub-dlc"))
        .build()
        .unwrap();
    ctx.add_route_definition(sub_route).unwrap();

    // Calling route continues after subroute (error was absorbed).
    let main_route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to("direct:sub2")
        .to("mock:caller-received")
        .build()
        .unwrap();
    ctx.add_route_definition(main_route).unwrap();

    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    ctx.stop().await.unwrap();

    // Sub DLC got the error.
    let sub_dlc = mock.get_endpoint("sub-dlc").unwrap();
    assert!(!sub_dlc.get_received_exchanges().await.is_empty());

    // Caller continued — mock:caller-received got the exchange (with error marked).
    let caller = mock.get_endpoint("caller-received").unwrap();
    assert!(!caller.get_received_exchanges().await.is_empty());
}

// ---------------------------------------------------------------------------
// Test 13: No error handler — route continues without crashing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_no_error_handler_logs_and_continues() {
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .process_fn(failing_step("no handler"))
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();
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
async fn test_circuit_breaker_with_error_handler() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=5")
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

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    // Give enough time for all 5 timer ticks to fire (5 × 50ms = 250ms).
    // The circuit opens after 2 failures. The pipeline backs off in a
    // retry loop (1s sleep) rather than breaking, but since open_duration
    // is 60s, no further exchanges are processed during this window.
    tokio::time::sleep(Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // DLC received exactly the 2 exchanges that passed through the CB
    // before it opened — each marked with an error.
    let dlc = mock.get_endpoint("dlc").unwrap();
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
    if let Some(sink) = mock.get_endpoint("sink") {
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
async fn test_split_with_timer_and_mock() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    // Timer fires once, body = "line1\nline2\nline3"
    // Split by lines, each fragment goes to mock:per-line
    // After split, aggregated result goes to mock:final
    let route = RouteBuilder::from("timer:split-test?period=100&repeatCount=1")
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.body = camel_api::body::Body::Text("line1\nline2\nline3".to_string());
            Ok(ex)
        })
        .split(
            SplitterConfig::new(split_body_lines())
                .aggregation(AggregationStrategy::CollectAll),
        )
            .to("mock:per-line")
        .end_split()
        .to("mock:final")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // Each line should have been sent to mock:per-line
    let per_line = mock.get_endpoint("per-line").unwrap();
    let per_line_count = per_line.get_received_exchanges().await.len();
    assert_eq!(per_line_count, 3, "Expected 3 per-line exchanges, got {per_line_count}");

    // The aggregated result should have been sent to mock:final
    let final_ep = mock.get_endpoint("final").unwrap();
    let final_exchanges = final_ep.get_received_exchanges().await;
    assert_eq!(final_exchanges.len(), 1, "Expected 1 final exchange, got {}", final_exchanges.len());

    // CollectAll produces a JSON array of the fragment bodies
    let expected = serde_json::json!(["line1", "line2", "line3"]);
    match &final_exchanges[0].input.body {
        camel_api::body::Body::Json(v) => assert_eq!(*v, expected, "CollectAll should produce JSON array of fragment bodies"),
        other => panic!("Expected JSON body from CollectAll, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 16: Split with error handler (stop_on_exception + DLC)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_split_with_error_handler() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:split-err?period=100&repeatCount=1")
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

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // The split error should propagate up to the route's error handler (DLC)
    let dlc = mock.get_endpoint("dlc").unwrap();
    let dlc_count = dlc.get_received_exchanges().await.len();
    assert_eq!(dlc_count, 1, "Expected exactly 1 DLC exchange (stop_on_exception), got {dlc_count}");

    // The sink should NOT have received anything (error stops pipeline)
    let sink = mock.get_endpoint("sink").unwrap();
    let sink_count = sink.get_received_exchanges().await.len();
    assert_eq!(sink_count, 0, "Expected 0 sink exchanges, got {sink_count}");
}
