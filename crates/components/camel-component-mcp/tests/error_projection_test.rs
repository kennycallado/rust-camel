//! Task 4.1 — tool failures project through the ADR-0097 envelope.
//!
//! The tool bridge (`spawn_tool_bridge`) answers every genuine failure with a
//! client-safe payload: business errors keep only `code`/`kind`, everything
//! else collapses to the generic `Internal Server Error` body, and the detail
//! lives in a WARN log only. Wire-level, against the real rmcp server (the
//! dispatch harness in `server_tool_dispatch_test.rs` bypasses the bridge and
//! cannot reach it).

mod common;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange, StreamBody};
use camel_component_api::consumer::ExchangeEnvelope;
use camel_component_api::{
    Component, Consumer, ConsumerContext, NoOpComponentContext, NoopRuntimeObservability,
    RuntimeObservability,
};
use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::adapter::RmcpClient;
use camel_component_mcp::client::McpClient;
use camel_component_mcp::component::McpComponent;
use camel_component_mcp::config::{
    McpGlobalConfig, McpRemoteConfig, McpServerConfig, McpTransport,
};
use camel_component_mcp::types::McpToolResult;
use common::{raw_rmcp_client, warn_capture};
use rmcp::ServiceError;
use rmcp::model::CallToolRequestParams;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

/// The YAML route document: one `mcp:` block with the `lookup` tool
/// (`{"id": string}` required).
fn lookup_yaml(bind: &str) -> String {
    format!(
        r#"
mcp:
  - server:
      name: crm
      bind: {bind}
      security_policy:
        roles: [mcp-client]
    tools:
      - name: lookup
        input_schema:
          type: object
          properties:
            id:
              type: string
          required: [id]
"#
    )
}

/// The YAML route document: one `mcp:` block with the `blob` tool (no
/// required arguments).
fn blob_yaml(bind: &str) -> String {
    format!(
        r#"
mcp:
  - server:
      name: crm
      bind: {bind}
      security_policy:
        roles: [mcp-client]
    tools:
      - name: blob
        input_schema:
          type: object
"#
    )
}

/// The YAML route document: one `mcp:` block with a single resource
/// (`name` at `uri`).
fn resource_yaml(bind: &str, name: &str, uri: &str) -> String {
    format!(
        r#"
mcp:
  - server:
      name: crm
      bind: {bind}
      security_policy:
        roles: [mcp-client]
    resources:
      - name: {name}
        uri: {uri}
"#
    )
}

/// The YAML route document: one `mcp:` block with the `big-value` tool
/// (`id` is a string capped at 10 characters).
fn big_value_yaml(bind: &str) -> String {
    format!(
        r#"
mcp:
  - server:
      name: crm
      bind: {bind}
      security_policy:
        roles: [mcp-client]
    tools:
      - name: big-value
        input_schema:
          type: object
          properties:
            id:
              type: string
              maxLength: 10
"#
    )
}

/// The YAML route document: one `mcp:` block with the `inventer` tool
/// (`additionalProperties: false` plus a `^x-` pattern).
fn inventer_yaml(bind: &str) -> String {
    format!(
        r#"
mcp:
  - server:
      name: crm
      bind: {bind}
      security_policy:
        roles: [mcp-client]
    tools:
      - name: inventer
        input_schema:
          type: object
          properties:
            id:
              type: string
          additionalProperties: false
          patternProperties:
            "^x-":
              type: string
"#
    )
}

/// The YAML route document: one `mcp:` block with the `falsifier` tool whose
/// `value` property has the `false` subschema.
fn falsifier_yaml(bind: &str) -> String {
    format!(
        r#"
mcp:
  - server:
      name: crm
      bind: {bind}
      security_policy:
        roles: [mcp-client]
    tools:
      - name: falsifier
        input_schema:
          type: object
          properties:
            value: false
"#
    )
}

/// The YAML route document: one `mcp:` block with the `capped` tool declaring
/// 30 required properties so a call with `{}` produces 30 violations.
fn capped_yaml(bind: &str) -> String {
    let required = (1..=30)
        .map(|i| format!("a{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
mcp:
  - server:
      name: crm
      bind: {bind}
      security_policy:
        roles: [mcp-client]
    tools:
      - name: capped
        input_schema:
          type: object
          properties:
            a1:
              type: string
          required: [{required}]
"#
    )
}

fn rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoopRuntimeObservability)
}

fn server_cfg(bind: &str) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({
        "bind": bind,
        "security_policy": {"require": "auth"},
    }))
    .expect("valid server config")
}

