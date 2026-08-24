use camel_api::{Exchange, Message, RuntimeCommand};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::NoOpComponentContext;
use camel_test::CamelTestContext;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test(flavor = "multi_thread")]
async fn test_seda_connects_two_routes() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_seda()
        .with_mock()
        .with_log()
        .build()
        .await;

    let route_a = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("producer-route")
        .to("seda:bridge")
        .build()
        .unwrap();

    let route_b = RouteBuilder::from("seda:bridge")
        .route_id("consumer-route")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route_a).await.unwrap();
    h.add_route(route_b).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(3).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_seda_concurrent_load() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_seda()
        .with_mock()
        .with_log()
        .build()
        .await;

    let route_a = RouteBuilder::from("timer:tick?period=10&repeatCount=50")
        .route_id("producer-route")
        .to("seda:load?concurrentConsumers=4")
        .build()
        .unwrap();

    let route_b = RouteBuilder::from("seda:load?concurrentConsumers=4")
        .route_id("consumer-route")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route_a).await.unwrap();
    h.add_route(route_b).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(50).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_seda_inout_integration() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_seda()
        .with_mock()
        .with_log()
        .build()
        .await;

    let route_a = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("inout-producer")
        .set_body("hello")
        .to("seda:inout-bridge?exchangePattern=InOut")
        .build()
        .unwrap();

    let route_b = RouteBuilder::from("seda:inout-bridge?exchangePattern=InOut")
        .route_id("inout-consumer")
        .to("mock:inout-result")
        .build()
        .unwrap();

    h.add_route(route_a).await.unwrap();
    h.add_route(route_b).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("inout-result").unwrap();
    endpoint.assert_exchange_count(3).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_seda_fanout_integration() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_seda()
        .with_mock()
        .with_log()
        .build()
        .await;

    let route_a = RouteBuilder::from("timer:tick?period=50&delay=100&repeatCount=3")
        .route_id("fanout-producer")
        .to("seda:broadcast?multipleConsumers=true&timeout=3000")
        .build()
        .unwrap();

    let route_b = RouteBuilder::from("seda:broadcast?multipleConsumers=true&timeout=3000")
        .route_id("fanout-consumer-a")
        .to("mock:result-a")
        .build()
        .unwrap();

    let route_c = RouteBuilder::from("seda:broadcast?multipleConsumers=true&timeout=3000")
        .route_id("fanout-consumer-b")
        .to("mock:result-b")
        .build()
        .unwrap();

    h.add_route(route_a).await.unwrap();
    h.add_route(route_b).await.unwrap();
    h.add_route(route_c).await.unwrap();
    h.start().await;

    let endpoint_a = h.mock().get_endpoint("result-a").unwrap();
    let endpoint_b = h.mock().get_endpoint("result-b").unwrap();
    endpoint_a
        .await_exchanges(3, std::time::Duration::from_secs(3))
        .await;
    endpoint_b
        .await_exchanges(3, std::time::Duration::from_secs(3))
        .await;

    h.stop().await;

    endpoint_a.assert_exchange_count(3).await;
    endpoint_b.assert_exchange_count(3).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn seda_single_consumer_survives_context_restart() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_seda()
        .with_mock()
        .build()
        .await;

    let consumer_route = RouteBuilder::from("seda:out")
        .route_id("consumer-route")
        .to("mock:result")
        .build()
        .unwrap();

    let send_route = RouteBuilder::from("direct:in")
        .route_id("send-route")
        .to("seda:out")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(send_route).await.unwrap();
    h.start().await;

    // Stop/restart through the locked underlying context — NOT via
    // h.stop()/h.start(): h.stop() permanently latches the harness `stopped`
    // flag and would make the final teardown a no-op.
    let mut ctx = h.ctx().lock().await;
    ctx.stop().await.unwrap();
    ctx.start().await.unwrap();
    drop(ctx);

    // Send one exchange into direct:in, holding a fresh lock across the whole
    // resolve/create/send sequence.
    let ctx = h.ctx().lock().await;
    let component = ctx
        .registry()
        .get("direct")
        .expect("direct component not registered");
    let producer_ctx = ctx.producer_context();
    let endpoint = component
        .create_endpoint("direct:in", &*ctx)
        .expect("failed to create direct endpoint");
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &producer_ctx)
        .expect("failed to create direct producer");
    producer
        .oneshot(Exchange::new(Message::new("after-restart")))
        .await
        .expect("direct call should succeed");
    drop(ctx);

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint
        .await_exchanges(1, std::time::Duration::from_secs(5))
        .await;
    endpoint.assert_exchange_count(1).await;
    let received = endpoint.get_received_exchanges().await;
    assert_eq!(received[0].input.body.as_text(), Some("after-restart"));

    h.stop().await;
}

/// Suspend/resume of a default-mode (single consumer) seda route must keep
/// delivering: resume recreates the Consumer, which re-acquires the restored
/// receiver (bd rc-u0v4, pins the rc-gwvs receiver-restoration contract on
/// the resume path).
#[tokio::test(flavor = "multi_thread")]
async fn seda_single_consumer_survives_suspend_resume() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_seda()
        .with_mock()
        .build()
        .await;

    let consumer_route = RouteBuilder::from("seda:out")
        .route_id("consumer-route")
        .to("mock:result")
        .build()
        .unwrap();
    let send_route = RouteBuilder::from("direct:in")
        .route_id("send-route")
        .to("seda:out")
        .build()
        .unwrap();
    h.add_route(consumer_route).await.unwrap();
    h.add_route(send_route).await.unwrap();
    h.start().await;

    // Suspend the consumer route, then resume it: resume recreates the
    // consumer through the same endpoint state.
    {
        let ctx = h.ctx().lock().await;
        let runtime = ctx.runtime();
        drop(ctx);
        runtime
            .execute(RuntimeCommand::SuspendRoute {
                route_id: "consumer-route".to_string(),
                command_id: "test:suspend:consumer-route".to_string(),
                causation_id: None,
            })
            .await
            .expect("failed to suspend route");
        runtime
            .execute(RuntimeCommand::ResumeRoute {
                route_id: "consumer-route".to_string(),
                command_id: "test:resume:consumer-route".to_string(),
                causation_id: None,
            })
            .await
            .expect("failed to resume route");
    }

    // Send after resume. Immediate consumers start asynchronously once the
    // resume command completes (the route-status handshake does not await
    // them — bd rc-slvd), so the first send can race the consumer task and
    // hit the producer fence. Retry within a bounded window: delivery must
    // happen, and a regression that never re-activates the consumer fails
    // the deadline instead of hanging.
    {
        let ctx = h.ctx().lock().await;
        let component = ctx
            .registry()
            .get("direct")
            .expect("direct component not registered");
        let producer_ctx = ctx.producer_context();
        let endpoint = component
            .create_endpoint("direct:in", &*ctx)
            .expect("failed to create direct endpoint");
        let producer = endpoint
            .create_producer(Arc::new(NoOpComponentContext), &producer_ctx)
            .expect("failed to create direct producer");
        drop(ctx);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let ctx = h.ctx().lock().await;
            let result = producer
                .clone()
                .oneshot(Exchange::new(Message::new("after-resume")))
                .await;
            drop(ctx);
            match result {
                Ok(_) => break,
                Err(e) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "direct send after resume never succeeded: {e}"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
        }
    }

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint
        .await_exchanges(1, std::time::Duration::from_secs(5))
        .await;
    endpoint.assert_exchange_count(1).await;
    let received = endpoint.get_received_exchanges().await;
    assert_eq!(received[0].input.body.as_text(), Some("after-resume"));

    h.stop().await;
}
