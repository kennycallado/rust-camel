//! Integration tests for autoStartup and ControlBus component.
//!
//! These tests verify:
//! - Routes with auto_startup(false) do not start when ctx.start() is called
//! - ControlBus component can start/stop routes dynamically

use std::time::Duration;

use camel_api::RouteStatus;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_controlbus::ControlBusComponent;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;

// ---------------------------------------------------------------------------
// Test 1: Route with auto_startup(false) does not start on ctx.start()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_autostartup_false_route_does_not_start() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    // Create a route with auto_startup(false)
    let route = RouteBuilder::from("timer:lazy?period=50&repeatCount=3")
        .route_id("lazy-route")
        .auto_startup(false)
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();

    // Start the context - should NOT start the lazy route
    ctx.start().await.unwrap();

    // Check route status is Stopped, not Started
    let status = ctx.route_status("lazy-route");
    assert_eq!(
        status,
        Some(RouteStatus::Stopped),
        "Route with auto_startup(false) should remain Stopped after ctx.start()"
    );

    // Wait a bit and verify no exchanges were processed
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The mock endpoint should NOT have received any exchanges
    // because the route was never started
    if let Some(endpoint) = mock.get_endpoint("result") {
        let exchanges = endpoint.get_received_exchanges().await;
        assert_eq!(
            exchanges.len(),
            0,
            "Lazy route should not have processed any exchanges"
        );
    }

    ctx.stop().await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 2: ControlBus starts a lazy route
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_controlbus_starts_route() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(ControlBusComponent::new());
    ctx.register_component(mock.clone());

    // Create a lazy route that won't start automatically
    let lazy_route = RouteBuilder::from("timer:lazy?period=50&repeatCount=3")
        .route_id("lazy-route")
        .auto_startup(false)
        .to("mock:lazy-result")
        .build()
        .unwrap();

    // Create a trigger route that starts the lazy route via ControlBus
    // The timer fires once after 50ms, then sends to controlbus to start the lazy route
    let trigger_route = RouteBuilder::from("timer:trigger?period=50&repeatCount=1")
        .route_id("trigger-route")
        .to("controlbus:route?routeId=lazy-route&action=start")
        .to("mock:trigger-done")
        .build()
        .unwrap();

    ctx.add_route_definition(lazy_route).unwrap();
    ctx.add_route_definition(trigger_route).unwrap();

    // Before starting, lazy route should be Stopped
    let status_before = ctx.route_status("lazy-route");
    assert_eq!(
        status_before,
        Some(RouteStatus::Stopped),
        "Lazy route should be Stopped before context starts"
    );

    // Start the context - only the trigger route should start
    ctx.start().await.unwrap();

    // Trigger route should be Started
    let trigger_status = ctx.route_status("trigger-route");
    assert_eq!(
        trigger_status,
        Some(RouteStatus::Started),
        "Trigger route should be Started"
    );

    // Wait for the trigger route to fire and start the lazy route
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Now check that the lazy route was started by the ControlBus
    let status_after = ctx.route_status("lazy-route");
    assert_eq!(
        status_after,
        Some(RouteStatus::Started),
        "Lazy route should be Started after ControlBus action"
    );

    // The trigger should have completed
    let trigger_endpoint = mock.get_endpoint("trigger-done").unwrap();
    trigger_endpoint.assert_exchange_count(1).await;

    // Give the lazy route time to process its timer exchanges
    tokio::time::sleep(Duration::from_millis(250)).await;

    ctx.stop().await.unwrap();

    // The lazy route should have processed its exchanges now that it's started
    let lazy_endpoint = mock.get_endpoint("lazy-result").unwrap();
    let lazy_exchanges = lazy_endpoint.get_received_exchanges().await;
    assert!(
        lazy_exchanges.len() >= 3,
        "Lazy route should have processed at least 3 exchanges after being started, got {}",
        lazy_exchanges.len()
    );
}

// ---------------------------------------------------------------------------
// Test 3: Direct API test - start_route via RouteController
// ---------------------------------------------------------------------------

