//! Integration tests for the rust-camel framework.
//!
//! These tests exercise the full pipeline: CamelContext → Consumer → Processors → Producer,
//! verifying that exchanges flow end-to-end correctly.
//!
//! All assertions use the `MockComponent` shared registry instead of manual
//! capture closures.

use camel_api::Value;
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

use camel_api::{BoxProcessor, BoxProcessorExt, CamelError};
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
