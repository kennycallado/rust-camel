//! Integration tests for the rust-camel framework.
//!
//! These tests exercise the full pipeline: CamelContext → Consumer → Processors → Producer,
//! verifying that exchanges flow end-to-end correctly.
//!
//! All assertions use the `MockComponent` shared registry instead of manual
//! capture closures.

use camel_api::Value;
use camel_api::aggregator::AggregatorConfig;
use camel_api::splitter::{split_body_lines, AggregationStrategy, SplitterConfig};
use camel_builder::RouteBuilder;
use camel_core::CamelContext;
use camel_file::FileComponent;
use camel_http::HttpComponent;
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

// ---------------------------------------------------------------------------
// File component integration tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test 17: File consumer → Mock (read files from directory)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_file_consumer_to_mock() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_str().unwrap();

    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    std::fs::write(dir.path().join("b.txt"), "beta").unwrap();

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(FileComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from(&format!(
        "file:{dir_path}?noop=true&initialDelay=0&delay=100"
    ))
    .to("mock:result")
    .build()
    .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("result").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(exchanges.len() >= 2, "Should have read at least 2 files, got {}", exchanges.len());

    for ex in &exchanges {
        assert!(ex.input.header("CamelFileName").is_some());
        assert!(ex.input.header("CamelFileNameOnly").is_some());
    }
}

// ---------------------------------------------------------------------------
// Test 18: Timer → File producer (write files)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_timer_to_file_producer() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_str().unwrap();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(FileComponent::new());

    let route = RouteBuilder::from("timer:write-test?period=50&repeatCount=2")
        .set_header("CamelFileName", Value::String("output.txt".into()))
        .to(&format!("file:{dir_path}?fileExist=Append"))
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    assert!(dir.path().join("output.txt").exists(), "File should have been written");
}

// ---------------------------------------------------------------------------
// Test 19: File consumer → Transform → File producer (file-to-file pipeline)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_file_to_file_pipeline() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();
    let input_path = input_dir.path().to_str().unwrap();
    let output_path = output_dir.path().to_str().unwrap();

    std::fs::write(input_dir.path().join("source.txt"), "hello world").unwrap();

    let mut ctx = CamelContext::new();
    ctx.register_component(FileComponent::new());

    let route = RouteBuilder::from(&format!(
        "file:{input_path}?noop=true&initialDelay=0&delay=100"
    ))
    .to(&format!("file:{output_path}"))
    .build()
    .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

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
async fn test_http_component_registration_and_endpoint_creation() {
    use camel_component::Component;

    let component = HttpComponent::new();

    // Verify component scheme
    assert_eq!(component.scheme(), "http");

    // Verify endpoint can be created with config
    let endpoint = component
        .create_endpoint("http://example.com/api?httpMethod=POST&connectTimeout=5000")
        .unwrap();
    assert!(endpoint.uri().contains("httpMethod=POST"));
}