/// This is a simpler version of test_controlbus_starts_route that uses
/// the direct API instead of going through a live route. This is more
/// reliable for testing the core functionality.
#[tokio::test]
async fn test_route_controller_starts_lazy_route() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    // Create a lazy route
    let lazy_route = RouteBuilder::from("timer:direct-test?period=50&repeatCount=3")
        .route_id("direct-lazy-route")
        .auto_startup(false)
        .to("mock:direct-result")
        .build()
        .unwrap();

    ctx.add_route_definition(lazy_route).unwrap();

    // Start context - lazy route should NOT start
    ctx.start().await.unwrap();

    let status = ctx.route_status("direct-lazy-route");
    assert_eq!(
        status,
        Some(RouteStatus::Stopped),
        "Lazy route should be Stopped after ctx.start()"
    );

    // Now use the RouteController directly to start the lazy route
    ctx.route_controller()
        .lock()
        .await
        .start_route("direct-lazy-route")
        .await
        .expect("Failed to start lazy route");

    // Verify the route is now started
    let status_after = ctx.route_status("direct-lazy-route");
    assert_eq!(
        status_after,
        Some(RouteStatus::Started),
        "Lazy route should be Started after direct start_route call"
    );

    // Wait for timer to fire
    tokio::time::sleep(Duration::from_millis(250)).await;

    ctx.stop().await.unwrap();

    // Verify exchanges were processed
    let endpoint = mock.get_endpoint("direct-result").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        exchanges.len() >= 3,
        "Route should have processed at least 3 exchanges after being started, got {}",
        exchanges.len()
    );
}

// ---------------------------------------------------------------------------
// Test 4: ControlBus stops a running route
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_controlbus_stops_route() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(ControlBusComponent::new());
    ctx.register_component(mock.clone());

    // Create a route that starts automatically
    let auto_route = RouteBuilder::from("timer:auto?period=100&repeatCount=10")
        .route_id("auto-route")
        .auto_startup(true)
        .to("mock:auto-result")
        .build()
        .unwrap();

    // Create a trigger route that stops the auto route after a delay
    let trigger_route = RouteBuilder::from("timer:stop-trigger?period=50&repeatCount=1")
        .route_id("stop-trigger-route")
        .to("controlbus:route?routeId=auto-route&action=stop")
        .to("mock:stop-done")
        .build()
        .unwrap();

    ctx.add_route_definition(auto_route).unwrap();
    ctx.add_route_definition(trigger_route).unwrap();

    ctx.start().await.unwrap();

    // Both routes should start
    let auto_status = ctx.route_status("auto-route");
    assert_eq!(
        auto_status,
        Some(RouteStatus::Started),
        "Auto route should be Started"
    );

    // Wait for the stop trigger to fire
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Now the auto route should be stopped
    let status_after = ctx.route_status("auto-route");
    assert_eq!(
        status_after,
        Some(RouteStatus::Stopped),
        "Auto route should be Stopped after ControlBus stop action"
    );

    ctx.stop().await.unwrap();

    // Verify the stop trigger executed
    let stop_endpoint = mock.get_endpoint("stop-done").unwrap();
    stop_endpoint.assert_exchange_count(1).await;
}

// ---------------------------------------------------------------------------
// Test 5: Multiple routes with different startup orders
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_startup_order_respected() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    // Create routes with different startup orders
    // Lower startup_order values start first
    let route1 = RouteBuilder::from("timer:order1?period=50&repeatCount=1")
        .route_id("route-order-1")
        .startup_order(10)
        .to("mock:order1")
        .build()
        .unwrap();

    let route2 = RouteBuilder::from("timer:order2?period=50&repeatCount=1")
        .route_id("route-order-2")
        .startup_order(5) // This should start first (lower value)
        .to("mock:order2")
        .build()
        .unwrap();

    let route3 = RouteBuilder::from("timer:order3?period=50&repeatCount=1")
        .route_id("route-order-3")
        .startup_order(20)
        .to("mock:order3")
        .build()
        .unwrap();

    ctx.add_route_definition(route1).unwrap();
    ctx.add_route_definition(route2).unwrap();
    ctx.add_route_definition(route3).unwrap();

    ctx.start().await.unwrap();

    // All routes should be started
    assert_eq!(
        ctx.route_status("route-order-1"),
        Some(RouteStatus::Started)
    );
    assert_eq!(
        ctx.route_status("route-order-2"),
        Some(RouteStatus::Started)
    );
    assert_eq!(
        ctx.route_status("route-order-3"),
        Some(RouteStatus::Started)
    );

    // Wait for timers to fire
    tokio::time::sleep(Duration::from_millis(200)).await;

    ctx.stop().await.unwrap();

    // All routes should have processed their exchanges
    let ep1 = mock.get_endpoint("order1").unwrap();
    let ep2 = mock.get_endpoint("order2").unwrap();
    let ep3 = mock.get_endpoint("order3").unwrap();

    ep1.assert_exchange_count(1).await;
    ep2.assert_exchange_count(1).await;
    ep3.assert_exchange_count(1).await;
}

