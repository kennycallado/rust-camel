//! Task 3.5 — DSL end-to-end acceptance: a YAML `mcp:` block is declared,
//! lowered to consumer routes by the camel-dsl YAML entry point, started
//! through `McpComponent`, and driven by a real rmcp host.
//!
//! The full Phase 3 exit-criteria chain: declare → lower → start → host calls
//! tool → result. The server is the real thing (`McpServerRegistry::get_or_spawn`
//! mounts the rmcp server adapter, no mock). Each test binds its own loopback
//! IP (repo convention for the process-global registry). The lowered consumer
//! routes carry no processing steps, so the test hand-owns the route channels
//! and acts as the route's pipeline (a set-body processor producing a
//! deterministic body), mirroring `server_consumer_e2e_test.rs`. The schema
//! travels from the DSL block through the lowered `from` URI into the tool
//! registry — proven live by the invalid-args rejection (no Exchange created).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use camel_api::Body;
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

/// The tool's input schema: `{"id": string}` required.
fn id_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { "id": { "type": "string" } },
        "required": ["id"]
    })
}

/// The YAML route document: one `mcp:` block declaring server `crm` with a
/// `security_policy`, one `lookup` tool, and one `customers` resource.
fn mcp_yaml(bind: &str) -> String {
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
    resources:
      - name: customers
        uri: crm://customers
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
    .expect("config must construct")
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

/// The component's own client for `tools/call` — JSON-RPC error codes are
/// asserted through its `McpError::Endpoint` strings.
async fn component_client(addr: SocketAddr) -> RmcpClient {
    RmcpClient::connect(
        "dsl-e2e-test",
        &McpRemoteConfig {
            url: format!("http://{addr}/mcp"),
            transport: McpTransport::StreamableHttp,
        },
    )
    .await
    .expect("component client must connect")
}

/// Assert a receiver saw zero invocations: `try_recv` yields an error both
/// while the channel is merely empty and after every sender has dropped — only
/// a buffered message means a dispatch happened.
fn assert_no_invocations<T>(rx: &mut mpsc::Receiver<T>, what: &str) {
    assert!(
        rx.try_recv().is_err(),
        "{what} must have seen zero invocations"
    );
}

#[tokio::test]
async fn dsl_block_runs_end_to_end() {
    let bind = "127.0.0.41:0";
    let cfg = server_cfg(bind);

    // Declare → lower: the `mcp:` block lowers to consumer routes.
    let routes = camel_dsl::parse_yaml_to_declarative(&mcp_yaml(bind))
        .expect("the mcp block must parse and lower");
    let tool_route = routes
        .iter()
        .find(|r| r.route_id == "mcp-crm-tool-lookup")
        .expect("the tool consumer route must be lowered");
    let resource_route = routes
        .iter()
        .find(|r| r.route_id == "mcp-crm-resource-customers")
        .expect("the resource consumer route must be lowered");

    // Start: consumers for the lowered `from` URIs, through the component.
    let component = component_with_server("crm", &cfg);
    let mut tool_consumer = consumer_for(&component, &tool_route.from);
    let (tool_ctx, mut tool_rx) = test_context();
    tool_consumer
        .start(tool_ctx)
        .await
        .expect("tool consumer must start");

    let mut customers_consumer = consumer_for(&component, &resource_route.from);
    let (customers_ctx, mut customers_rx) = test_context();
    customers_consumer
        .start(customers_ctx)
        .await
        .expect("resource consumer must start");

    let addr = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener")
        .local_addr;

    // Route pipelines (the processing step): answer one invocation each with a
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
            .expect("resource route must receive the read");
        let mut out = envelope.exchange;
        out.input.body = Body::Text("customers-ok".to_string());
        out.input.set_header("Content-Type", "text/plain");
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("resource route reply");
    });

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
        "the listed tool must carry its DSL-injected input schema"
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
            assert_eq!(
                text, "customers-ok",
                "read must return the resource route's body"
            )
        }
        other => panic!("expected text resource contents, got {other:?}"),
    }

    tool_route.await.expect("tool route task must finish");
    customers_route
        .await
        .expect("resource route task must finish");

    tool_consumer.stop().await.expect("clean stop");
    customers_consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn dsl_injected_schema_is_enforced() {
    let bind = "127.0.0.42:0";
    let cfg = server_cfg(bind);

    let routes = camel_dsl::parse_yaml_to_declarative(&mcp_yaml(bind))
        .expect("the mcp block must parse and lower");
    let tool_route = routes
        .iter()
        .find(|r| r.route_id == "mcp-crm-tool-lookup")
        .expect("the tool consumer route must be lowered");

    let component = component_with_server("crm", &cfg);
    let mut tool_consumer = consumer_for(&component, &tool_route.from);
    let (tool_ctx, mut tool_rx) = test_context();
    tool_consumer
        .start(tool_ctx)
        .await
        .expect("tool consumer must start");

    let addr = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener")
        .local_addr;

    // `{"id": 7}` violates the DSL-injected `{"id": string}` schema — rejected
    // before any Exchange is created (proving the DSL schema is live).
    let error = component_client(addr)
        .await
        .call_tool("lookup", serde_json::json!({ "id": 7 }))
        .await
        .expect_err("arguments violating the DSL-injected schema must be rejected");
    let message = match &error {
        camel_component_mcp::error::McpError::Endpoint(message) => message.clone(),
        other => panic!("expected a clean endpoint error, got {other:?}"),
    };
    assert!(
        message.contains("-32602") && message.contains("lookup"),
        "expected an invalid_params (-32602) error naming the tool, got: {message}"
    );
    assert_no_invocations(&mut tool_rx, "the tool route");

    tool_consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn dsl_server_missing_from_config_fails_cleanly() {
    // A DSL block declares server `nosuch`, but the component is started
    // WITHOUT that server in config. The lowered consumer route must fail at
    // consumer START with a clean error naming the missing server — the DSL
    // server `name` MUST match a TOML `mcp.servers.<name>` key (TOML owns
    // runtime server config).
    let yaml = r#"
mcp:
  - server:
      name: nosuch
      bind: 127.0.0.1:9100
      security_policy:
        roles: [mcp-client]
    tools:
      - name: lookup
        input_schema:
          type: object
"#;
    let routes =
        camel_dsl::parse_yaml_to_declarative(yaml).expect("the mcp block must parse and lower");
    let tool_route = routes
        .iter()
        .find(|r| r.route_id == "mcp-nosuch-tool-lookup")
        .expect("the tool consumer route must be lowered");

    // Component with NO `nosuch` server in config.
    let component = McpComponent::default();
    let mut consumer = consumer_for(&component, &tool_route.from);
    let (ctx, _rx) = test_context();
    let err = consumer
        .start(ctx)
        .await
        .expect_err("consumer start must fail when the server is missing from config");
    let message = err.to_string();
    assert!(
        message.contains("nosuch"),
        "the error must name the missing server, got: {message}"
    );
}
