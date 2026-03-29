// crates/camel-test/tests/harness_test.rs

use std::time::Duration;

use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_test::CamelTestContext;

// ---------------------------------------------------------------------------
// Test 1: Basic timer → mock via harness
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_harness_basic_timer_to_mock() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("harness-test-1")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(300)).await;

    h.mock()
        .get_endpoint("result")
        .unwrap()
        .assert_exchange_count(3)
        .await;
    // stop() called automatically by TestGuard on drop
}

// ---------------------------------------------------------------------------
// Test 2: Custom component via with_component()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_harness_with_direct_component() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(DirectComponent::new())
        .build()
        .await;

    // Sub-route: direct:sub → mock:sub-result
    let sub = RouteBuilder::from("direct:sub")
        .route_id("harness-test-2-sub")
        .to("mock:sub-result")
        .build()
        .unwrap();

    // Main route: timer → direct:sub
    let main = RouteBuilder::from("timer:tick?period=50&repeatCount=2")
        .route_id("harness-test-2-main")
        .to("direct:sub")
        .build()
        .unwrap();

    h.add_route(sub).await.unwrap();
    h.add_route(main).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(300)).await;

    h.mock()
        .get_endpoint("sub-result")
        .unwrap()
        .assert_exchange_count(2)
        .await;
}

// ---------------------------------------------------------------------------
// Test 3: Multiple routes in the same harness
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_harness_multiple_routes() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route_a = RouteBuilder::from("timer:a?period=50&repeatCount=2")
        .route_id("harness-test-3-a")
        .set_header("route", Value::String("A".into()))
        .to("mock:result-a")
        .build()
        .unwrap();

    let route_b = RouteBuilder::from("timer:b?period=50&repeatCount=3")
        .route_id("harness-test-3-b")
        .set_header("route", Value::String("B".into()))
        .to("mock:result-b")
        .build()
        .unwrap();

    h.add_route(route_a).await.unwrap();
    h.add_route(route_b).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(400)).await;

    h.mock()
        .get_endpoint("result-a")
        .unwrap()
        .assert_exchange_count(2)
        .await;
    h.mock()
        .get_endpoint("result-b")
        .unwrap()
        .assert_exchange_count(3)
        .await;
}

// ---------------------------------------------------------------------------
// Test 4: Explicit stop + drop = no panic, no double-stop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_no_double_stop_on_explicit_plus_drop() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("harness-test-4")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    h.stop().await; // explicit stop
    // drop(h) happens here — TestGuard should be a no-op (already stopped)
    // Test passes if no panic occurs
}

// ---------------------------------------------------------------------------
// Test 5: time.advance() fires timer ticks without real sleep
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_time_control_advances_timer() {
    let (h, time) = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_time_control()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("harness-test-5")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Yield first to let the timer task start and register its interval.
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    // Advance in small increments, yielding between each to allow pipeline tasks to run.
    // tokio::time::interval fires immediately on first tick, then every `period`.
    // We advance 4 × 50ms to ensure all 3 repeatCount ticks have fired.
    for _ in 0..4 {
        time.advance(Duration::from_millis(50)).await;
        for _ in 0..30 {
            tokio::task::yield_now().await;
        }
    }

    h.mock()
        .get_endpoint("result")
        .unwrap()
        .assert_exchange_count(3)
        .await;
}

// ---------------------------------------------------------------------------
// Test 6: Exact tick count — advance N×period = exactly N ticks, no extras
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_time_control_exact_tick_count() {
    let (h, time) = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_time_control()
        .build()
        .await;

    // repeatCount=10 but we only advance enough for 2 ticks
    let route = RouteBuilder::from("timer:tick?period=100&repeatCount=10")
        .route_id("harness-test-6")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Let timer task initialize, then advance exactly 2 periods.
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    for _ in 0..2 {
        time.advance(Duration::from_millis(100)).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    let ep = h.mock().get_endpoint("result").unwrap();
    let count = ep.get_received_exchanges().await.len();
    assert_eq!(
        count, 2,
        "expected exactly 2 ticks for 200ms advance with 100ms period"
    );
}

// ---------------------------------------------------------------------------
// Test 7: shutdown() performs deterministic teardown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_shutdown_consumes_and_stops() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("harness-test-7")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// Test 8: resume() lets real time flow after pause — no panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_time_control_resume() {
    let (_h, time) = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_time_control()
        .build()
        .await;

    // Just verify resume() doesn't panic
    time.resume();
}
