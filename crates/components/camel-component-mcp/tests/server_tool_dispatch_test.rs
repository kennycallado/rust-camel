//! Task 2.6 — server tool dispatch: listing, readiness gating, schema
//! validation, and `tools/call` through the tool registry.
//!
//! The server is the real thing: `McpServerRegistry::get_or_spawn` mounts
//! the rmcp server adapter (no mock). Each test binds its own loopback IP
//! (repo convention for the process-global registry). Route handlers are
//! real registry senders: either an mpsc receiver answering invocations
//! directly (registered into a real listener's tool registry), or a real
//! `McpConsumer` for the stop scenario. `tools/list` is driven with rmcp's
//! discover-lifecycle client; `tools/call` is driven through the
//! component's own `RmcpClient` so JSON-RPC error codes surface in the
//! asserted error strings.

mod common;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use camel_component_api::consumer::ExchangeEnvelope;
use camel_component_api::{
    Component, ConsumerContext, NoOpComponentContext, NoopRuntimeObservability,
};
use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::adapter::RmcpClient;
use camel_component_mcp::client::McpClient;
use camel_component_mcp::component::McpComponent;
use camel_component_mcp::config::{
    McpGlobalConfig, McpRemoteConfig, McpServerConfig, McpTransport,
};
use camel_component_mcp::types::{McpToolInvocation, McpToolResult};
use rmcp::ClientLifecycleMode;
use rmcp::model::ProtocolVersion;
use rmcp::service::ClientServiceExt;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The schema every registered tool uses: `{"id": string}` required.
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

/// Spawn the real listener for `bind`, returning its bound address and the
/// handle carrying the tool registry.
async fn spawn_server(
    bind: &str,
) -> (
    SocketAddr,
    std::sync::Arc<camel_component_mcp::registry::McpListenerHandle>,
) {
    let cfg = server_cfg(bind);
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("listener must spawn");
    (handle.local_addr, handle)
}

/// A fresh discover-lifecycle rmcp client against the listener (fresh per
/// `tools/list` so no client-side response cache can serve a stale list).
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

/// The component's own client for `tools/call` — JSON-RPC error codes are
/// asserted through its `McpError::Endpoint` strings.
async fn component_client(addr: SocketAddr) -> RmcpClient {
    RmcpClient::connect(
        "dispatch-test",
        &McpRemoteConfig {
            url: format!("http://{addr}/mcp"),
            transport: McpTransport::StreamableHttp,
        },
    )
    .await
    .expect("component client must connect")
}

/// A hand-owned route channel: the test acts as the route's pipeline.
fn test_context() -> (ConsumerContext, mpsc::Receiver<ExchangeEnvelope>) {
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "test-route".to_string());
    (ctx, rx)
}

/// Assert a receiver saw zero invocations: `try_recv` yields an error both
/// while the channel is merely empty and after every sender has dropped
/// (a stopped consumer) — only a buffered message means a dispatch happened.
fn assert_no_invocations<T>(rx: &mut mpsc::Receiver<T>, what: &str) {
    assert!(
        rx.try_recv().is_err(),
        "{what} must have seen zero invocations"
    );
}

#[tokio::test]
async fn tools_list_hides_not_ready_tool() {
    let (addr, handle) = spawn_server("127.0.0.2:0").await;

    // Registered but NOT ready: a started consumer's channel held pre-ready.
    let (tx, _rx) = mpsc::channel::<McpToolInvocation>(8);
    let owner = Arc::new(());
    handle
        .tool_registry
        .register(
            "lookup".to_string(),
            "dispatch-route".to_string(),
            tx,
            id_schema(),
            Arc::downgrade(&owner),
        )
        .expect("tool must register");

    let listed = rmcp_client(addr)
        .await
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list must succeed");
    assert!(
        !listed.tools.iter().any(|tool| tool.name == "lookup"),
        "a not-ready tool must be omitted from tools/list, got {:?}",
        listed.tools
    );

    // mark_ready: a fresh client's tools/list now includes it with its
    // input schema (fresh connection — no cached list).
    handle.tool_registry.mark_ready("lookup");
    let listed = rmcp_client(addr)
        .await
        .peer()
        .list_tools(None)
        .await
        .expect("second tools/list must succeed");
    let tool = listed
        .tools
        .iter()
        .find(|tool| tool.name == "lookup")
        .expect("the ready tool must be listed");
    assert_eq!(
        serde_json::Value::Object((*tool.input_schema).clone()),
        id_schema(),
        "the listed tool must carry its registered input schema"
    );
}

#[tokio::test]
async fn valid_args_reach_route() {
    let (addr, handle) = spawn_server("127.0.0.3:0").await;

    let (tx, mut rx) = mpsc::channel::<McpToolInvocation>(8);
    let owner = Arc::new(());
    handle
        .tool_registry
        .register(
            "lookup".to_string(),
            "dispatch-route".to_string(),
            tx,
            id_schema(),
            Arc::downgrade(&owner),
        )
        .expect("tool must register");
    handle.tool_registry.mark_ready("lookup");

    // Route handler: answer one invocation, then hand its name/arguments
    // back for asserts.
    let route = tokio::spawn(async move {
        let invocation = rx.recv().await.expect("route must receive the invocation");
        let McpToolInvocation {
            name,
            arguments,
            reply,
            ..
        } = invocation;
        reply
            .send(McpToolResult {
                content: serde_json::Value::String("lookup-ok".to_string()),
                is_error: false,
            })
            .expect("route must answer");
        (name, arguments)
    });

    let result = component_client(addr)
        .await
        .call_tool("lookup", serde_json::json!({ "id": "42" }))
        .await
        .expect("valid arguments must be dispatched");

    let (name, arguments) = route.await.expect("route task must finish");
    assert_eq!(name, "lookup");
    assert_eq!(arguments, serde_json::json!({ "id": "42" }));
    // The route's reply content returned as the tools/call content blocks.
    assert_eq!(
        result.content[0]["text"],
        serde_json::json!("lookup-ok"),
        "the call result must carry the route's reply, got {}",
        result.content
    );
}

