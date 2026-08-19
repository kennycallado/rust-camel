//! Task 2.7 — server resource dispatch: listing, readiness gating, resource
//! reads through the registry, and the declined prompt/subscription surfaces.
//!
//! The server is the real thing: `McpServerRegistry::get_or_spawn` mounts
//! the rmcp server adapter (no mock). Each test binds its own loopback IP
//! (repo convention for the process-global registry). Resource routes are
//! either a hand-owned registry sender (an mpsc receiver answering reads) or
//! a real `McpConsumer` for the start/stop scenarios. `resources/list` and
//! the declined surfaces are driven with rmcp's discover-lifecycle client;
//! `resources/read` is driven through the component's own `RmcpClient` so
//! JSON-RPC error codes surface in the asserted error strings.

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
use camel_component_mcp::types::McpResourceRead;
use rmcp::ClientLifecycleMode;
use rmcp::model::{ProtocolVersion, SubscribeRequestParams};
use rmcp::service::ClientServiceExt;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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

/// Spawn the real listener for `bind`, returning its bound address and the
/// handle carrying the resource registry.
async fn spawn_server(
    bind: &str,
) -> (
    SocketAddr,
    Arc<camel_component_mcp::registry::McpListenerHandle>,
) {
    let cfg = server_cfg(bind);
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("listener must spawn");
    (handle.local_addr, handle)
}

/// A fresh discover-lifecycle rmcp client against the listener (fresh per
/// call so no client-side response cache can serve a stale list).
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

/// The component's own client for `resources/read` — JSON-RPC error codes are
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
async fn unknown_resource_read_rejected_no_exchange() {
    let (addr, handle) = spawn_server("127.0.0.21:0").await;

    // A live resource route exists — the unknown URI must not reach it.
    let (tx, mut rx) = mpsc::channel::<McpResourceRead>(8);
    handle
        .resource_registry
        .register("crm://customers".to_string(), tx)
        .expect("resource must register");
    handle.resource_registry.mark_ready("crm://customers");

    let error = component_client(addr)
        .await
        .read_resource("crm://unknown")
        .await
        .expect_err("an unregistered resource must be a clean error");
    let message = match &error {
        camel_component_mcp::error::McpError::Endpoint(message) => message.clone(),
        other => panic!("expected a clean endpoint error, got {other:?}"),
    };
    assert!(
        message.contains("-32601") && message.contains("crm://unknown"),
        "expected a method-not-found (-32601) error naming the resource, got: {message}"
    );
    assert_no_invocations(&mut rx, "the resource route handler");
}

#[tokio::test]
async fn stopped_resource_read_rejected_no_exchange() {
    let bind = "127.0.0.22:0";
    let cfg = server_cfg(bind);
    let component = component_with_server("crm", &cfg);

    let endpoint = component
        .create_endpoint(
            "mcp:crm/resource/customers?uri=crm://customers",
            &NoOpComponentContext,
        )
        .expect("endpoint creation must succeed");
    let mut consumer = endpoint
        .create_consumer(Arc::new(NoopRuntimeObservability))
        .expect("consumer creation must succeed");
    let (ctx, mut route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("resource consumer must start");
    consumer.stop().await.expect("clean stop");

    let addr = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener")
        .local_addr;
    let error = component_client(addr)
        .await
        .read_resource("crm://customers")
        .await
        .expect_err("a stopped resource must be a clean error");
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
async fn resources_list_advertises_uris() {
    let bind = "127.0.0.23:0";
    let cfg = server_cfg(bind);
    let component = component_with_server("crm", &cfg);

    let endpoint = component
        .create_endpoint(
            "mcp:crm/resource/customers?uri=crm://customers",
            &NoOpComponentContext,
        )
        .expect("endpoint creation must succeed");
    let mut consumer = endpoint
        .create_consumer(Arc::new(NoopRuntimeObservability))
        .expect("consumer creation must succeed");
    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("resource consumer must start");

    let addr = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener")
        .local_addr;
    let listed = rmcp_client(addr)
        .await
        .peer()
        .list_resources(None)
        .await
        .expect("resources/list must succeed");
    assert!(
        listed
            .resources
            .iter()
            .any(|resource| resource.uri == "crm://customers"),
        "resources/list must include crm://customers, got {:?}",
        listed.resources
    );

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn prompts_list_declines() {
    let (addr, _handle) = spawn_server("127.0.0.24:0").await;

    let error = rmcp_client(addr)
        .await
        .peer()
        .list_prompts(None)
        .await
        .expect_err("prompts/list must decline");
    let message = error.to_string();
    assert!(
        message.contains("-32601") && message.contains("prompts"),
        "expected a method-not-found (-32601) error naming prompts, got: {message}"
    );
}

#[tokio::test]
#[allow(deprecated)]
async fn resources_subscribe_declines() {
    let (addr, _handle) = spawn_server("127.0.0.25:0").await;

    let error = rmcp_client(addr)
        .await
        .peer()
        .subscribe(SubscribeRequestParams::new("crm://customers"))
        .await
        .expect_err("resources/subscribe must decline");
    let message = error.to_string();
    assert!(
        message.contains("-32601") && message.contains("subscribe"),
        "expected a method-not-found (-32601) error naming subscribe, got: {message}"
    );
}
