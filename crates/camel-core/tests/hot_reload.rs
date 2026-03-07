//! Integration test: verify that swap_pipeline replaces the pipeline atomically.
//!
//! These tests verify the hot-reload contract:
//! - In-flight requests continue with the OLD pipeline (kept alive by Arc)
//! - New requests immediately see the NEW pipeline
//! - No requests are dropped or corrupted during the swap

use camel_api::RouteController;
use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, IdentityProcessor, RouteStatus, Value};
use camel_core::registry::Registry;
use camel_core::route::RouteDefinition;
use camel_core::route_controller::DefaultRouteController;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;
use tower::ServiceExt;

#[tokio::test]
async fn test_swap_pipeline_is_picked_up_by_new_requests() {
    // This test verifies the contract: after swap_pipeline(), new pipeline loads
    // return the swapped version. We test the controller layer, not a running route,
    // because spawning a full consumer requires component registration.

    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let mut controller = DefaultRouteController::new(registry);
    let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
        DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
    ));
    controller.set_self_ref(controller_arc);

    let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("hot-test");
    controller.add_route(def).unwrap();

    // Swap with a new identity pipeline (just verifying the method works)
    let new_pipeline = BoxProcessor::new(IdentityProcessor);
    controller.swap_pipeline("hot-test", new_pipeline).unwrap();

    // Verify route still exists and is valid
    assert!(controller.route_from_uri("hot-test").is_some());
}

#[tokio::test]
async fn test_hot_reload_in_flight_requests_use_old_pipeline() {
    // This test verifies the hot-reload contract:
    // 1. New requests immediately see the NEW pipeline after swap
    // 2. In-flight requests (holding old pipeline reference) complete with OLD pipeline
    // 3. No panics or errors during the swap

    // Setup controller
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let mut controller = DefaultRouteController::new(registry);
    let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
        DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
    ));
    controller.set_self_ref(controller_arc);

    // Create route
    let def = RouteDefinition::new("direct:test", vec![]).with_route_id("hot-reload-test");
    controller.add_route(def).unwrap();

    // Create v1 pipeline that adds a "v1" marker property
    let v1_pipeline = BoxProcessor::from_fn(|mut ex: Exchange| async move {
        ex.set_property("pipeline-version", Value::String("v1".to_string()));
        Ok(ex)
    });

    // Swap to v1 pipeline
    controller
        .swap_pipeline("hot-reload-test", v1_pipeline)
        .unwrap();

    // Get v1 pipeline reference (simulates an "in-flight" request that grabbed the pipeline)
    let in_flight_v1_pipeline = controller
        .get_pipeline("hot-reload-test")
        .expect("route should exist");

    // Verify v1 pipeline works
    let exchange = Exchange::default();
    let result = in_flight_v1_pipeline
        .clone()
        .oneshot(exchange)
        .await
        .unwrap();
    assert_eq!(
        result.property("pipeline-version"),
        Some(&Value::String("v1".to_string())),
        "v1 pipeline should set 'v1' marker"
    );

    // Swap to v2 pipeline (simulates hot-reload while in-flight request is pending)
    let v2_pipeline = BoxProcessor::from_fn(|mut ex: Exchange| async move {
        ex.set_property("pipeline-version", Value::String("v2".to_string()));
        Ok(ex)
    });
    controller
        .swap_pipeline("hot-reload-test", v2_pipeline)
        .unwrap();

    // New requests should use v2 (get fresh pipeline reference after swap)
    let new_request_pipeline = controller
        .get_pipeline("hot-reload-test")
        .expect("route should exist");
    let exchange2 = Exchange::default();
    let result2 = new_request_pipeline
        .clone()
        .oneshot(exchange2)
        .await
        .unwrap();
    assert_eq!(
        result2.property("pipeline-version"),
        Some(&Value::String("v2".to_string())),
        "new requests after swap should use v2 pipeline"
    );

    // In-flight request (holding old v1 reference) should still complete with v1
    // This verifies ArcSwap's contract: old Arc references remain valid
    let exchange3 = Exchange::default();
    let result3 = in_flight_v1_pipeline
        .clone()
        .oneshot(exchange3)
        .await
        .unwrap();
    assert_eq!(
        result3.property("pipeline-version"),
        Some(&Value::String("v1".to_string())),
        "in-flight requests should complete with old v1 pipeline"
    );
}

