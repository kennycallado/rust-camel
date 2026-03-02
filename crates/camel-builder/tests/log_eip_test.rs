//! Integration test for Log EIP (Enterprise Integration Pattern).
//!
//! Verifies that log statements work correctly within routes and that
//! exchanges flow through the pipeline correctly.

use camel_api::Value;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::CamelContext;
use camel_component_direct::DirectComponent;
use camel_component_mock::MockComponent;
use camel_processor::LogLevel;
use camel_component_timer::TimerComponent;

// ---------------------------------------------------------------------------
// Test 1: Log EIP with timer trigger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_log_eip_with_timer() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:log-test?period=50&repeatCount=1")
        .log("Starting processing", LogLevel::Info)
        .log("Debug message", LogLevel::Debug)
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.body = Body::Text("processed".into());
            Ok(ex)
        })
        .log("Finished processing", LogLevel::Info)
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();

    // Verify the exchange reached the mock endpoint
    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges[0].input.body.as_text(), Some("processed"));
}

// ---------------------------------------------------------------------------
// Test 2: Log EIP with direct component (synchronous request-reply)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_log_eip_with_direct() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(mock.clone());

    // Route 1: Timer triggers and sends to direct:process
    let trigger_route = RouteBuilder::from("timer:trigger?period=50&repeatCount=1")
        .set_header("source", Value::String("timer".into()))
        .to("direct:process")
        .build()
        .unwrap();

    // Route 2: Direct endpoint processes with logging
    let process_route = RouteBuilder::from("direct:process")
        .log("Starting processing in direct route", LogLevel::Info)
        .log("Processing exchange", LogLevel::Debug)
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.body = Body::Text("processed".into());
            Ok(ex)
        })
        .log("Finished processing in direct route", LogLevel::Info)
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(trigger_route).unwrap();
    ctx.add_route_definition(process_route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();

    // Verify the exchange reached the mock endpoint
    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges[0].input.body.as_text(), Some("processed"));
    assert_eq!(
        exchanges[0].input.header("source"),
        Some(&Value::String("timer".into()))
    );
}

// ---------------------------------------------------------------------------
// Test 3: Log EIP with filter (verify logs inside filter scope)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_log_eip_in_filter_scope() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter_clone = std::sync::Arc::clone(&counter);

    let route = RouteBuilder::from("timer:filter-log?period=50&repeatCount=2")
        .process(move |mut ex: camel_api::Exchange| {
            let c = std::sync::Arc::clone(&counter_clone);
            async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ex.input.set_header("count", Value::Number(n.into()));
                Ok(ex)
            }
        })
        .log("Before filter", LogLevel::Info)
        .filter(|ex| {
            ex.input
                .header("count")
                .and_then(|v| v.as_u64())
                .map(|n| n % 2 == 0)
                .unwrap_or(false)
        })
        .log("Inside filter - only even counts", LogLevel::Info)
        .set_header("filtered", Value::Bool(true))
        .end_filter()
        .log("After filter", LogLevel::Info)
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // All 2 exchanges should reach mock:result (filter only affects inner scope)
    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(2).await;

    let exchanges = endpoint.get_received_exchanges().await;
    // Only the first exchange (count=0) should have the "filtered" header
    assert_eq!(
        exchanges[0].input.header("filtered"),
        Some(&Value::Bool(true))
    );
    assert!(exchanges[1].input.header("filtered").is_none());
}

// ---------------------------------------------------------------------------
// Test 4: Log EIP with split (verify logs in split scope)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_log_eip_in_split_scope() {
    use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_lines};

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:split-log?period=50&repeatCount=1")
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.body = Body::Text("line1\nline2\nline3".to_string());
            Ok(ex)
        })
        .log("Before split", LogLevel::Info)
        .split(SplitterConfig::new(split_body_lines()).aggregation(AggregationStrategy::CollectAll))
        .log("Processing fragment", LogLevel::Info)
        .to("mock:per-line")
        .end_split()
        .log("After split", LogLevel::Info)
        .to("mock:final")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // Each line should have been sent to mock:per-line
    let per_line = mock.get_endpoint("per-line").unwrap();
    let per_line_count = per_line.get_received_exchanges().await.len();
    assert_eq!(per_line_count, 3, "Expected 3 per-line exchanges");

    // The aggregated result should have been sent to mock:final
    let final_ep = mock.get_endpoint("final").unwrap();
    let final_exchanges = final_ep.get_received_exchanges().await;
    assert_eq!(final_exchanges.len(), 1, "Expected 1 final exchange");
}

// ---------------------------------------------------------------------------
// Test 5: Log EIP with multiple log levels
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_log_eip_multiple_levels() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:levels?period=50&repeatCount=1")
        .log("Trace level message", LogLevel::Trace)
        .log("Debug level message", LogLevel::Debug)
        .log("Info level message", LogLevel::Info)
        .log("Warn level message", LogLevel::Warn)
        .log("Error level message", LogLevel::Error)
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();

    // Verify the exchange reached the mock endpoint
    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}