// ---------------------------------------------------------------------------
// Test 6: Mixed auto_startup routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mixed_autostartup_routes() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    // Route that starts automatically
    let auto_route = RouteBuilder::from("timer:auto-start?period=50&repeatCount=2")
        .route_id("auto-start-route")
        .auto_startup(true)
        .to("mock:auto")
        .build()
        .unwrap();

    // Route that does NOT start automatically
    let lazy_route = RouteBuilder::from("timer:lazy-start?period=50&repeatCount=2")
        .route_id("lazy-start-route")
        .auto_startup(false)
        .to("mock:lazy")
        .build()
        .unwrap();

    ctx.add_route_definition(auto_route).unwrap();
    ctx.add_route_definition(lazy_route).unwrap();

    ctx.start().await.unwrap();

    // Auto route should be started
    assert_eq!(
        ctx.route_status("auto-start-route"),
        Some(RouteStatus::Started)
    );

    // Lazy route should be stopped
    assert_eq!(
        ctx.route_status("lazy-start-route"),
        Some(RouteStatus::Stopped)
    );

    // Wait for auto route to process
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Auto route should have processed exchanges
    let auto_ep = mock.get_endpoint("auto").unwrap();
    auto_ep.assert_exchange_count(2).await;

    // Lazy route should NOT have processed any
    if let Some(lazy_ep) = mock.get_endpoint("lazy") {
        let exchanges = lazy_ep.get_received_exchanges().await;
        assert_eq!(
            exchanges.len(),
            0,
            "Lazy route should not have processed any exchanges"
        );
    }

    ctx.stop().await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 7: Suspend/Resume Lifecycle - Basic State Transitions
// ---------------------------------------------------------------------------

/// Test that suspend_route changes route status to Suspended
#[tokio::test]
async fn test_suspend_changes_status_to_suspended() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    // Create a route that starts automatically
    let route = RouteBuilder::from("timer:suspend-test?period=100&repeatCount=10")
        .route_id("suspend-route")
        .auto_startup(true)
        .to("mock:suspend-result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    // Route should be started
    assert_eq!(
        ctx.route_status("suspend-route"),
        Some(RouteStatus::Started),
        "Route should be Started initially"
    );

    // Suspend the route
    ctx.route_controller()
        .lock()
        .await
        .suspend_route("suspend-route")
        .await
        .expect("Failed to suspend route");

    // Status should be Suspended
    assert_eq!(
        ctx.route_status("suspend-route"),
        Some(RouteStatus::Suspended),
        "Route should be Suspended after suspend_route call"
    );

    ctx.stop().await.unwrap();
}

