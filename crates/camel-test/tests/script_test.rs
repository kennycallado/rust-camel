//! Integration tests for Rhai script step side effects.
//!
//! These tests verify that Rhai scripts can read AND write Exchange headers,
//! properties, and body, with changes persisting to downstream steps.

use std::time::Duration;

use camel_api::{Exchange, Message, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::LanguageRegistryError;
use camel_language_rhai::RhaiLanguage;
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn ensure_rhai_registered(ctx: &mut camel_core::CamelContext) {
    match ctx.register_language("rhai", Box::new(RhaiLanguage::new())) {
        Ok(()) | Err(LanguageRegistryError::AlreadyRegistered { .. }) => {}
    }
}

// ---------------------------------------------------------------------------
// Test 0: Script error prevents downstream delivery (CRITICAL)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_error_prevents_downstream_delivery() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    let mut guard = h.ctx().lock().await;
    ensure_rhai_registered(&mut guard);
    drop(guard);

    let route = RouteBuilder::from("direct:input-err")
        .route_id("test-script-error")
        .script("rhai", r#"headers["x"] = "modified"; throw "boom""#)
        .to("mock:error-output")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let exchange = Exchange::new(Message::new("test"));
    // send_to_direct will return an Err since the script throws - ignore the error
    let _ = send_to_direct_ignore_error(&h, "direct:input-err", exchange).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    h.stop().await;

    // Mock should NOT have received any exchanges (script threw before reaching mock)
    // Get the endpoint - it may not exist if nothing was sent to it
    if let Some(endpoint) = h.mock().get_endpoint("error-output") {
        let exchanges = endpoint.get_received_exchanges().await;
        assert_eq!(
            exchanges.len(),
            0,
            "Exchange should not reach mock when script throws"
        );
    }
    // If endpoint doesn't exist, that's also fine - means nothing was sent to it
}

// ---------------------------------------------------------------------------
// Test 1: Script sets a header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_sets_header() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    let mut guard = h.ctx().lock().await;
    ensure_rhai_registered(&mut guard);
    drop(guard);

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-header")
        .script("rhai", r#"headers["result"] = "processed""#)
        .to("mock:header-output")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Give the route a moment to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with body "hello"
    let exchange = Exchange::new(Message::new("hello"));
    send_to_direct(&h, "direct:input", exchange).await;

    h.stop().await;

    // Assert: mock received 1 exchange with header result == "processed"
    let endpoint = h.mock().get_endpoint("header-output").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    assert_eq!(
        ex.input.header("result"),
        Some(&Value::String("processed".into())),
        "Header 'result' should be 'processed'"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Script reads and transforms body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_reads_and_transforms_body() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    let mut guard = h.ctx().lock().await;
    ensure_rhai_registered(&mut guard);
    drop(guard);

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-body")
        .script("rhai", r#"body = body + "_done""#)
        .to("mock:body-output")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with body "hello"
    let exchange = Exchange::new(Message::new("hello"));
    send_to_direct(&h, "direct:input", exchange).await;

    h.stop().await;

    // Assert: mock received 1 exchange with body "hello_done"
    let endpoint = h.mock().get_endpoint("body-output").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    assert_eq!(
        ex.input.body.as_text(),
        Some("hello_done"),
        "Body should be 'hello_done'"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Script sets multiple headers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_sets_multiple_headers() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    let mut guard = h.ctx().lock().await;
    ensure_rhai_registered(&mut guard);
    drop(guard);

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-multi-headers")
        .script("rhai", r#"headers["a"] = "x"; headers["b"] = "y""#)
        .to("mock:multi-header-output")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with body "test"
    let exchange = Exchange::new(Message::new("test"));
    send_to_direct(&h, "direct:input", exchange).await;

    h.stop().await;

    // Assert: mock received 1 exchange with header a == "x" AND header b == "y"
    let endpoint = h.mock().get_endpoint("multi-header-output").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    assert_eq!(
        ex.input.header("a"),
        Some(&Value::String("x".into())),
        "Header 'a' should be 'x'"
    );
    assert_eq!(
        ex.input.header("b"),
        Some(&Value::String("y".into())),
        "Header 'b' should be 'y'"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Script reads existing header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_reads_existing_header() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    let mut guard = h.ctx().lock().await;
    ensure_rhai_registered(&mut guard);
    drop(guard);

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-read-header")
        .script("rhai", r#"headers["echo"] = headers["input"]"#)
        .to("mock:read-header-output")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with header input = "original" and any body
    let mut msg = Message::new("test body");
    msg.set_header("input", Value::String("original".into()));
    let exchange = Exchange::new(msg);
    send_to_direct(&h, "direct:input", exchange).await;

    h.stop().await;

    // Assert: mock received 1 exchange with header echo == "original"
    let endpoint = h.mock().get_endpoint("read-header-output").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    assert_eq!(
        ex.input.header("echo"),
        Some(&Value::String("original".into())),
        "Header 'echo' should be 'original'"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Script sets property
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_sets_property() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    let mut guard = h.ctx().lock().await;
    ensure_rhai_registered(&mut guard);
    drop(guard);

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-property")
        .script("rhai", r#"properties["flag"] = true"#)
        .to("mock:property-output")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with body "test"
    let exchange = Exchange::new(Message::new("test"));
    send_to_direct(&h, "direct:input", exchange).await;

    h.stop().await;

    // Assert: mock received 1 exchange with property flag == true
    let endpoint = h.mock().get_endpoint("property-output").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    assert_eq!(
        ex.properties.get("flag"),
        Some(&Value::Bool(true)),
        "Property 'flag' should be true"
    );
}

// ---------------------------------------------------------------------------
// Test 6: Unregistered language fails at route add (IMPORTANT #2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_unregistered_language_fails_at_route_add() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    // Use a language name that is guaranteed not to be registered
    let route = RouteBuilder::from("direct:input-noreg")
        .route_id("test-script-noreg")
        .script("nonexistent-lang", r#"headers["x"] = "y""#)
        .to("mock:noreg-output")
        .build()
        .unwrap();

    let result = h.add_route(route).await;
    assert!(
        result.is_err(),
        "Expected error when language not registered"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("nonexistent-lang"),
        "Error should mention the language name, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// Test 7: Empty body handled gracefully (IMPORTANT #3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_empty_body_handled() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    let mut guard = h.ctx().lock().await;
    ensure_rhai_registered(&mut guard);
    drop(guard);

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-empty-body")
        .script("rhai", r#"headers["processed"] = "yes""#)
        .to("mock:empty-body-output")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with empty body
    let exchange = Exchange::new(Message::new(""));
    send_to_direct(&h, "direct:input", exchange).await;

    h.stop().await;

    // Assert: mock received 1 exchange with header processed == "yes" and body is empty
    let endpoint = h.mock().get_endpoint("empty-body-output").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    assert_eq!(
        ex.input.header("processed"),
        Some(&Value::String("yes".into())),
        "Header 'processed' should be 'yes'"
    );
    assert_eq!(
        ex.input.body.as_text(),
        Some(""),
        "Body should be empty string"
    );
}

// ---------------------------------------------------------------------------
// Helper: Send exchange to a direct endpoint
// ---------------------------------------------------------------------------

/// Helper function to send an exchange to a direct endpoint.
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

/// Helper function to send an exchange to a direct endpoint, ignoring errors.
/// Used for tests where the script is expected to throw.
async fn send_to_direct_ignore_error(h: &CamelTestContext, endpoint_uri: &str, exchange: Exchange) {
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

    let _ = producer.oneshot(exchange).await;
}