/// A component configured with one `crm` server on `cfg.bind`.
fn component_with_server(name: &str, cfg: &McpServerConfig) -> McpComponent {
    let mut servers = HashMap::new();
    servers.insert(name.to_string(), cfg.clone());
    McpComponent::new(McpGlobalConfig {
        servers,
        remotes: HashMap::new(),
    })
}

/// Create the consumer for a lowered consumer-shaped `from` URI.
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

/// Lower `yaml`, start the tool consumer for `route_id`, and mount the shared
/// listener. Returns the consumer (keep it alive), the route's envelope
/// receiver, and the listener address.
async fn start_tool_route(
    bind: &str,
    yaml: &str,
    route_id: &str,
) -> (
    Box<dyn Consumer>,
    mpsc::Receiver<ExchangeEnvelope>,
    SocketAddr,
) {
    let cfg = server_cfg(bind);
    let routes = camel_dsl::parse_yaml_to_declarative(yaml).expect("the mcp block must lower");
    let route = routes
        .iter()
        .find(|r| r.route_id == route_id)
        .expect("the tool consumer route must be lowered");

    let component = component_with_server("crm", &cfg);
    let mut consumer = consumer_for(&component, &route.from);
    let (ctx, rx) = test_context();
    consumer.start(ctx).await.expect("tool consumer must start");

    let addr = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener")
        .local_addr;
    (consumer, rx, addr)
}