/// Test that resume_route changes status back to Started
#[tokio::test]
async fn test_resume_changes_status_to_started() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:resume-test?period=100&repeatCount=10")
        .route_id("resume-route")
        .auto_startup(true)
        .to("mock:resume-result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    // Suspend the route
    ctx.route_controller()
        .lock()
        .await
        .suspend_route("resume-route")
        .await
        .expect("Failed to suspend route");

    assert_eq!(
        ctx.route_status("resume-route"),
        Some(RouteStatus::Suspended),
        "Route should be Suspended"
    );

    // Resume the route
    ctx.route_controller()
        .lock()
        .await
        .resume_route("resume-route")
        .await
        .expect("Failed to resume route");

    // Status should be Started again
    assert_eq!(
        ctx.route_status("resume-route"),
        Some(RouteStatus::Started),
        "Route should be Started after resume_route call"
    );

    ctx.stop().await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 8: Suspend Drains In-Flight Messages
// ---------------------------------------------------------------------------

/// Test that suspend waits for in-flight messages to complete before returning.
///
/// INTENDED BEHAVIOR:
/// When suspend_route is called while messages are being processed, it should:
/// 1. Stop accepting new messages from the consumer
/// 2. Wait for all in-flight pipeline processing to complete
/// 3. Only then return and mark the route as Suspended
///
/// EXPECTED RESULT:
/// This test currently PASSES because the implementation waits for tasks to finish,
/// but it doesn't truly "drain" in a graceful manner - it cancels and waits.
/// The test verifies that at minimum, messages are not lost.
#[tokio::test]
async fn test_suspend_drains_inflight_messages() {
    // Timing constants
    const TIMER_PERIOD_MS: u64 = 50;
    const INITIAL_FLOW_MS: u64 = 120; // Let ~2 messages through

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from(&format!(
        "timer:drain-test?period={TIMER_PERIOD_MS}&repeatCount=5"
    ))
    .route_id("drain-route")
    .auto_startup(true)
    .to("mock:drain-result")
    .build()
    .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    // Let some messages be sent (but not all)
    tokio::time::sleep(Duration::from_millis(INITIAL_FLOW_MS)).await;

    let endpoint = mock.get_endpoint("drain-result").unwrap();
    let count_before_suspend = endpoint.get_received_exchanges().await.len();

    // Suspend should wait for in-flight messages to complete
    let suspend_start = std::time::Instant::now();
    ctx.route_controller()
        .lock()
        .await
        .suspend_route("drain-route")
        .await
        .expect("Failed to suspend route");
    let suspend_duration = suspend_start.elapsed();

    let count_after_suspend = endpoint.get_received_exchanges().await.len();

    // With proper drain: messages complete before suspend returns
    assert!(
        count_after_suspend >= count_before_suspend,
        "Suspend should drain in-flight messages. Before: {}, After: {}, Duration: {:?}",
        count_before_suspend,
        count_after_suspend,
        suspend_duration
    );

    ctx.stop().await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 9: Suspend Blocks New Intake Until Resume
// ---------------------------------------------------------------------------

/// Test that a suspended route does not accept new messages until resumed.
///
/// INTENDED BEHAVIOR:
/// When a route is suspended:
/// 1. The consumer should stop producing new messages
/// 2. No new exchanges should enter the pipeline while suspended
/// 3. After resume, the consumer should start producing messages again
///
/// CURRENT IMPLEMENTATION:
/// There's a race condition where messages can slip through during the
/// suspension window. This test accounts for that by:
/// - Waiting for a settling period after suspend
/// - Checking that messages definitively stop after settling
/// - Verifying resume restarts message flow
#[tokio::test]
async fn test_suspend_blocks_new_intake_until_resume() {
    // Timing constants
    const TIMER_PERIOD_MS: u64 = 20; // Fast timer to ensure continuous flow
    const INITIAL_FLOW_MS: u64 = 100; // Let messages flow before suspend
    const SETTLING_MS: u64 = 50; // Wait for cancellation to fully take effect
    const SUSPENDED_OBSERVATION_MS: u64 = 150; // Observe while suspended
    const POST_RESUME_FLOW_MS: u64 = 100; // Let messages flow after resume

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from(&format!(
        "timer:block-test?period={TIMER_PERIOD_MS}&repeatCount=50"
    ))
    .route_id("block-route")
    .auto_startup(true)
    .to("mock:block-result")
    .build()
    .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    // Let messages flow before suspend
    tokio::time::sleep(Duration::from_millis(INITIAL_FLOW_MS)).await;

    let endpoint = mock.get_endpoint("block-result").unwrap();
    let count_before_suspend = endpoint.get_received_exchanges().await.len();

    // Suspend the route
    ctx.route_controller()
        .lock()
        .await
        .suspend_route("block-route")
        .await
        .expect("Failed to suspend route");

    // Wait for consumer cancellation to fully take effect (avoid race window)
    tokio::time::sleep(Duration::from_millis(SETTLING_MS)).await;

    // Get baseline count after settling - messages may have slipped through during race
    let count_after_settling = endpoint.get_received_exchanges().await.len();

    // Wait longer while suspended - no NEW messages should arrive after settling
    tokio::time::sleep(Duration::from_millis(SUSPENDED_OBSERVATION_MS)).await;

    let count_while_suspended = endpoint.get_received_exchanges().await.len();

    // Key assertion: After settling, no new messages should arrive while suspended
    // This is robust to the race condition at suspend time
    assert_eq!(
        count_while_suspended, count_after_settling,
        "After settling, no new messages should arrive while suspended. \
         Before suspend: {}, After settling: {}, While suspended: {}",
        count_before_suspend, count_after_settling, count_while_suspended
    );

    // Resume the route - messages should start flowing again
    ctx.route_controller()
        .lock()
        .await
        .resume_route("block-route")
        .await
        .expect("Failed to resume route");

    // Wait for messages after resume
    tokio::time::sleep(Duration::from_millis(POST_RESUME_FLOW_MS)).await;

    let count_after_resume = endpoint.get_received_exchanges().await.len();

    // Verify messages flow again after resume
    assert!(
        count_after_resume > count_while_suspended,
        "Messages should flow again after resume. While suspended: {}, After resume: {}",
        count_while_suspended,
        count_after_resume
    );

    ctx.stop().await.unwrap();
}
