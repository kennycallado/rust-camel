//! Task 2.4 — server consumer endpoint: wiring registries to routes.
//!
//! Covers the consumer-side spec scenarios: bind refused without a security
//! policy (through the consumer), non-loopback bind warns, tool invocations
//! served through the route, stop unregisters, resource URI registration,
//! the default 128-tool cap rejecting the 129th consumer start, and a raised
//! cap allowing 150 tools on the shared listener.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::{Body, CamelError};
use camel_component_api::consumer::ExchangeEnvelope;
use camel_component_api::{
    Component, Consumer, ConsumerContext, NoOpComponentContext, NoopRuntimeObservability,
    RuntimeObservability,
};
use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::component::McpComponent;
use camel_component_mcp::config::{McpGlobalConfig, McpServerConfig};
use camel_component_mcp::types::McpToolInvocation;
use common::warn_capture;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Trivial schema shared by the cap tests.
const TRIVIAL_SCHEMA: &str = r#"{"type":"object"}"#;

fn rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoopRuntimeObservability)
}

/// URL-encode a value for a query parameter (the DSL lowering channel).
fn encoded(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn server_config(bind: &str, policy: bool, max_tools: Option<usize>) -> McpServerConfig {
    let mut json = serde_json::json!({ "bind": bind });
    if policy {
        json["security_policy"] = serde_json::json!({ "require": "auth" });
    }
    if let Some(max_tools) = max_tools {
        json["max_tools"] = serde_json::json!(max_tools);
    }
    serde_json::from_value(json).expect("valid server config")
}

fn component_with_server(name: &str, cfg: McpServerConfig) -> McpComponent {
    let mut servers = HashMap::new();
    servers.insert(name.to_string(), cfg);
    McpComponent::new(McpGlobalConfig {
        servers,
        remotes: HashMap::new(),
    })
    .expect("config must construct")
}

/// Create the consumer for a consumer-shaped URI (endpoint + consumer).
fn consumer_for(component: &McpComponent, uri: &str) -> Box<dyn Consumer> {
    let endpoint = component
        .create_endpoint(uri, &NoOpComponentContext)
        .expect("endpoint creation must succeed");
    endpoint
        .create_consumer(rt())
        .expect("consumer creation must succeed")
}

/// A hand-owned route channel: the test acts as the route's pipeline.
fn test_context() -> (ConsumerContext, mpsc::Receiver<ExchangeEnvelope>) {
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "test-route".to_string());
    (ctx, rx)
}

#[tokio::test]
async fn consumer_start_requires_security_policy() {
    let component = component_with_server("nosec", server_config("127.0.0.10:0", false, None));
    let mut consumer = consumer_for(
        &component,
        &format!("mcp:nosec/tool/t?schema={}", encoded(TRIVIAL_SCHEMA)),
    );

    let (ctx, _route_rx) = test_context();
    let err = consumer
        .start(ctx)
        .await
        .expect_err("bind without a security policy must refuse to start");
    assert!(
        matches!(err, CamelError::Config(ref message) if message.contains("missing security policy") && message.contains("nosec")),
        "expected Config(missing security policy) naming the server, got {err}"
    );
}