/// Lower `yaml`, start the resource consumer for `route_id`, and mount the
/// shared listener. Returns the consumer (keep it alive), the route's envelope
/// receiver, and the listener address.
async fn start_resource_route(
    bind: &str,
    yaml: &str,
    route_id: &str,
) -> (
    Box<dyn Consumer>,
    mpsc::Receiver<ExchangeEnvelope>,
    SocketAddr,
) {
    let cfg = server_cfg(bind);
    let routes = camel_dsl::parse_yaml_to_declarative(yaml).expect("the mcp block must lower");
    let route = routes
        .iter()
        .find(|r| r.route_id == route_id)
        .expect("the resource consumer route must be lowered");

    let component = component_with_server("crm", &cfg);
    let mut consumer = consumer_for(&component, &route.from);
    let (ctx, rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("resource consumer must start");

    let addr = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener")
        .local_addr;
    (consumer, rx, addr)
}

/// The component's own client for `tools/call`.
async fn component_client(addr: SocketAddr) -> RmcpClient {
    RmcpClient::connect(
        "error-projection-test",
        &McpRemoteConfig {
            url: format!("http://{addr}/mcp"),
            transport: McpTransport::StreamableHttp,
            allow_internal: true, // tests bind loopback
            allow_cleartext: false,
            request_timeout_secs: 120,
            connect_timeout_secs: 15,
        },
    )
    .await
    .expect("component client must connect")
}

/// The single text block's payload of a tool result.
fn payload(result: &McpToolResult) -> &str {
    result.content[0]["text"]
        .as_str()
        .expect("tool result must carry exactly one text content block")
}

/// A bounded `recv` on the route's envelope channel.
async fn next_envelope(rx: &mut mpsc::Receiver<ExchangeEnvelope>) -> ExchangeEnvelope {
    timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("route must receive the invocation within 2s")
        .expect("route channel alive")
}

#[tokio::test]
async fn business_failure_uses_adr0097_envelope() {
    let bind = "127.0.0.61:0";
    // The sink is process-wide: install BEFORE any client request so the
    // event is never emitted into an unrecorded subscriber.
    let warns = warn_capture();

    let (mut consumer, mut rx, addr) =
        start_tool_route(bind, &lookup_yaml(bind), "mcp-crm-tool-lookup").await;

    let route = tokio::spawn(async move {
        let envelope = next_envelope(&mut rx).await;
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Err(CamelError::Business {
                code: "ORDER_NOT_FOUND".to_string(),
                kind: "not-found".to_string(),
                message: "customer 42 is log-only".to_string(),
                data: None,
            }))
            .expect("tool route reply");
    });

    let result = component_client(addr)
        .await
        .call_tool("lookup", serde_json::json!({ "id": "42" }))
        .await
        .expect("tools/call must answer a projected failure");

    assert!(
        result.is_error,
        "a business failure must set the structured is_error flag"
    );
    assert_eq!(
        payload(&result),
        r#"{"error":"ORDER_NOT_FOUND","kind":"not-found"}"#,
        "business failures must project to the ADR-0097 code/kind envelope"
    );
    let serialized = result.content.to_string();
    assert!(
        !serialized.contains("customer 42"),
        "the exchange-derived business message must never reach the client, got: {serialized}"
    );
    assert!(
        !serialized.contains("\"message\""),
        "the envelope must not carry a message key, got: {serialized}"
    );
    assert!(
        !serialized.contains("\"data\""),
        "the envelope must not carry a data key, got: {serialized}"
    );
    assert!(
        warns.has_warn_containing("tool dispatch failed"),
        "the business failure detail must be logged"
    );

    route.await.expect("route task must finish"); // allow-test-wait: route task join bounded by inner 2s recv timeout
    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn technical_failure_body_is_generic() {
    let bind = "127.0.0.62:0";
    let warns = warn_capture();

    let (mut consumer, mut rx, addr) =
        start_tool_route(bind, &lookup_yaml(bind), "mcp-crm-tool-lookup").await;

    // The production leak shape from `camel-http/src/ssrf.rs:373`: the URL,
    // the `ssrf` wording, and upstream type names.
    let route = tokio::spawn(async move {
        let envelope = next_envelope(&mut rx).await;
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Err(CamelError::ProcessorError(
                "HTTP request failed: error sending request for url (http://10.0.0.138:9/setup): \
                 ssrf guard: ConnectError: tcp connect error"
                    .to_string(),
            )))
            .expect("tool route reply");
    });

    let result = component_client(addr)
        .await
        .call_tool("lookup", serde_json::json!({ "id": "42" }))
        .await
        .expect("tools/call must answer a projected failure");

    assert!(
        result.is_error,
        "a technical failure must set the structured is_error flag"
    );
    assert_eq!(
        payload(&result),
        r#"{"error":"Internal Server Error"}"#,
        "a technical failure body must be the generic constant"
    );
    let serialized = result.content.to_string();
    assert!(
        !serialized.contains("http://"),
        "the failure URL must never reach the client, got: {serialized}"
    );
    assert!(
        !serialized.contains("ssrf"),
        "the guard wording must never reach the client, got: {serialized}"
    );
    assert!(
        !serialized.contains("ConnectError"),
        "upstream type names must never reach the client, got: {serialized}"
    );
    assert!(
        warns.has_warn_field_containing("tool dispatch failed", "http://10.0.0.138:9"),
        "the dispatch WARN event must carry the failure detail in its error field"
    );

    route.await.expect("route task must finish"); // allow-test-wait: route task join bounded by inner 2s recv timeout
    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn propagated_exchange_error_uses_same_projection() {
    let bind = "127.0.0.63:0";
    let warns = warn_capture();

    let (mut consumer, mut rx, addr) =
        start_tool_route(bind, &lookup_yaml(bind), "mcp-crm-tool-lookup").await;

    // The same `Ok(out)` arm a real route lands in: the pipeline completed but
    // left a propagated error on the Exchange output.
    let route = tokio::spawn(async move {
        let envelope = next_envelope(&mut rx).await;
        let mut out = envelope.exchange;
        out.error = Some(CamelError::Business {
            code: "SOLD_OUT".to_string(),
            kind: "failed".to_string(),
            message: String::new(),
            data: None,
        });
        out.input.body = Body::Text("ignored".to_string());
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("first call reply");

        let envelope = next_envelope(&mut rx).await;
        let mut out = envelope.exchange;
        out.error = Some(CamelError::ProcessorError(
            "HTTP request failed: error sending request for url (http://10.1.1.1:2/x): ssrf guard"
                .to_string(),
        ));
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("second call reply");

        let envelope = next_envelope(&mut rx).await;
        let mut out: Exchange = envelope.exchange;
        out.error = None;
        out.input.body = Body::Text("ok".to_string());
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("third call reply");
    });

    let client = component_client(addr).await;

    // (1) Propagated business error: projected, body ignored.
    let first = client
        .call_tool("lookup", serde_json::json!({ "id": "1" }))
        .await
        .expect("first tools/call must answer");
    assert!(
        first.is_error,
        "a propagated business error must set is_error"
    );
    assert_eq!(
        payload(&first),
        r#"{"error":"SOLD_OUT","kind":"failed"}"#,
        "a propagated business error must project to the ADR-0097 envelope"
    );

    // (2) Propagated technical error: generic body.
    let second = client
        .call_tool("lookup", serde_json::json!({ "id": "2" }))
        .await
        .expect("second tools/call must answer");
    assert!(
        second.is_error,
        "a propagated technical error must set is_error"
    );
    assert_eq!(
        payload(&second),
        r#"{"error":"Internal Server Error"}"#,
        "a propagated technical error body must be the generic constant"
    );
    assert!(
        !second.content.to_string().contains("http://"),
        "the propagated failure URL must never reach the client"
    );

    // (3) Handled-and-cleared: success stays success.
    let third = client
        .call_tool("lookup", serde_json::json!({ "id": "3" }))
        .await
        .expect("third tools/call must answer");
    assert!(!third.is_error, "a cleared error must not set is_error");
    assert_eq!(
        payload(&third),
        "ok",
        "a successful route body must pass through untouched"
    );

    assert!(
        warns.has_warn_field_containing("tool dispatch failed", "http://10.1.1.1:2"),
        "the propagated dispatch WARN event must carry the failure detail"
    );

    route.await.expect("route task must finish"); // allow-test-wait: route task join bounded by inner 2s recv timeout
    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn resource_failure_uses_same_projection() {
    let bind = "127.0.0.65:0";
    let warns = warn_capture();

    let (mut consumer, mut rx, addr) = start_resource_route(
        bind,
        &resource_yaml(bind, "customers", "crm://customers"),
        "mcp-crm-resource-customers",
    )
    .await;

    // Three reads drive all three resource failure arms: the `Err(e)` reply,
    // the carried `Ok(out)` error, and the client-visible projection.
    let route = tokio::spawn(async move {
        let envelope = next_envelope(&mut rx).await;
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Err(CamelError::Business {
                code: "DATA_LOCKED".to_string(),
                kind: "conflict".to_string(),
                message: String::new(),
                data: None,
            }))
            .expect("first read reply");

        let envelope = next_envelope(&mut rx).await;
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Err(CamelError::ProcessorError(
                "HTTP request failed: error sending request for url \
                 (http://10.9.9.9:1/customers): ssrf guard"
                    .to_string(),
            )))
            .expect("second read reply");

        let envelope = next_envelope(&mut rx).await;
        let mut out = envelope.exchange;
        out.error = Some(CamelError::Business {
            code: "STALE".to_string(),
            kind: "failed".to_string(),
            message: String::new(),
            data: None,
        });
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("third read reply");
    });

    let client = component_client(addr).await;

    // (1) Direct bridge failure: business envelope.
    let first = client
        .read_resource("crm://customers")
        .await
        .expect("resources/read must answer a projected failure");
    assert_eq!(
        first.mime_type, "application/json",
        "a projected resource failure must declare the JSON MIME type"
    );
    assert_eq!(
        String::from_utf8(first.content).expect("resource content must be utf-8"),
        r#"{"error":"DATA_LOCKED","kind":"conflict"}"#,
        "business failures must project to the ADR-0097 code/kind envelope"
    );

    // (2) Technical failure: generic body, detail only in the WARN event.
    let second = client
        .read_resource("crm://customers")
        .await
        .expect("resources/read must answer a projected failure");
    assert_eq!(
        second.mime_type, "application/json",
        "a projected resource failure must declare the JSON MIME type"
    );
    let body = String::from_utf8(second.content).expect("resource content must be utf-8");
    assert_eq!(
        body, r#"{"error":"Internal Server Error"}"#,
        "a technical resource failure body must be the generic constant"
    );
    assert!(
        !body.contains("http://"),
        "the failure URL must never reach the client, got: {body}"
    );
    assert!(
        warns.has_warn_field_containing("resource read failed", "http://10.9.9.9:1"),
        "the resource read WARN event must carry the failure detail in its error field"
    );

    // (3) Carried `Ok(out)` error: same projection, body ignored.
    let third = client
        .read_resource("crm://customers")
        .await
        .expect("resources/read must answer a projected failure");
    assert_eq!(
        third.mime_type, "application/json",
        "a projected resource failure must declare the JSON MIME type"
    );
    assert_eq!(
        String::from_utf8(third.content).expect("resource content must be utf-8"),
        r#"{"error":"STALE","kind":"failed"}"#,
        "a carried resource error must project to the ADR-0097 envelope"
    );

    route.await.expect("route task must finish"); // allow-test-wait: route task join bounded by inner 2s recv timeout
    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn resource_materialization_failure_is_generic() {
    let bind = "127.0.0.66:0";
    let warns = warn_capture();

    let (mut consumer, mut rx, addr) = start_resource_route(
        bind,
        &resource_yaml(bind, "blob", "crm://blob"),
        "mcp-crm-resource-blob",
    )
    .await;

    // 11 MiB over the 10 MiB `MAX_STREAM_BODY_BYTES`: the resource body's
    // materialization step fails with `CamelError::StreamLimitExceeded`.
    let route = tokio::spawn(async move {
        let envelope = next_envelope(&mut rx).await;
        let chunks: Vec<Result<bytes::Bytes, CamelError>> =
            vec![Ok(bytes::Bytes::from_static(&[b'x'; 1024])); 11_000];
        let mut out = envelope.exchange;
        out.input.body = Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(
                futures::stream::iter(chunks),
            )))),
            metadata: Default::default(),
        });
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("resource route reply");
    });

    let resource = component_client(addr)
        .await
        .read_resource("crm://blob")
        .await
        .expect("resources/read must answer a projected materialization failure");

    assert_eq!(
        resource.mime_type, "application/json",
        "a projected materialization failure must declare the JSON MIME type"
    );
    let body = String::from_utf8(resource.content).expect("resource content must be utf-8");
    assert_eq!(
        body, r#"{"error":"Internal Server Error"}"#,
        "a materialization failure body must be the generic constant"
    );
    assert!(
        !body.contains("Stream size exceeded limit"),
        "the stream limit detail must never reach the client, got: {body}"
    );
    assert!(
        warns.has_warn_field_containing(
            "resource body materialization failed",
            "Stream size exceeded limit"
        ),
        "the materialization WARN event must carry the limit detail in its error field"
    );

    route.await.expect("route task must finish"); // allow-test-wait: route task join bounded by inner 2s recv timeout
    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn body_to_json_materialization_failure_is_generic() {
    let bind = "127.0.0.64:0";
    let warns = warn_capture();

    let (mut consumer, mut rx, addr) =
        start_tool_route(bind, &blob_yaml(bind), "mcp-crm-tool-blob").await;

    // 11 MiB over the 10 MiB `MAX_STREAM_BODY_BYTES`: the bridge's
    // materialization step fails with `CamelError::StreamLimitExceeded`.
    let route = tokio::spawn(async move {
        let envelope = next_envelope(&mut rx).await;
        let chunks: Vec<Result<bytes::Bytes, CamelError>> =
            vec![Ok(bytes::Bytes::from_static(&[b'x'; 1024])); 11_000];
        let mut out = envelope.exchange;
        out.input.body = Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(
                futures::stream::iter(chunks),
            )))),
            metadata: Default::default(),
        });
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("tool route reply");
    });

    let result = component_client(addr)
        .await
        .call_tool("blob", serde_json::json!({}))
        .await
        .expect("tools/call must answer a projected materialization failure");

    assert!(
        result.is_error,
        "a materialization failure must set the structured is_error flag"
    );
    assert_eq!(
        payload(&result),
        r#"{"error":"Internal Server Error"}"#,
        "a materialization failure body must be the generic constant"
    );
    let serialized = result.content.to_string();
    assert!(
        !serialized.contains("Stream size exceeded limit"),
        "the stream limit detail must never reach the client, got: {serialized}"
    );
    assert!(
        warns.has_warn_field_containing(
            "tool output materialization failed",
            "Stream size exceeded limit"
        ),
        "the materialization WARN event must carry the limit detail in its error field"
    );

    route.await.expect("route task must finish"); // allow-test-wait: route task join bounded by inner 2s recv timeout
    consumer.stop().await.expect("clean stop");
}

