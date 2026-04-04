use std::time::Duration;

use camel_api::{Exchange, Message};
use camel_builder::RouteBuilder;
use camel_builder::StepAccumulator;
use camel_test::CamelTestContext;
use tower::ServiceExt;

async fn send_to_direct(h: &CamelTestContext, endpoint_uri: &str, exchange: Exchange) {
    let producer = {
        let ctx = h.ctx().lock().await;
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered");
        let endpoint = component
            .create_endpoint(endpoint_uri)
            .expect("failed to create direct endpoint");
        endpoint
            .create_producer(&producer_ctx)
            .expect("failed to create direct producer")
    };

    producer
        .oneshot(exchange)
        .await
        .expect("failed to send exchange to direct endpoint");
}

#[tokio::test]
async fn json_round_trip() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:in")
        .route_id("test-marshal-roundtrip")
        .unmarshal("json")
        .marshal("json")
        .to("mock:out")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let exchange = Exchange::new(Message::new(r#"{"key":"value"}"#));
    send_to_direct(&h, "direct:in", exchange).await;

    h.stop().await;

    let endpoint = h.mock().get_endpoint("out").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    match &exchanges[0].input.body {
        camel_api::body::Body::Text(s) => {
            let parsed: serde_json::Value =
                serde_json::from_str(s).expect("body should be valid JSON");
            assert_eq!(parsed["key"], serde_json::json!("value"));
        }
        other => panic!("expected Body::Text, got {:?}", other),
    }
}

#[tokio::test]
async fn unmarshal_produces_structured_body() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:in")
        .route_id("test-marshal-structured")
        .unmarshal("json")
        .to("mock:out")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let exchange = Exchange::new(Message::new(r#"{"key":"value"}"#));
    send_to_direct(&h, "direct:in", exchange).await;

    h.stop().await;

    let endpoint = h.mock().get_endpoint("out").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    match &exchanges[0].input.body {
        camel_api::body::Body::Json(v) => {
            assert_eq!(v["key"], serde_json::json!("value"));
        }
        other => panic!("expected Body::Json, got {:?}", other),
    }
}

#[tokio::test]
async fn unmarshal_invalid_json_propagates_error() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:in")
        .route_id("test-marshal-error")
        .unmarshal("json")
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .to("mock:out")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let exchange = Exchange::new(Message::new("not json"));
    send_to_direct(&h, "direct:in", exchange).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    h.stop().await;

    let dlc = h.mock().get_endpoint("dlc").unwrap();
    let dlc_exchanges = dlc.get_received_exchanges().await;
    assert_eq!(
        dlc_exchanges.len(),
        1,
        "DLC should receive the failed exchange"
    );
    assert!(
        dlc_exchanges[0].has_error(),
        "exchange should carry an error"
    );

    if let Some(out) = h.mock().get_endpoint("out") {
        assert_eq!(
            out.get_received_exchanges().await.len(),
            0,
            "mock:out should not receive the failed exchange"
        );
    }
}
