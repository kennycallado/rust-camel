//! Task 2.8 — server consumer end-to-end: a real rmcp host discovers, lists,
//! calls, and reads through consumer routes started via `McpComponent`
//! (`create_endpoint` → `create_consumer`).
//!
//! The server is the real thing: `McpServerRegistry::get_or_spawn` mounts the
//! rmcp server adapter (no mock). Each test binds its own loopback IP (repo
//! convention for the process-global registry). Routes are hand-owned
//! `ConsumerContext` channels — the test acts as the route's pipeline (an
//! identity/set-body processor producing a deterministic body) and replies to
//! each invocation. The client is rmcp's discover-lifecycle host.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;

use camel_api::Body;
use camel_component_api::consumer::ExchangeEnvelope;
use camel_component_api::{
    Component, Consumer, ConsumerContext, NoOpComponentContext, NoopRuntimeObservability,
    RuntimeObservability,
};
use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::component::McpComponent;
use camel_component_mcp::config::{McpGlobalConfig, McpServerConfig};
use rmcp::ClientLifecycleMode;
use rmcp::model::{
    CallToolRequestParams, ProtocolVersion, ReadResourceRequestParams, ResourceContents,
};
use rmcp::service::ClientServiceExt;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Non-UTF-8 PNG-flavoured bytes for the binary resource round-trip.
const LOGO_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\xff\xfe\x00\x01";

fn rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoopRuntimeObservability)
}

/// URL-encode a value for a query parameter (the DSL lowering channel).
fn encoded(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The tool's input schema: `{"id": string}` required.
fn id_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { "id": { "type": "string" } },
        "required": ["id"]
    })
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

/// A fresh discover-lifecycle rmcp host client against the listener.
async fn rmcp_client(
    addr: SocketAddr,
) -> rmcp::service::RunningService<rmcp::service::RoleClient, ()> {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp")),
    );
    ().serve_with_lifecycle(
        transport,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await
    .expect("discover-lifecycle connect against the component server")
}

