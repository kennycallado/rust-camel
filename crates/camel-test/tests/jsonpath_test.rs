use std::time::Duration;

use camel_api::body::Body;
use camel_api::{Exchange, Message, Value};
use camel_dsl::parse_yaml;
use camel_language_api::LanguageError;
use camel_language_jsonpath::JsonPathLanguage;
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn ensure_jsonpath_registered(ctx: &mut camel_core::CamelContext) {
    match ctx.register_language("jsonpath", Box::new(JsonPathLanguage)) {
        Ok(()) | Err(LanguageError::AlreadyRegistered(_)) => {}
        Err(e) => panic!("failed to register jsonpath language: {e}"),
    }
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
async fn jsonpath_filter_with_json_body() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await;
    ensure_jsonpath_registered(&mut *guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "jsonpath-filter"
    from: "direct:start"
    steps:
      - filter:
          jsonpath: "$.active"
          steps:
            - to: "mock:filtered"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let json: serde_json::Value =
        serde_json::from_str(r#"{"active": true, "name": "test"}"#).unwrap();
    let exchange = Exchange::new(Message::new(Body::Json(json)));
    send_to_direct(&h, "direct:start", exchange).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("filtered").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges.len(), 1);
}

#[tokio::test]
async fn jsonpath_set_header_from_body() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await;
    ensure_jsonpath_registered(&mut *guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "jsonpath-header"
    from: "direct:start"
    steps:
      - set_header:
          key: "orderId"
          jsonpath: "$.order.id"
      - to: "mock:header-out"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let json: serde_json::Value = serde_json::from_str(r#"{"order": {"id": "ORD-123"}}"#).unwrap();
    let exchange = Exchange::new(Message::new(Body::Json(json)));
    send_to_direct(&h, "direct:start", exchange).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("header-out").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    assert_eq!(
        ex.input.header("orderId"),
        Some(&Value::String("ORD-123".into())),
        "Header 'orderId' should be 'ORD-123'"
    );
}

#[tokio::test]
async fn jsonpath_set_body_extracts_field() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await;
    ensure_jsonpath_registered(&mut *guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "jsonpath-body"
    from: "direct:start"
    steps:
      - set_body:
          jsonpath: "$.items[0]"
      - to: "mock:body-out"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let json: serde_json::Value =
        serde_json::from_str(r#"{"items": ["first", "second", "third"]}"#).unwrap();
    let exchange = Exchange::new(Message::new(Body::Json(json)));
    send_to_direct(&h, "direct:start", exchange).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("body-out").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    match &ex.input.body {
        Body::Json(v) => {
            assert_eq!(v, &serde_json::Value::String("first".to_string()));
        }
        Body::Text(t) => {
            assert_eq!(t, "first");
        }
        other => panic!("expected JSON or Text body with 'first', got {:?}", other),
    }
}