#[tokio::test]
async fn invalid_args_rejected_no_exchange() {
    let (addr, handle) = spawn_server("127.0.0.4:0").await;

    let (tx, mut rx) = mpsc::channel::<McpToolInvocation>(8);
    let owner = Arc::new(());
    handle
        .tool_registry
        .register(
            "lookup".to_string(),
            "dispatch-route".to_string(),
            tx,
            id_schema(),
            Arc::downgrade(&owner),
        )
        .expect("tool must register");
    handle.tool_registry.mark_ready("lookup");

    let error = component_client(addr)
        .await
        .call_tool("lookup", serde_json::json!({ "id": 7 }))
        .await
        .expect_err("arguments violating the schema must be rejected");
    let message = match &error {
        camel_component_mcp::error::McpError::Endpoint(message) => message.clone(),
        other => panic!("expected a clean endpoint error, got {other:?}"),
    };
    assert!(
        message.contains("-32602") && message.contains("lookup"),
        "expected an invalid_params (-32602) error naming the tool, got: {message}"
    );
    assert_no_invocations(&mut rx, "the route handler");
}

#[tokio::test]
async fn unknown_tool_call_returns_clean_error() {
    let (addr, handle) = spawn_server("127.0.0.5:0").await;

    // A live route handler exists — the unknown name must not reach it.
    let (tx, mut rx) = mpsc::channel::<McpToolInvocation>(8);
    let owner = Arc::new(());
    handle
        .tool_registry
        .register(
            "known".to_string(),
            "dispatch-route".to_string(),
            tx,
            id_schema(),
            Arc::downgrade(&owner),
        )
        .expect("tool must register");
    handle.tool_registry.mark_ready("known");

    let error = component_client(addr)
        .await
        .call_tool("nonexistent", serde_json::json!({ "id": "42" }))
        .await
        .expect_err("an unregistered tool must be a clean error");
    let message = match &error {
        camel_component_mcp::error::McpError::Endpoint(message) => message.clone(),
        other => panic!("expected a clean endpoint error, got {other:?}"),
    };
    assert!(
        message.contains("-32601") && message.contains("nonexistent"),
        "expected a method-not-found (-32601) error naming the tool, got: {message}"
    );
    assert_no_invocations(&mut rx, "the route handler");
}

#[tokio::test]
async fn stopped_tool_call_returns_clean_error() {
    let bind = "127.0.0.6:0";
    let cfg = server_cfg(bind);
    let mut servers = HashMap::new();
    servers.insert("crm".to_string(), cfg.clone());
    let component = McpComponent::new(McpGlobalConfig {
        servers,
        remotes: HashMap::new(),
    });

    let encoded_schema = percent_encoding::utf8_percent_encode(
        &id_schema().to_string(),
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();
    let endpoint = component
        .create_endpoint(
            &format!("mcp:crm/tool/lookup?schema={encoded_schema}"),
            &NoOpComponentContext,
        )
        .expect("endpoint creation must succeed");
    let mut consumer = endpoint
        .create_consumer(Arc::new(NoopRuntimeObservability))
        .expect("consumer creation must succeed");
    let (ctx, mut route_rx) = test_context();
    consumer.start(ctx).await.expect("tool consumer must start");
    consumer.stop().await.expect("clean stop");

    let addr = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener")
        .local_addr;
    let error = component_client(addr)
        .await
        .call_tool("lookup", serde_json::json!({ "id": "42" }))
        .await
        .expect_err("a stopped tool must be a clean error");
    let message = match &error {
        camel_component_mcp::error::McpError::Endpoint(message) => message.clone(),
        other => panic!("expected a clean endpoint error, got {other:?}"),
    };
    assert!(
        message.contains("-32601"),
        "expected a method-not-found (-32601) error, got: {message}"
    );
    assert_no_invocations(&mut route_rx, "the route handler");
}

#[tokio::test]
async fn error_shaped_success_content_stays_success_and_flag_drives_error() {
    let (addr, handle) = spawn_server("127.0.0.7:0").await;
    let (tx, mut rx) = mpsc::channel::<McpToolInvocation>(8);
    let owner = Arc::new(());
    handle
        .tool_registry
        .register(
            "lookup".to_string(),
            "dispatch-route".to_string(),
            tx,
            id_schema(),
            Arc::downgrade(&owner),
        )
        .expect("tool must register");
    handle.tool_registry.mark_ready("lookup");

    // Twin replies with identical error-shaped content; only the structured
    // `is_error` flag differs — the server must honor the flag, never sniff
    // content for an "error" key.
    let route = tokio::spawn(async move {
        for is_error in [false, true] {
            let invocation = rx.recv().await.expect("route must receive the invocation");
            invocation
                .reply
                .send(McpToolResult {
                    content: serde_json::json!({ "error": null }),
                    is_error,
                })
                .expect("route must answer");
        }
    });

    for expected_error in [false, true] {
        let result = component_client(addr)
            .await
            .call_tool("lookup", serde_json::json!({ "id": "42" }))
            .await
            .expect("a completed reply must return as a result");
        assert_eq!(
            result.is_error, expected_error,
            "the structured flag must drive failure semantics, content must not be sniffed"
        );
    }
    route.await.expect("route task must finish");
}
