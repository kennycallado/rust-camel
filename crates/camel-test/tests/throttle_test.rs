use std::sync::Arc;
use std::time::Duration;

use camel_api::body::Body;
use camel_api::{Exchange, Message};
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
