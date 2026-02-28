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
