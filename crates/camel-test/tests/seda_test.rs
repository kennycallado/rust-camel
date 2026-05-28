use camel_builder::{RouteBuilder, StepAccumulator};
use camel_test::CamelTestContext;

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

    let route_a = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
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

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    h.stop().await;

    let endpoint_a = h.mock().get_endpoint("result-a").unwrap();
    let endpoint_b = h.mock().get_endpoint("result-b").unwrap();
    endpoint_a.assert_exchange_count(3).await;
    endpoint_b.assert_exchange_count(3).await;
}
