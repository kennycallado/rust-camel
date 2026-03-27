//! Integration tests for Rhai script step side effects.
//!
//! These tests verify that Rhai scripts can read AND write Exchange headers,
//! properties, and body, with changes persisting to downstream steps.

use std::time::Duration;

use camel_api::{Exchange, Message, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_mock::MockComponent;
use camel_core::CamelContext;
use camel_language_rhai::RhaiLanguage;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test 0: Script error prevents downstream delivery (CRITICAL)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_script_error_prevents_downstream_delivery() {
    let mock = MockComponent::new();
    let direct = DirectComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(direct);
    ctx.register_component(mock.clone());
    ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
        .unwrap();

    let route = RouteBuilder::from("direct:input-err")
        .route_id("test-script-error")
        .script("rhai", r#"headers["x"] = "modified"; throw "boom""#)
        .to("mock:error-output")
        .build()
        .unwrap();

    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let exchange = Exchange::new(Message::new("test"));
    // send_to_direct will return an Err since the script throws - ignore the error
    let _ = send_to_direct_ignore_error(&ctx, "direct:input-err", exchange).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    ctx.stop().await.unwrap();

    // Mock should NOT have received any exchanges (script threw before reaching mock)
    // Get the endpoint - it may not exist if nothing was sent to it
    if let Some(endpoint) = mock.get_endpoint("error-output") {
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
async fn test_script_sets_header() {
    let mock = MockComponent::new();
    let direct = DirectComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(direct);
    ctx.register_component(mock.clone());
    ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
        .unwrap();

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-header")
        .script("rhai", r#"headers["result"] = "processed""#)
        .to("mock:header-output")
        .build()
        .unwrap();

    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();

    // Give the route a moment to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with body "hello"
    let exchange = Exchange::new(Message::new("hello"));
    send_to_direct(&ctx, "direct:input", exchange).await;

    ctx.stop().await.unwrap();

    // Assert: mock received 1 exchange with header result == "processed"
    let endpoint = mock.get_endpoint("header-output").unwrap();
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
async fn test_script_reads_and_transforms_body() {
    let mock = MockComponent::new();
    let direct = DirectComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(direct);
    ctx.register_component(mock.clone());
    ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
        .unwrap();

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-body")
        .script("rhai", r#"body = body + "_done""#)
        .to("mock:body-output")
        .build()
        .unwrap();

    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with body "hello"
    let exchange = Exchange::new(Message::new("hello"));
    send_to_direct(&ctx, "direct:input", exchange).await;

    ctx.stop().await.unwrap();

    // Assert: mock received 1 exchange with body "hello_done"
    let endpoint = mock.get_endpoint("body-output").unwrap();
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
async fn test_script_sets_multiple_headers() {
    let mock = MockComponent::new();
    let direct = DirectComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(direct);
    ctx.register_component(mock.clone());
    ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
        .unwrap();

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-multi-headers")
        .script("rhai", r#"headers["a"] = "x"; headers["b"] = "y""#)
        .to("mock:multi-header-output")
        .build()
        .unwrap();

    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with body "test"
    let exchange = Exchange::new(Message::new("test"));
    send_to_direct(&ctx, "direct:input", exchange).await;

    ctx.stop().await.unwrap();

    // Assert: mock received 1 exchange with header a == "x" AND header b == "y"
    let endpoint = mock.get_endpoint("multi-header-output").unwrap();
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
async fn test_script_reads_existing_header() {
    let mock = MockComponent::new();
    let direct = DirectComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(direct);
    ctx.register_component(mock.clone());
    ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
        .unwrap();

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-read-header")
        .script("rhai", r#"headers["echo"] = headers["input"]"#)
        .to("mock:read-header-output")
        .build()
        .unwrap();

    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with header input = "original" and any body
    let mut msg = Message::new("test body");
    msg.set_header("input", Value::String("original".into()));
    let exchange = Exchange::new(msg);
    send_to_direct(&ctx, "direct:input", exchange).await;

    ctx.stop().await.unwrap();

    // Assert: mock received 1 exchange with header echo == "original"
    let endpoint = mock.get_endpoint("read-header-output").unwrap();
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
async fn test_script_sets_property() {
    let mock = MockComponent::new();
    let direct = DirectComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(direct);
    ctx.register_component(mock.clone());
    ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
        .unwrap();

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-property")
        .script("rhai", r#"properties["flag"] = true"#)
        .to("mock:property-output")
        .build()
        .unwrap();

    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with body "test"
    let exchange = Exchange::new(Message::new("test"));
    send_to_direct(&ctx, "direct:input", exchange).await;

    ctx.stop().await.unwrap();

    // Assert: mock received 1 exchange with property flag == true
    let endpoint = mock.get_endpoint("property-output").unwrap();
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
async fn test_script_unregistered_language_fails_at_route_add() {
    let mock = MockComponent::new();
    let direct = DirectComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(direct);
    ctx.register_component(mock.clone());
    // Intentionally do NOT register "rhai" language

    let route = RouteBuilder::from("direct:input-noreg")
        .route_id("test-script-noreg")
        .script("rhai", r#"headers["x"] = "y""#)
        .to("mock:noreg-output")
        .build()
        .unwrap();

    // Should fail because "rhai" language is not registered
    let result = ctx.add_route_definition(route).await;
    assert!(
        result.is_err(),
        "Expected error when language not registered"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("rhai"),
        "Error should mention the language name, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// Test 7: Empty body handled gracefully (IMPORTANT #3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_script_empty_body_handled() {
    let mock = MockComponent::new();
    let direct = DirectComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(direct);
    ctx.register_component(mock.clone());
    ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
        .unwrap();

    let route = RouteBuilder::from("direct:input")
        .route_id("test-script-empty-body")
        .script("rhai", r#"headers["processed"] = "yes""#)
        .to("mock:empty-body-output")
        .build()
        .unwrap();

    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send exchange with empty body
    let exchange = Exchange::new(Message::new(""));
    send_to_direct(&ctx, "direct:input", exchange).await;

    ctx.stop().await.unwrap();

    // Assert: mock received 1 exchange with header processed == "yes" and body is empty
    let endpoint = mock.get_endpoint("empty-body-output").unwrap();
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
async fn send_to_direct(ctx: &CamelContext, endpoint_uri: &str, exchange: Exchange) {
    let producer_ctx = ctx.producer_context();
    let registry = ctx.registry();
    let component = registry
        .get("direct")
        .expect("direct component not registered");
    let endpoint = component
        .create_endpoint(endpoint_uri)
        .expect("failed to create direct endpoint");
    let producer = endpoint
        .create_producer(&producer_ctx)
        .expect("failed to create direct producer");
    drop(registry); // Release the registry lock

    producer
        .oneshot(exchange)
        .await
        .expect("failed to send exchange to direct endpoint");
}

/// Helper function to send an exchange to a direct endpoint, ignoring errors.
/// Used for tests where the script is expected to throw.
async fn send_to_direct_ignore_error(ctx: &CamelContext, endpoint_uri: &str, exchange: Exchange) {
    let producer_ctx = ctx.producer_context();
    let registry = ctx.registry();
    let component = registry
        .get("direct")
        .expect("direct component not registered");
    let endpoint = component
        .create_endpoint(endpoint_uri)
        .expect("failed to create direct endpoint");
    let producer = endpoint
        .create_producer(&producer_ctx)
        .expect("failed to create direct producer");
    drop(registry); // Release the registry lock

    let _ = producer.oneshot(exchange).await;
}