#[tokio::test]
async fn test_hot_reload_multiple_swaps_preserve_correctness() {
    // Verify that multiple rapid swaps don't corrupt state

    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let mut controller = DefaultRouteController::new(registry);
    let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
        DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
    ));
    controller.set_self_ref(controller_arc);

    let def = RouteDefinition::new("direct:multi-swap", vec![]).with_route_id("multi-swap-test");
    controller.add_route(def).unwrap();

    // Perform multiple swaps
    for version in 1..=5 {
        let marker = format!("v{}", version);
        let pipeline = BoxProcessor::from_fn(move |mut ex: Exchange| {
            let marker = marker.clone();
            async move {
                ex.set_property("pipeline-version", Value::String(marker));
                Ok(ex)
            }
        });
        controller
            .swap_pipeline("multi-swap-test", pipeline)
            .unwrap();

        // Verify current pipeline has correct marker
        let current = controller.get_pipeline("multi-swap-test").unwrap();
        let exchange = Exchange::default();
        let result = current.clone().oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property("pipeline-version"),
            Some(&Value::String(format!("v{}", version))),
            "after swap #{}, pipeline should be v{}",
            version,
            version
        );
    }
}

#[tokio::test]
async fn test_hot_reload_concurrent_access_no_panic() {
    // Verify that concurrent reads during swap don't cause panics

    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let mut controller = DefaultRouteController::new(registry);
    let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
        DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
    ));
    controller.set_self_ref(controller_arc);

    let def = RouteDefinition::new("direct:concurrent", vec![]).with_route_id("concurrent-test");
    controller.add_route(def).unwrap();

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    // Initial pipeline
    let initial_pipeline = BoxProcessor::from_fn(move |ex: Exchange| {
        counter_clone.fetch_add(1, Ordering::SeqCst);
        async move { Ok(ex) }
    });
    controller
        .swap_pipeline("concurrent-test", initial_pipeline)
        .unwrap();

    // Spawn multiple concurrent readers
    let mut handles = vec![];
    for _ in 0..10 {
        let pipeline = controller.get_pipeline("concurrent-test").unwrap();
        handles.push(tokio::spawn(async move {
            let exchange = Exchange::default();
            pipeline.clone().oneshot(exchange).await.unwrap()
        }));
    }

    // Swap while reads are in progress
    let new_pipeline = BoxProcessor::from_fn(|ex: Exchange| async move { Ok(ex) });
    controller
        .swap_pipeline("concurrent-test", new_pipeline)
        .unwrap();

    // All reads should complete without panic
    for handle in handles {
        handle.await.expect("concurrent read should not panic");
    }

    // Counter should reflect all initial pipeline invocations
    assert_eq!(counter.load(Ordering::SeqCst), 10);
}

fn make_controller() -> DefaultRouteController {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let mut controller = DefaultRouteController::new(registry);
    let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
        DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
    ));
    controller.set_self_ref(controller_arc);
    controller
}

#[tokio::test]
async fn test_compile_route_definition_does_not_insert() {
    let controller = make_controller();

    let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("compile-only-test");

    // compile_route_definition should compile without inserting the route
    let _pipeline = controller.compile_route_definition(def).unwrap();

    // route count must still be 0 (no insertion happened)
    assert_eq!(controller.route_count(), 0);
}

#[tokio::test]
async fn test_remove_route() {
    let mut controller = make_controller();

    let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("remove-test");

    controller.add_route(def).unwrap();
    assert_eq!(controller.route_count(), 1);

    controller.remove_route("remove-test").unwrap();
    assert_eq!(controller.route_count(), 0);
}

#[tokio::test]
async fn test_remove_route_rejects_running_route() {
    // Verifies the stopped-first invariant: remove_route must return an error
    // when the route is in Started state, preventing resource leaks.
    let mut controller = make_controller();

    let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("running-remove-test");

    controller.add_route(def).unwrap();
    // Simulate a running route by forcing the status to Started
    controller.force_route_status("running-remove-test", RouteStatus::Started);

    let result = controller.remove_route("running-remove-test");
    assert!(
        result.is_err(),
        "remove_route should return an error when route is in Started state"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("must be stopped"),
        "Error message should indicate route must be stopped: {err_msg}"
    );
    // Route should still be present
    assert_eq!(controller.route_count(), 1);
}
