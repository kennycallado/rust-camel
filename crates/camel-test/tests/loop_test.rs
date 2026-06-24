use std::sync::Arc;
use std::time::Duration;

use camel_api::body::Body;
use camel_api::{Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(camel_component_api::NoOpComponentContext)
}

async fn send_to_direct(h: &CamelTestContext, endpoint_uri: &str, exchange: Exchange) {
    let producer = {
        let ctx = h.ctx().lock().await;
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered");
        let endpoint = component
            .create_endpoint(endpoint_uri, &*ctx)
            .expect("failed to create direct endpoint");
        endpoint
            .create_producer(test_rt(), &producer_ctx)
            .expect("failed to create direct producer")
    };

    producer
        .oneshot(exchange)
        .await
        .expect("failed to send exchange to direct endpoint");
}

/// Count-mode loop via programmatic DSL.
#[tokio::test(flavor = "multi_thread")]
async fn loop_count_integration() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:loop-count")
        .route_id("loop-count-route")
        .loop_count(3)
        .process(|mut ex: Exchange| async move {
            let body = ex.input.body.as_text().unwrap_or("0");
            let counter: u64 = body.parse().unwrap_or(0);
            ex.input.body = Body::Text(format!("{}", counter + 1));
            Ok(ex)
        })
        .end_loop()
        .to("mock:loop-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    // Allow spawned consumer tasks to register their endpoints.
    // DirectConsumer::start() registers synchronously inside a tokio::spawn;
    // on slow CI runners the task may not be scheduled before send_to_direct.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let ex = Exchange::new(Message::new("0"));
    send_to_direct(&h, "direct:loop-count", ex).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let endpoint = h.mock().get_endpoint("loop-result").unwrap();
    endpoint.assert_exchange_count(1).await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges.len(), 1);
    assert_eq!(exchanges[0].input.body.as_text(), Some("3"));

    h.stop().await;
}

/// Count-mode loop via YAML DSL.
#[tokio::test(flavor = "multi_thread")]
async fn loop_count_yaml_integration() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let yaml = r#"
routes:
  - id: "loop-yaml"
    from: "direct:loop-yaml-in"
    steps:
      - set_body:
          value: "0"
      - loop:
          count: 3
          steps:
            - to: "mock:loop-yaml-out"
"#;

    let routes = camel_dsl::parse_yaml(yaml).unwrap();
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;
    // Allow spawned consumer tasks to register their endpoints.
    // DirectConsumer::start() registers synchronously inside a tokio::spawn;
    // on slow CI runners the task may not be scheduled before send_to_direct.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let ex = Exchange::new(Message::new("0"));
    send_to_direct(&h, "direct:loop-yaml-in", ex).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Loop runs 3 times, so mock should receive 3 exchanges
    h.mock()
        .get_endpoint("loop-yaml-out")
        .unwrap()
        .assert_exchange_count(3)
        .await;

    h.stop().await;
}

/// While-mode loop via programmatic DSL.
#[tokio::test(flavor = "multi_thread")]
async fn loop_while_integration() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:while-in")
        .route_id("loop-while-route")
        .loop_while(|ex: &Exchange| -> bool {
            let body = ex.input.body.as_text().unwrap_or("0");
            let n: u64 = body.parse().unwrap_or(0);
            n < 5
        })
        .process(|mut ex: Exchange| async move {
            let body = ex.input.body.as_text().unwrap_or("0");
            let n: u64 = body.parse().unwrap_or(0);
            ex.input.body = Body::Text(format!("{}", n + 1));
            Ok(ex)
        })
        .end_loop()
        .to("mock:while-out")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    // Allow spawned consumer tasks to register their endpoints.
    // DirectConsumer::start() registers synchronously inside a tokio::spawn;
    // on slow CI runners the task may not be scheduled before send_to_direct.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let ex = Exchange::new(Message::new("0"));
    send_to_direct(&h, "direct:while-in", ex).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let endpoint = h.mock().get_endpoint("while-out").unwrap();
    endpoint.assert_exchange_count(1).await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges.len(), 1);
    assert_eq!(exchanges[0].input.body.as_text(), Some("5"));

    h.stop().await;
}

/// Stop inside a loop body must propagate `PipelineOutcome::Stopped`
/// upward (halting the route) while preserving exchange mutations from the
/// iteration where Stop fired (ADR-0025 §3 invariant).
///
/// Unlike `LoopService` (Tower layer), `LoopSegment` propagates
/// `PipelineOutcome::Stopped` so that downstream
/// steps after the loop do NOT fire — the route halts at the Stop point.
#[tokio::test(flavor = "multi_thread")]
async fn stop_inside_loop_halts_route_and_preserves_iteration_mutations() {
    let direct = DirectComponent::new();
    let h = CamelTestContext::builder()
        .with_component(direct)
        .build()
        .await;

    let route = RouteBuilder::from("direct:loop-stop-halt")
        .route_id("test-loop-stop-mutations")
        .loop_count(5)
        .process(|mut ex: Exchange| async move {
            let idx = ex
                .property("CamelLoopIndex")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            ex.input.body = Body::Text(format!("iter={}", idx));
            Ok(ex)
        })
        .stop()
        .end_loop()
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
            .create_endpoint("direct:loop-stop-halt", &*ctx)
            .unwrap();
        endpoint
            .create_producer(
                Arc::new(camel_component_api::NoOpComponentContext),
                &producer_ctx,
            )
            .unwrap()
    };

    let ex = Exchange::new(Message::new("start"));
    let result_ex = producer.oneshot(ex).await.unwrap();

    // Route halted — body preserved at the Stop point inside the loop body.
    assert_eq!(
        result_ex.input.body.as_text(),
        Some("iter=0"),
        "BUG: loop-iteration Stop must preserve iteration-0 mutation (got: {:?})",
        result_ex.input.body
    );

    // No mock endpoint exists after the loop: the fact that the exchange was
    // returned as the route result (rather than being sent to a downstream
    // endpoint) proves that Stop propagated and the route halted.

    h.stop().await;
}