#[tokio::test]
async fn host_discovers_lists_calls_and_reads() {
    let bind = "127.0.0.31:0";
    let cfg = server_cfg(bind);
    let component = component_with_server("crm", &cfg);

    // One tool consumer (id-schema), one text resource, one binary resource —
    // all started through the component, not the registry directly.
    let mut tool_consumer = consumer_for(
        &component,
        &format!(
            "mcp:crm/tool/lookup?schema={}",
            encoded(&id_schema().to_string())
        ),
    );
    let (tool_ctx, mut tool_rx) = test_context();
    tool_consumer
        .start(tool_ctx)
        .await
        .expect("tool consumer must start");

    let mut customers_consumer =
        consumer_for(&component, "mcp:crm/resource/customers?uri=crm://customers");
    let (customers_ctx, mut customers_rx) = test_context();
    customers_consumer
        .start(customers_ctx)
        .await
        .expect("customers resource consumer must start");

    let mut logo_consumer = consumer_for(
        &component,
        "mcp:crm/resource/logo?uri=crm://customers/logo.png",
    );
    let (logo_ctx, mut logo_rx) = test_context();
    logo_consumer
        .start(logo_ctx)
        .await
        .expect("logo resource consumer must start");

    let addr = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener")
        .local_addr;

    // Route pipelines (identity/set-body): answer one invocation each with a
    // deterministic body.
    let tool_route = tokio::spawn(async move {
        let envelope = tool_rx
            .recv()
            .await
            .expect("tool route must receive the invocation");
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
            .expect("tool route reply");
    });

    let customers_route = tokio::spawn(async move {
        let envelope = customers_rx
            .recv()
            .await
            .expect("customers route must receive the read");
        let mut out = envelope.exchange;
        out.input.body = Body::Text("customers-ok".to_string());
        out.input.set_header("Content-Type", "text/plain");
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("customers route reply");
    });

    let logo_route = tokio::spawn(async move {
        let envelope = logo_rx
            .recv()
            .await
            .expect("logo route must receive the read");
        let mut out = envelope.exchange;
        out.input.body = Body::from(LOGO_BYTES.to_vec());
        out.input.set_header("Content-Type", "image/png");
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("logo route reply");
    });

    // Drive the full host lifecycle: discover (via connect) → list → call →
    // read (text + binary).
    let client = rmcp_client(addr).await;

    let listed = client
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list must succeed");
    let tool = listed
        .tools
        .iter()
        .find(|tool| tool.name == "lookup")
        .expect("the lookup tool must be listed");
    assert_eq!(
        serde_json::Value::Object((*tool.input_schema).clone()),
        id_schema(),
        "the listed tool must carry its registered input schema"
    );

    let mut params = CallToolRequestParams::new("lookup");
    params.arguments = Some(
        serde_json::json!({ "id": "42" })
            .as_object()
            .cloned()
            .expect("arguments are an object"),
    );
    let call = client
        .peer()
        .call_tool(params)
        .await
        .expect("tools/call must succeed");
    assert_eq!(
        call.content[0].as_text().expect("text content block").text,
        "lookup-ok:42",
        "tools/call must return the route's processed body"
    );

    let customers = client
        .peer()
        .read_resource(ReadResourceRequestParams::new("crm://customers"))
        .await
        .expect("resources/read must succeed");
    match &customers.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => {
            assert_eq!(text, "customers-ok", "read must return the route's body")
        }
        other => panic!("expected text resource contents, got {other:?}"),
    }

    let logo = client
        .peer()
        .read_resource(ReadResourceRequestParams::new("crm://customers/logo.png"))
        .await
        .expect("binary resources/read must succeed");
    match &logo.contents[0] {
        ResourceContents::BlobResourceContents {
            uri,
            mime_type,
            blob,
            ..
        } => {
            assert_eq!(uri, "crm://customers/logo.png");
            assert_eq!(
                mime_type.as_deref(),
                Some("image/png"),
                "the blob must carry the route's MIME type"
            );
            assert_eq!(
                BASE64_STANDARD
                    .decode(blob)
                    .expect("blob must be base64-encoded"),
                LOGO_BYTES,
                "the blob must round-trip the raw non-UTF-8 bytes"
            );
        }
        other => panic!("expected blob resource contents, got {other:?}"),
    }

    tool_route.await.expect("tool route task must finish");
    customers_route
        .await
        .expect("customers route task must finish");
    logo_route.await.expect("logo route task must finish");

    tool_consumer.stop().await.expect("clean stop");
    customers_consumer.stop().await.expect("clean stop");
    logo_consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn two_consumers_share_one_listener() {
    let bind = "127.0.0.32:0";
    let cfg = server_cfg(bind);
    let component = component_with_server("crm", &cfg);

    let mut tool_consumer = consumer_for(
        &component,
        &format!(
            "mcp:crm/tool/lookup?schema={}",
            encoded(&id_schema().to_string())
        ),
    );
    let (tool_ctx, mut tool_rx) = test_context();
    tool_consumer
        .start(tool_ctx)
        .await
        .expect("tool consumer must start");

    let mut resource_consumer =
        consumer_for(&component, "mcp:crm/resource/customers?uri=crm://customers");
    let (resource_ctx, mut resource_rx) = test_context();
    resource_consumer
        .start(resource_ctx)
        .await
        .expect("resource consumer must start");

    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    assert_eq!(
        handle.spawn_count.load(Ordering::SeqCst),
        1,
        "tool and resource consumers must share exactly one spawned listener"
    );
    let addr = handle.local_addr;

    // Route pipelines answer one invocation each.
    let tool_route = tokio::spawn(async move {
        let envelope = tool_rx
            .recv()
            .await
            .expect("tool route must receive the invocation");
        let mut out = envelope.exchange;
        out.input.body = Body::Text("t-ok".to_string());
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("tool route reply");
    });

    let resource_route = tokio::spawn(async move {
        let envelope = resource_rx
            .recv()
            .await
            .expect("resource route must receive the read");
        let mut out = envelope.exchange;
        out.input.body = Body::Text("r-ok".to_string());
        out.input.set_header("Content-Type", "text/plain");
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("resource route reply");
    });

    let client = rmcp_client(addr).await;

    let mut params = CallToolRequestParams::new("lookup");
    params.arguments = Some(
        serde_json::json!({ "id": "42" })
            .as_object()
            .cloned()
            .expect("arguments are an object"),
    );
    let call = client
        .peer()
        .call_tool(params)
        .await
        .expect("tools/call must succeed");
    assert_eq!(
        call.content[0].as_text().expect("text content").text,
        "t-ok"
    );

    let read = client
        .peer()
        .read_resource(ReadResourceRequestParams::new("crm://customers"))
        .await
        .expect("resources/read must succeed");
    match &read.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => assert_eq!(text, "r-ok"),
        other => panic!("expected text resource contents, got {other:?}"),
    }

    tool_route.await.expect("tool route task must finish");
    resource_route
        .await
        .expect("resource route task must finish");

    tool_consumer.stop().await.expect("clean stop");
    resource_consumer.stop().await.expect("clean stop");
}
