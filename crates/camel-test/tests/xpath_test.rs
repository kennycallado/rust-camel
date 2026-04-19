use std::time::Duration;

use camel_api::body::Body;
use camel_api::{Exchange, Message, Value};
use camel_core::LanguageRegistryError;
use camel_dsl::parse_yaml;
use camel_language_xpath::XPathLanguage;
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn ensure_xpath_registered(ctx: &mut camel_core::CamelContext) {
    match ctx.register_language("xpath", Box::new(XPathLanguage)) {
        Ok(()) | Err(LanguageRegistryError::AlreadyRegistered { .. }) => {}
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
            .create_endpoint(endpoint_uri, &*ctx)
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
async fn xpath_filter_with_xml_body() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await;
    ensure_xpath_registered(&mut guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "xpath-filter"
    from: "direct:start"
    steps:
      - filter:
          xpath: "/order[@status='active']"
          steps:
            - to: "mock:filtered"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let xml = r#"<order status="active"><item>widget</item></order>"#;
    let exchange = Exchange::new(Message::new(Body::Xml(xml.to_string())));
    send_to_direct(&h, "direct:start", exchange).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("filtered").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges.len(), 1);
}

#[tokio::test]
async fn xpath_set_header_from_body() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await;
    ensure_xpath_registered(&mut guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "xpath-header"
    from: "direct:start"
    steps:
      - set_header:
          key: "bookTitle"
          xpath: "/books/book[1]/title"
      - to: "mock:header-out"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let xml = r#"<books><book><title>Rust in Action</title></book><book><title>Programming Rust</title></book></books>"#;
    let exchange = Exchange::new(Message::new(Body::Xml(xml.to_string())));
    send_to_direct(&h, "direct:start", exchange).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("header-out").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    assert_eq!(
        ex.input.header("bookTitle"),
        Some(&Value::String("Rust in Action".into())),
        "Header 'bookTitle' should be 'Rust in Action'"
    );
}

#[tokio::test]
async fn xpath_set_body_from_query() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await;
    ensure_xpath_registered(&mut guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "xpath-body"
    from: "direct:start"
    steps:
      - set_body:
          xpath: "/root/value"
      - to: "mock:body-out"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let xml = r#"<root><value>hello world</value></root>"#;
    let exchange = Exchange::new(Message::new(Body::Xml(xml.to_string())));
    send_to_direct(&h, "direct:start", exchange).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("body-out").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    match &ex.input.body {
        Body::Text(t) | Body::Xml(t) => {
            assert_eq!(t, "hello world");
        }
        other => panic!("expected text body with 'hello world', got {:?}", other),
    }
}
