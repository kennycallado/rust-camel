use std::time::Duration;

use camel_api::body::Body;
use camel_api::{Exchange, Message, Value};
use camel_component_api::test_support::acquire_deadline;
use camel_core::LanguageRegistryError;
use camel_dsl::parse_yaml;
use camel_language_jsonpath::JsonPathLanguage;
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(camel_component_api::NoOpComponentContext)
}

fn ensure_jsonpath_registered(ctx: &mut camel_core::CamelContext) {
    match ctx.register_language("jsonpath", Box::new(JsonPathLanguage::new())) {
        Ok(()) | Err(LanguageRegistryError::AlreadyRegistered { .. }) => {}
    }
}

async fn send_to_direct(h: &CamelTestContext, endpoint_uri: &str, exchange: Exchange) {
    let producer = {
        let ctx = acquire_deadline(
            h.ctx(),
            "camel context (send_to_direct)",
            Duration::from_secs(10),
        )
        .await;
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

#[tokio::test(flavor = "multi_thread")]
async fn jsonpath_filter_with_json_body() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await;
    ensure_jsonpath_registered(&mut guard);
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

#[tokio::test(flavor = "multi_thread")]
async fn jsonpath_set_header_from_body() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await;
    ensure_jsonpath_registered(&mut guard);
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

#[tokio::test(flavor = "multi_thread")]
async fn jsonpath_set_body_extracts_field() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await;
    ensure_jsonpath_registered(&mut guard);
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

// ── jsonpath split fragment-typing guards ───────────────────────────────────
// `$.links` over a JSON object with a string array evaluates to that array;
// each element must become a raw-text fragment (Body::Text), not a
// JSON-quoted string. Non-string elements (numbers, objects) must stay JSON.

/// Extract the raw string of a `Body::Text` fragment, panicking with the
/// observed variant on any other body type.
fn expect_raw_text<'a>(body: &'a Body, ctx: &str) -> &'a str {
    match body {
        Body::Text(t) => t,
        other => panic!("{ctx}: expected Body::Text (raw string fragment), got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jsonpath_split_string_array_yields_text_fragments() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await; // allow-test-wait: harness ctx lock for language registration — momentary, uncontended in-test (ADR-0069 §13.2 R1)
    ensure_jsonpath_registered(&mut guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "jsonpath-split-string-array"
    from: "direct:start"
    steps:
      - split:
          expression:
            jsonpath: "$.links"
          steps:
            - to: "mock:frags"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let json: serde_json::Value = serde_json::from_str(r#"{"links": ["", "a", "b"]}"#).unwrap();
    let exchange = Exchange::new(Message::new(Body::Json(json)));
    send_to_direct(&h, "direct:start", exchange).await;

    let endpoint = h.mock().get_endpoint("frags").unwrap();
    endpoint.await_exchanges(3, Duration::from_secs(2)).await;
    h.stop().await;

    // The leading empty-string element pins the accepted delta: it renders
    // as an empty Body::Text fragment, not a quoted `""` JSON string.
    let expected = ["", "a", "b"];
    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges.len(),
        3,
        "one fragment per $.links element expected"
    );
    for (i, ex) in exchanges.iter().enumerate() {
        let t = expect_raw_text(&ex.input.body, &format!("fragment {i}"));
        assert_eq!(t, expected[i], "fragment {i} must be the raw string");
        assert!(
            !t.contains('"'),
            "fragment {i} must not carry a literal quote character"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jsonpath_split_non_string_elements_stay_json() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await; // allow-test-wait: harness ctx lock for language registration — momentary, uncontended in-test (ADR-0069 §13.2 R1)
    ensure_jsonpath_registered(&mut guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "jsonpath-split-non-string"
    from: "direct:start"
    steps:
      - split:
          expression:
            jsonpath: "$.items"
          steps:
            - to: "mock:items"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let json: serde_json::Value = serde_json::from_str(r#"{"items": [41, {"k": "v"}]}"#).unwrap();
    let exchange = Exchange::new(Message::new(Body::Json(json)));
    send_to_direct(&h, "direct:start", exchange).await;

    let endpoint = h.mock().get_endpoint("items").unwrap();
    endpoint.await_exchanges(2, Duration::from_secs(2)).await;
    h.stop().await;

    let expected = [serde_json::json!(41), serde_json::json!({"k": "v"})];
    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges.len(),
        2,
        "one fragment per $.items element expected"
    );
    for (i, ex) in exchanges.iter().enumerate() {
        match &ex.input.body {
            Body::Json(v) => assert_eq!(v, &expected[i], "fragment {i} must stay JSON"),
            other => panic!("fragment {i}: expected Body::Json, got {other:?}"),
        }
    }
}
