use std::sync::Arc;
use std::time::Duration;

use camel_api::body::Body;
use camel_api::{Exchange, Message, ThrottleStrategy};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

/// ThrottleSegment must propagate Stopped (with mutations) correctly.
/// Route: direct:start -> throttle(100/s) { process(mutate body); stop(); } -> (unreachable)
/// The returned exchange should have the mutated body, proving Stop
/// propagation works per ADR-0025 §3.
#[tokio::test(flavor = "multi_thread")]
async fn stop_inside_throttle_halts_route_and_preserves_mutations() {
    let direct = DirectComponent::new();
    let h = CamelTestContext::builder()
        .with_component(direct)
        .build()
        .await;

    let route = RouteBuilder::from("direct:throttle-stop")
        .route_id("test-throttle-stop")
        .throttle(100, Duration::from_secs(1))
        .process(|mut ex: Exchange| async move {
            ex.input.body = Body::Text("throttled-mut".to_string());
            Ok(ex)
        })
        .stop()
        .end_throttle()
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Build a Direct producer to send and receive the result exchange
    let producer = {
        let ctx = h.ctx().lock().await;
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry.get("direct").unwrap();
        let endpoint = component
            .create_endpoint("direct:throttle-stop", &*ctx)
            .unwrap();
        endpoint.create_producer(test_rt(), &producer_ctx).unwrap()
    };

    let ex = Exchange::new(Message::new("trigger"));
    let result_ex = producer.oneshot(ex).await.unwrap();

    // Route halted — body preserved at the Stop point inside throttle body.
    assert_eq!(
        result_ex.input.body.as_text(),
        Some("throttled-mut"),
        "BUG: throttle Stop must preserve body mutation (got: {:?})",
        result_ex.input.body
    );

    h.stop().await;
}

/// Throttle Drop strategy must prevent an exchange from reaching downstream steps.
/// Route: direct:start -> throttle(rate=1, strategy=drop) -> to:mock:sink
/// The first exchange acquires the token and passes; the second is dropped at
/// the throttle and must never reach mock:sink.
///
/// Note: D-M8 (v1 batch1 DoS caps) made `max_requests == 0` a construction
/// error to avoid the `1.0/0.0 = inf` panic in the Delay path. So we use 1
/// token, consume it, then verify the next exchange is dropped.
#[tokio::test(flavor = "multi_thread")]
async fn throttle_drop_stops_exchange_before_sink() {
    let direct = DirectComponent::new();
    let h = CamelTestContext::builder()
        .with_component(direct)
        .with_mock()
        .build()
        .await;

    // throttle(1, 1s) with Drop: first exchange acquires the token, second is dropped.
    let route = RouteBuilder::from("direct:throttle-drop")
        .route_id("test-throttle-drop")
        .throttle(1, Duration::from_secs(1))
        .strategy(ThrottleStrategy::Drop)
        .to("mock:sink")
        .end_throttle()
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
        let endpoint = component
            .create_endpoint("direct:throttle-drop", &*ctx)
            .unwrap();
        endpoint.create_producer(test_rt(), &producer_ctx).unwrap()
    };

    // First exchange acquires the single token and reaches mock:sink.
    let ex_pass = Exchange::new(Message::new("passes"));
    let _ = producer.clone().oneshot(ex_pass).await.unwrap();

    // Second exchange: token exhausted -> Drop -> must not reach the sink.
    let ex_drop = Exchange::new(Message::new("dropped"));
    let result = producer.clone().oneshot(ex_drop).await;

    // The dropped exchange returns Ok (Stop at the Tower boundary).
    assert!(result.is_ok(), "dropped exchange must return Ok");

    // mock:sink received exactly the one exchange that passed; the dropped
    // one never arrived.
    if let Some(sink) = h.mock().get_endpoint("sink") {
        let received = sink.get_received_exchanges().await;
        assert_eq!(
            received.len(),
            1,
            "mock:sink should receive exactly 1 exchange (the passing one); the dropped exchange must not arrive"
        );
    }

    h.stop().await;
}
