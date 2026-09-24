use std::time::Duration;

use camel_api::body::Body;
use camel_api::{Exchange, Message, Value};
use camel_component_api::test_support::acquire_deadline;
use camel_core::LanguageRegistryError;
use camel_dsl::parse_yaml;
use camel_language_xpath::XPathLanguage;
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(camel_component_api::NoOpComponentContext)
}

fn ensure_xpath_registered(ctx: &mut camel_core::CamelContext) {
    match ctx.register_language("xpath", Box::new(XPathLanguage::new())) {
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

#[tokio::test(flavor = "multi_thread")]
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

#[tokio::test(flavor = "multi_thread")]
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

// ── xpath split fragment-typing guards ─────────────────────────────────────
// `//item/link` with N>1 matches evaluates to a JSON array of strings; each
// array element must become a raw-text fragment (Body::Text), not a
// JSON-quoted string. Guard (c) pins aggregation indifference: collect_all
// must produce the same byte-identical array regardless of fragment typing.

/// Extract the raw string of a `Body::Text` fragment, panicking with the
/// observed variant on any other body type.
fn expect_raw_text<'a>(body: &'a Body, ctx: &str) -> &'a str {
    match body {
        Body::Text(t) => t,
        other => panic!("{ctx}: expected Body::Text (raw link string), got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn xpath_split_three_matches_fragments_are_raw_text() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await; // allow-test-wait: harness ctx lock for language registration — momentary, uncontended in-test (ADR-0069 §13.2 R1)
    ensure_xpath_registered(&mut guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "xpath-split-three"
    from: "direct:start"
    steps:
      - split:
          expression:
            xpath: "//item/link"
          steps:
            - to: "mock:fragments"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let xml = r#"<items><item><link>https://example.org/1.xml</link></item><item><link>https://example.org/2.xml</link></item><item><link>https://example.org/3.xml</link></item></items>"#;
    let exchange = Exchange::new(Message::new(Body::Xml(xml.to_string())));
    send_to_direct(&h, "direct:start", exchange).await;

    let endpoint = h.mock().get_endpoint("fragments").unwrap();
    endpoint.await_exchanges(3, Duration::from_secs(2)).await;
    h.stop().await;

    let expected = [
        "https://example.org/1.xml",
        "https://example.org/2.xml",
        "https://example.org/3.xml",
    ];
    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges.len(),
        3,
        "one fragment per //item/link match expected"
    );
    for (i, ex) in exchanges.iter().enumerate() {
        let t = expect_raw_text(&ex.input.body, &format!("fragment {i}"));
        assert_eq!(t, expected[i], "fragment {i} must be the raw link string");
        assert!(
            !t.contains('"'),
            "fragment {i} must not carry a literal quote character"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn xpath_split_match_count_parity() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await; // allow-test-wait: harness ctx lock for language registration — momentary, uncontended in-test (ADR-0069 §13.2 R1)
    ensure_xpath_registered(&mut guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "xpath-split-parity-one"
    from: "direct:one"
    steps:
      - split:
          expression:
            xpath: "//item/link"
          steps:
            - to: "mock:parity-one"
  - id: "xpath-split-parity-many"
    from: "direct:many"
    steps:
      - split:
          expression:
            xpath: "//item/link"
          steps:
            - to: "mock:parity-many"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let one_link = r#"<items><item><link>https://example.org/bar.xml</link></item></items>"#;
    let three_links = r#"<items><item><link>https://example.org/bar.xml</link></item><item><link>https://example.org/foo.xml</link></item><item><link>https://example.org/quux.xml</link></item></items>"#;
    send_to_direct(
        &h,
        "direct:one",
        Exchange::new(Message::new(Body::Xml(one_link.to_string()))),
    )
    .await;
    send_to_direct(
        &h,
        "direct:many",
        Exchange::new(Message::new(Body::Xml(three_links.to_string()))),
    )
    .await;

    let one_ep = h.mock().get_endpoint("parity-one").unwrap();
    let many_ep = h.mock().get_endpoint("parity-many").unwrap();
    one_ep.await_exchanges(1, Duration::from_secs(2)).await;
    many_ep.await_exchanges(3, Duration::from_secs(2)).await;
    h.stop().await;

    let one_exchanges = one_ep.get_received_exchanges().await;
    let many_exchanges = many_ep.get_received_exchanges().await;
    assert_eq!(one_exchanges.len(), 1, "1-link run yields one fragment");
    assert_eq!(many_exchanges.len(), 3, "3-link run yields three fragments");

    // Fragment[0] must have the identical variant and content in both runs:
    // xpath count-dependent result shape (String vs Array[String]) must not
    // leak into fragment body typing.
    let one_first = expect_raw_text(&one_exchanges[0].input.body, "1-link run fragment[0]");
    let many_first = expect_raw_text(&many_exchanges[0].input.body, "3-link run fragment[0]");
    assert_eq!(
        one_first, "https://example.org/bar.xml",
        "1-link run fragment[0] must be the raw link string"
    );
    assert_eq!(
        many_first, "https://example.org/bar.xml",
        "3-link run fragment[0] must be the raw link string"
    );
    assert_eq!(
        one_first, many_first,
        "fragment[0] of both runs must be byte-identical"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn xpath_split_collect_all_aggregate_byte_identical() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let mut guard = h.ctx().lock().await; // allow-test-wait: harness ctx lock for language registration — momentary, uncontended in-test (ADR-0069 §13.2 R1)
    ensure_xpath_registered(&mut guard);
    drop(guard);

    let yaml = r#"
routes:
  - id: "xpath-split-collect-all"
    from: "direct:start"
    steps:
      - split:
          expression:
            xpath: "//item/link"
          aggregation: collect_all
      - to: "mock:agg"
"#;

    let routes = parse_yaml(yaml).expect("YAML parse failed");
    for route in routes {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let xml = r#"<items><item><link>https://example.org/1.xml</link></item><item><link>https://example.org/2.xml</link></item><item><link>https://example.org/3.xml</link></item></items>"#;
    let exchange = Exchange::new(Message::new(Body::Xml(xml.to_string())));
    send_to_direct(&h, "direct:start", exchange).await;

    let endpoint = h.mock().get_endpoint("agg").unwrap();
    endpoint.await_exchanges(1, Duration::from_secs(2)).await;
    h.stop().await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges.len(), 1, "collect_all emits one aggregate");
    match &exchanges[0].input.body {
        Body::Json(v) => assert_eq!(
            v.to_string(),
            r#"["https://example.org/1.xml","https://example.org/2.xml","https://example.org/3.xml"]"#,
            "aggregate must be a string array byte-identical to the link set"
        ),
        other => panic!("expected JSON array aggregate body, got {other:?}"),
    }
}