// ---------------------------------------------------------------------------
// Test 21: HTTP query params are forwarded (Bug #3 verification)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_query_params_forwarding_config() {
    use camel_http::HttpConfig;

    // Verify config parsing forwards non-Camel query params
    let config = HttpConfig::from_uri(
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
async fn test_http_get_e2e() {
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
        "http://127.0.0.1:{}/api/greeting?httpMethod=GET",
        server.address().port()
    );

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(&http_uri)
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // Verify mock captured the exchange with response body
    let endpoint = mock.get_endpoint("result").unwrap();
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
async fn test_http_post_with_body_e2e() {
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
        "http://127.0.0.1:{}/api/data?httpMethod=POST",
        server.address().port()
    );

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .map_body(|_body| camel_api::body::Body::Text("payload-from-camel".into()))
        .to(&http_uri)
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("result").unwrap();
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
async fn test_http_response_headers_mapped_e2e() {
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
        "http://127.0.0.1:{}/api/headers?httpMethod=GET",
        server.address().port()
    );

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(&http_uri)
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("result").unwrap();
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
async fn test_http_error_handling_e2e() {
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
        "http://127.0.0.1:{}/api/fail?httpMethod=GET",
        server.address().port()
    );

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(&http_uri)
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // The exchange should have been routed to the DLC, not mock:result
    let dlc = mock.get_endpoint("dlc").unwrap();
    let dlc_exchanges = dlc.get_received_exchanges().await;
    assert_eq!(dlc_exchanges.len(), 1, "DLC should receive the failed exchange");
    assert!(dlc_exchanges[0].has_error(), "Exchange should carry an error");

    // mock:result should NOT have received anything
    if let Some(result) = mock.get_endpoint("result") {
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
async fn test_aggregator_collect_all() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter_clone = std::sync::Arc::clone(&counter);

    let route = RouteBuilder::from("timer:agg-test?period=1&repeatCount=9")
        .process(move |mut ex: camel_api::Exchange| {
            let c = std::sync::Arc::clone(&counter_clone);
            async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let key = ["A", "B", "C"][(n % 3) as usize];
                ex.input.headers.insert("orderId".to_string(), serde_json::json!(key));
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

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();

    // 9 exchanges total: 6 pending (Body::Empty) + 3 completed (Body::Json).
    // The aggregator emits all exchanges; pending ones have Body::Empty.
    let endpoint = mock.get_endpoint("aggregated").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    let completed: Vec<_> = exchanges
        .iter()
        .filter(|e| e.property("CamelAggregatorPending").is_none())
        .collect();
    assert_eq!(completed.len(), 3, "expected 3 completed batches, got {}", completed.len());

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
async fn test_aggregator_custom_strategy() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

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
        .process(move |mut ex: camel_api::Exchange| {
            let c = std::sync::Arc::clone(&counter_clone);
            async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ex.input.headers.insert("key".to_string(), serde_json::json!("X"));
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

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("custom-agg").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    // 4 exchanges total: 3 pending (Body::Empty) + 1 completed (Body::Text "0+1+2+3").
    let completed: Vec<_> = exchanges
        .iter()
        .filter(|e| e.property("CamelAggregatorPending").is_none())
        .collect();
    assert_eq!(completed.len(), 1, "expected 1 completed aggregate, got {}", completed.len());
    assert_eq!(completed[0].input.body.as_text(), Some("0+1+2+3"));
}

// ---------------------------------------------------------------------------
// Test 28 (Aggregator): scatter-gather — timer fires 3 times, each fire
//          is aggregated by timer name, completionSize=3,
//          expect 1 exchange at mock:scatter-gather
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_aggregator_scatter_gather() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    // Timer fires 3 times with the same timer name "scatter".
    // The aggregator groups by CamelTimerName (all 3 share key "scatter")
    // and emits when completionSize=3 is reached.
    let route = RouteBuilder::from("timer:scatter?period=10&repeatCount=3")
        .aggregate(
            AggregatorConfig::correlate_by("CamelTimerName")
                .complete_when_size(3)
                .build(),
        )
        .to("mock:scatter-gather")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // 3 exchanges: 2 pending + 1 completed (Body::Json array of 3).
    let endpoint = mock.get_endpoint("scatter-gather").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    let completed: Vec<_> = exchanges
        .iter()
        .filter(|e| e.property("CamelAggregatorPending").is_none())
        .collect();
    assert_eq!(completed.len(), 1, "expected 1 completed aggregate, got {}", completed.len());
}

// ---------------------------------------------------------------------------
// Test 26: HTTP query params forwarded in actual request to wiremock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_query_params_forwarded_e2e() {
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
        "http://127.0.0.1:{}/api/search?httpMethod=GET&apiKey=secret123&lang=rust",
        server.address().port()
    );

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(&http_uri)
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    match &exchanges[0].input.body {
        camel_api::body::Body::Bytes(b) => {
            assert_eq!(std::str::from_utf8(b).unwrap(), "found");
        }
        other => panic!("expected Body::Bytes, got {:?}", other),
    }
}