/// Call `tool` through the raw rmcp peer and return the `ErrorData` the
/// server answered. The raw peer surfaces JSON-RPC errors as
/// `ServiceError::McpError`, which the component client would flatten into an
/// `Endpoint` string.
async fn rejected_call(
    addr: SocketAddr,
    tool: &str,
    arguments: serde_json::Value,
) -> rmcp::model::ErrorData {
    let params = CallToolRequestParams::new(tool.to_string()).with_arguments(
        arguments
            .as_object()
            .cloned()
            .expect("tool arguments must be a JSON object"),
    );
    let error = raw_rmcp_client(addr)
        .await
        .call_tool_once(params)
        .await
        .expect_err("the call must be rejected");
    match error {
        ServiceError::McpError(data) => data,
        other => panic!("expected a JSON-RPC error, got {other:?}"),
    }
}

/// The `data.violations` array of a rejected call.
fn violations(data: &rmcp::model::ErrorData) -> Vec<serde_json::Value> {
    data.data
        .as_ref()
        .and_then(|data| data.get("violations"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .expect("a rejected call must carry a violations array")
}

#[tokio::test]
async fn violations_exclude_instance_values() {
    let bind = "127.0.0.67:0";
    // Install BEFORE any client request: the rmcp WARN is emitted before the
    // response is written, so both queries are deterministic once the error
    // reaches the client.
    let warns = warn_capture();

    let (mut consumer, _rx, addr) =
        start_tool_route(bind, &big_value_yaml(bind), "mcp-crm-tool-big-value").await;

    let data = rejected_call(
        addr,
        "big-value",
        serde_json::json!({ "id": "A".repeat(1_048_576) }),
    )
    .await;

    assert_eq!(
        data.code,
        rmcp::model::ErrorCode::INVALID_PARAMS,
        "a schema violation must answer -32602"
    );
    let serialized = serde_json::to_string(&data).expect("ErrorData serializes");
    assert!(
        !serialized.contains("AAAAAAAAAAAAAAAA"),
        "a 16-char slice of the instance value must never reach the wire, got: {serialized}"
    );
    assert_eq!(
        violations(&data),
        vec![serde_json::json!("maxLength at /id")],
        "violations must carry only the keyword and redacted path"
    );

    assert!(
        warns.has_warn_field_containing(
            "response error",
            "tool 'big-value' arguments failed input schema validation"
        ),
        "the rmcp response-error WARN event must carry this rejection"
    );
    assert!(
        !warns.has_warn_field_containing("response error", "AAAAAAAAAAAAAAAA"),
        "the instance value must not leak into the WARN log either"
    );

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn violations_mask_invented_member_names() {
    let bind = "127.0.0.68:0";
    let warns = warn_capture();

    let (mut consumer, _rx, addr) =
        start_tool_route(bind, &inventer_yaml(bind), "mcp-crm-tool-inventer").await;

    let data = rejected_call(
        addr,
        "inventer",
        serde_json::json!({ "id": "ok", "x-caller_invented_9f2e": 5 }),
    )
    .await;

    assert_eq!(
        data.code,
        rmcp::model::ErrorCode::INVALID_PARAMS,
        "a schema violation must answer -32602"
    );
    let violations = violations(&data);
    assert!(
        violations.contains(&serde_json::json!("type at /<prop>")),
        "the invented member must be masked, got: {violations:?}"
    );
    let serialized = serde_json::to_string(&data).expect("ErrorData serializes");
    assert!(
        !serialized.contains("caller_invented_9f2e"),
        "the invented member name must never reach the wire, got: {serialized}"
    );

    assert!(
        warns.has_warn_field_containing(
            "response error",
            "tool 'inventer' arguments failed input schema validation"
        ),
        "the rmcp response-error WARN event must carry this rejection"
    );
    assert!(
        !warns.has_warn_field_containing("response error", "caller_invented_9f2e"),
        "the invented member name must not leak into the WARN log either"
    );

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn violations_keyword_from_false_subschema() {
    let bind = "127.0.0.69:0";

    let (mut consumer, _rx, addr) =
        start_tool_route(bind, &falsifier_yaml(bind), "mcp-crm-tool-falsifier").await;

    let data = rejected_call(addr, "falsifier", serde_json::json!({ "value": 1 })).await;

    assert_eq!(
        data.code,
        rmcp::model::ErrorCode::INVALID_PARAMS,
        "a schema violation must answer -32602"
    );
    assert_eq!(
        violations(&data),
        vec![serde_json::json!("falseSchema at /value")],
        "the keyword comes from the failing subschema, not its text"
    );

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn violations_capped_at_25() {
    let bind = "127.0.0.70:0";

    let (mut consumer, _rx, addr) =
        start_tool_route(bind, &capped_yaml(bind), "mcp-crm-tool-capped").await;

    let data = rejected_call(addr, "capped", serde_json::json!({})).await;

    assert_eq!(
        data.code,
        rmcp::model::ErrorCode::INVALID_PARAMS,
        "a schema violation must answer -32602"
    );
    let violations = violations(&data);
    assert_eq!(
        violations.len(),
        25,
        "30 produced violations must be capped at 25, got: {violations:?}"
    );
    for violation in &violations {
        let text = violation.as_str().expect("violations are strings");
        assert!(
            text.starts_with("required at "),
            "every capped entry must be a required violation, got: {text}"
        );
    }

    consumer.stop().await.expect("clean stop");
}