#[tokio::test]
async fn non_loopback_bind_warns_at_start() {
    let captures = warn_capture();
    let component = component_with_server("exposed", server_config("0.0.0.0:0", true, None));
    let mut consumer = consumer_for(
        &component,
        &format!("mcp:exposed/tool/t?schema={}", encoded(TRIVIAL_SCHEMA)),
    );

    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("non-loopback bind with a policy must start");
    assert_eq!(
        captures.warn_count_containing("exposed"),
        1,
        "exactly one warn naming the server"
    );

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn tool_consumer_serves_invocation() {
    let bind = "127.0.0.11:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());

    let schema = r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#;
    let mut consumer = consumer_for(
        &component,
        &format!("mcp:crm/tool/lookup?schema={}", encoded(schema)),
    );

    let (ctx, mut route_rx) = test_context();
    consumer.start(ctx).await.expect("tool consumer must start");

    // Resolve the registered sender and enqueue an invocation directly.
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    let entry = handle
        .tool_registry
        .resolve("lookup")
        .expect("tool registered");
    let (reply_tx, reply_rx) = oneshot::channel();
    entry
        .sender
        .send(McpToolInvocation {
            name: "lookup".to_string(),
            arguments: serde_json::json!({ "id": "42" }),
            headers: std::collections::HashMap::new(),
            reply: reply_tx,
        })
        .await
        .expect("invocation enqueue");

    // The route: identity/set-body processor.
    let envelope = route_rx.recv().await.expect("route received the exchange");
    assert_eq!(
        envelope.exchange.input.body,
        Body::Json(serde_json::json!({ "id": "42" }))
    );
    let mut out = envelope.exchange;
    out.input.body = Body::Text("lookup-ok:42".to_string());
    envelope
        .reply_tx
        .expect("reply channel present")
        .send(Ok(out))
        .expect("route reply");

    let result = reply_rx.await.expect("tool result");
    assert_eq!(
        result.content,
        serde_json::Value::String("lookup-ok:42".to_string())
    );

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn consumer_stop_unregisters() {
    let bind = "127.0.0.12:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());

    let mut consumer = consumer_for(
        &component,
        &format!("mcp:crm/tool/lookup?schema={}", encoded(TRIVIAL_SCHEMA)),
    );
    let (ctx, _route_rx) = test_context();
    consumer.start(ctx).await.expect("tool consumer must start");
    consumer.stop().await.expect("clean stop");

    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    assert!(
        handle.tool_registry.resolve("lookup").is_none(),
        "stop must unregister the tool"
    );
}

#[tokio::test]
async fn resource_consumer_registers_uri() {
    let bind = "127.0.0.13:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());

    let mut consumer = consumer_for(&component, "mcp:crm/resource/customers?uri=crm://customers");
    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("resource consumer must start");

    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    assert!(
        handle
            .resource_registry
            .list_ready()
            .contains(&"crm://customers".to_string()),
        "ready resource URIs must contain the declared URI, got {:?}",
        handle.resource_registry.list_ready()
    );

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn consumer_start_rejects_129th_tool_with_camel_error() {
    let bind = "127.0.0.14:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());
    let (ctx, _route_rx) = test_context();

    let mut consumers = Vec::new();
    for i in 0..128 {
        let uri = format!("mcp:crm/tool/t{i}?schema={}", encoded(TRIVIAL_SCHEMA));
        let mut consumer = consumer_for(&component, &uri);
        consumer
            .start(ctx.clone())
            .await
            .unwrap_or_else(|e| panic!("consumer {i} must start: {e}"));
        consumers.push(consumer);
    }

    let mut one_too_many = consumer_for(
        &component,
        &format!("mcp:crm/tool/t128?schema={}", encoded(TRIVIAL_SCHEMA)),
    );
    let err = one_too_many
        .start(ctx.clone())
        .await
        .expect_err("the 129th tool must exceed the default cap");
    assert!(
        matches!(err, CamelError::Config(ref message) if message.contains("cap exceeded") && message.contains("tools")),
        "expected Config(cap exceeded for tools), got {err}"
    );

    for mut consumer in consumers {
        consumer.stop().await.expect("clean stop");
    }
}

#[tokio::test]
async fn raised_cap_starts_150_tool_consumers() {
    let bind = "127.0.0.15:0";
    let cfg = server_config(bind, true, Some(200));
    let component = component_with_server("crm", cfg.clone());
    let (ctx, _route_rx) = test_context();

    let mut consumers = Vec::new();
    for i in 0..150 {
        let uri = format!("mcp:crm/tool/t{i}?schema={}", encoded(TRIVIAL_SCHEMA));
        let mut consumer = consumer_for(&component, &uri);
        consumer
            .start(ctx.clone())
            .await
            .unwrap_or_else(|e| panic!("consumer {i} must start: {e}"));
        consumers.push(consumer);
    }

    for mut consumer in consumers {
        consumer.stop().await.expect("clean stop");
    }
}
