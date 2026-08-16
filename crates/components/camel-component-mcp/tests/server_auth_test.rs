//! Route-level authentication for the MCP server role (ADR-0060 Rule 8).
//!
//! The carry: the rmcp adapter copies inbound HTTP request headers into the
//! dispatch payload, and the consumer bridge sets them as Exchange input
//! headers before the pipeline runs. Enforcement is route-level — the
//! route's `SecurityPolicy` evaluates against the Exchange exactly as
//! camel-http routes do. A denial flows through the existing bridge error
//! path: tools get an `isError` result and resources get the error body; the
//! route body never sees a denied Exchange.
//!
//! Requests are driven with raw JSON-RPC over HTTP (a minimal
//! `Connection: close` client over `tokio::net::TcpStream`) so the
//! `Authorization` header is controlled precisely — the same seam
//! `server_protocol_test.rs` uses.

mod common;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use camel_api::Body;
use camel_api::security_policy::{CredentialSource, Principal, SecurityPolicyConfig};
use camel_auth::native_auth::{NativeCredential, NativeCredentialSecret, NativeCredentialStore};
use camel_auth::{RolePolicy, StaticTokenAuthenticator, TokenAuthenticator};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::consumer::ExchangeEnvelope;
use camel_component_api::{
    Component, ConsumerContext, NoOpComponentContext, NoopRuntimeObservability,
    RuntimeObservability,
};
use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::component::McpComponent;
use camel_component_mcp::config::{McpGlobalConfig, McpServerConfig};
use camel_test::CamelTestContext;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Sentinel token — test fixture, not a real secret.
const SENTINEL_MCP_TOKEN: &str = "mcp-test-token-1"; // allow-secret
/// The role the test principal holds.
const MCP_CLIENT_ROLE: &str = "mcp-client";

/// One parsed raw HTTP JSON-RPC reply.
struct RawJsonRpc {
    /// The JSON-RPC response body (parsed JSON).
    value: serde_json::Value,
}

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

/// TOML-side server config. `security_policy` stays opaque here — it is the
/// bind-presence gate (fail-closed), not the enforcement surface.
fn server_cfg(bind: &str) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({
        "bind": bind,
        "security_policy": {"require": "auth"},
    }))
    .expect("valid server config")
}

fn component_with_server(name: &str, cfg: &McpServerConfig) -> McpComponent {
    let mut servers = HashMap::new();
    servers.insert(name.to_string(), cfg.clone());
    McpComponent::new(McpGlobalConfig {
        servers,
        remotes: HashMap::new(),
    })
    .expect("config must construct")
}

/// A hand-owned route channel: the test acts as the route's pipeline.
fn test_context() -> (ConsumerContext, mpsc::Receiver<ExchangeEnvelope>) {
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "test-route".to_string());
    (ctx, rx)
}

/// POST one JSON-RPC message to the server's `/mcp` route, returning the
/// parsed JSON-RPC reply body. `Connection: close` makes the reply
/// EOF-terminated so no chunked/length parsing is needed.
async fn raw_json_rpc_post(
    addr: SocketAddr,
    body: &serde_json::Value,
    extra_headers: &[(&str, &str)],
) -> RawJsonRpc {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let payload = body.to_string();
    // Streamable HTTP requires Accept to name both JSON and SSE, and the
    // `_meta` protocolVersion must be mirrored by the MCP-Protocol-Version
    // header (rmcp's stateless request validation).
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         MCP-Protocol-Version: 2026-07-28\r\nConnection: close\r\n\
         Content-Length: {}\r\n",
        payload.len()
    );
    for (name, value) in extra_headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&payload);

    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect to listener");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write JSON-RPC request");
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .expect("read reply to EOF");
    // Body follows the blank line separating the HTTP head from the payload.
    let text = String::from_utf8_lossy(&raw);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    let value = serde_json::from_str(&body).expect("reply body must be JSON");
    RawJsonRpc { value }
}

/// A `tools/call` JSON-RPC request carrying the per-request protocol version
/// and client capabilities in `_meta` (the stateless authorizer; no session
/// header is sent).
fn tools_call(name: &str, arguments: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": name,
            "arguments": arguments,
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    })
}

/// A `resources/read` JSON-RPC request, same `_meta` shape.
fn resources_read(uri: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "resources/read",
        "params": {
            "uri": uri,
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    })
}

/// Build a route-level `RolePolicy` requiring `MCP_CLIENT_ROLE` over a native
/// store seeded with `SENTINEL_MCP_TOKEN`.
fn role_policy_config() -> (SecurityPolicyConfig, Arc<dyn TokenAuthenticator>) {
    let principal = Principal {
        subject: "mcp-user".into(),
        issuer: "native".into(),
        audience: vec![],
        scopes: vec![],
        roles: vec![MCP_CLIENT_ROLE.to_string()],
        claims: serde_json::json!({}),
    };
    let store = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: SENTINEL_MCP_TOKEN.to_string().into(),
        },
        principal,
    }])
    .unwrap();
    let authenticator: Arc<dyn TokenAuthenticator> = Arc::new(StaticTokenAuthenticator::new(store));
    let sources = vec![CredentialSource::AuthorizationHeader];
    let policy = RolePolicy::new(
        vec![MCP_CLIENT_ROLE.to_string()],
        true,
        false,
        Arc::clone(&authenticator),
        sources.clone(),
    );
    let config = SecurityPolicyConfig::new(policy).with_credential_sources(sources);
    (config, authenticator)
}

/// Build a full `CamelTestContext` running one MCP server route protected by a
/// route-level `RolePolicy`, returning the harness and the bound listener
/// address. The route body is a `mock:result` producer so the test asserts
/// whether an Exchange reached it.
async fn build_secure_mcp_route(bind: &str, from_uri: &str) -> (CamelTestContext, SocketAddr) {
    let cfg = server_cfg(bind);
    let component = component_with_server("crm", &cfg);

    let h = CamelTestContext::builder()
        .with_component(component)
        .with_mock()
        .build()
        .await;

    let (policy, authenticator) = role_policy_config();
    let route = RouteBuilder::from(from_uri)
        .route_id(format!("mcp-auth-{from_uri}"))
        .security_policy(policy)
        .security_authenticator(authenticator)
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let addr = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener")
        .local_addr;

    (h, addr)
}

#[tokio::test(flavor = "multi_thread")]
async fn request_headers_reach_exchange() {
    let bind = "127.0.0.51:0";
    let cfg = server_cfg(bind);
    let component = component_with_server("crm", &cfg);

    let endpoint = component
        .create_endpoint(
            &format!(
                "mcp:crm/tool/lookup?schema={}",
                encoded(&id_schema().to_string())
            ),
            &NoOpComponentContext,
        )
        .expect("endpoint creation must succeed");
    let mut consumer = endpoint
        .create_consumer(rt())
        .expect("consumer creation must succeed");

    let (ctx, mut route_rx) = test_context();
    consumer.start(ctx).await.expect("tool consumer must start");

    let addr = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener")
        .local_addr;

    // The route: assert the carried header reached the Exchange, then answer.
    let route = tokio::spawn(async move {
        let envelope = route_rx.recv().await.expect("route received the exchange");
        assert_eq!(
            envelope.exchange.input.header_ic("X-Probe"),
            Some(&serde_json::Value::String("abc".to_string())),
            "the inbound HTTP header must reach the Exchange input headers"
        );
        let mut out = envelope.exchange;
        out.input.body = Body::Text("lookup-ok".to_string());
        envelope
            .reply_tx
            .expect("reply channel present")
            .send(Ok(out))
            .expect("route reply");
    });

    let response = raw_json_rpc_post(
        addr,
        &tools_call("lookup", serde_json::json!({ "id": "42" })),
        &[
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "lookup"),
            ("X-Probe", "abc"),
        ],
    )
    .await;
    assert_eq!(
        response.value["result"]["content"][0]["text"],
        serde_json::json!("lookup-ok"),
        "the tool call must return the route's reply, got {}",
        response.value
    );

    route.await.expect("route task must finish");
    consumer.stop().await.expect("clean stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_call_denied_without_credentials() {
    let bind = "127.0.0.52:0";
    let schema = encoded(&id_schema().to_string());
    let (h, addr) =
        build_secure_mcp_route(bind, &format!("mcp:crm/tool/lookup?schema={schema}")).await;

    let response = raw_json_rpc_post(
        addr,
        &tools_call("lookup", serde_json::json!({ "id": "42" })),
        &[("Mcp-Method", "tools/call"), ("Mcp-Name", "lookup")],
    )
    .await;
    assert_eq!(
        response.value["result"]["isError"],
        serde_json::json!(true),
        "a call without credentials must be an isError result, got {}",
        response.value
    );

    // The route body never saw the denied Exchange.
    h.mock()
        .get_endpoint("result")
        .expect("mock endpoint must exist")
        .assert_exchange_count(0)
        .await;

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_call_granted_with_valid_token() {
    let bind = "127.0.0.53:0";
    let schema = encoded(&id_schema().to_string());
    let (h, addr) =
        build_secure_mcp_route(bind, &format!("mcp:crm/tool/lookup?schema={schema}")).await;

    let response = raw_json_rpc_post(
        addr,
        &tools_call("lookup", serde_json::json!({ "id": "42" })),
        &[
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "lookup"),
            ("Authorization", &format!("Bearer {SENTINEL_MCP_TOKEN}")),
        ],
    )
    .await;
    assert_eq!(
        response.value["result"]["isError"],
        serde_json::json!(false),
        "a call with a valid token must succeed, got {}",
        response.value
    );

    // The route body ran and the principal reached the Exchange.
    let endpoint = h
        .mock()
        .get_endpoint("result")
        .expect("mock endpoint must exist");
    endpoint.assert_exchange_count(1).await;
    let received = endpoint.get_received_exchanges().await;
    let roles: Vec<String> = serde_json::from_str(
        received[0]
            .property("camel.auth.roles")
            .expect("principal roles must be present")
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        roles,
        vec![MCP_CLIENT_ROLE],
        "the granted Exchange must carry the principal's roles"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn resource_read_denied_without_credentials() {
    let bind = "127.0.0.54:0";
    let (h, addr) =
        build_secure_mcp_route(bind, "mcp:crm/resource/customers?uri=crm://customers").await;

    let response = raw_json_rpc_post(
        addr,
        &resources_read("crm://customers"),
        &[
            ("Mcp-Method", "resources/read"),
            ("Mcp-Name", "crm://customers"),
        ],
    )
    .await;
    let text = response.value["result"]["contents"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("Unauthenticated"),
        "a read without credentials must carry the denial as the error body, got {text}"
    );

    // The route body never saw the denied Exchange.
    h.mock()
        .get_endpoint("result")
        .expect("mock endpoint must exist")
        .assert_exchange_count(0)
        .await;

    h.stop().await;
}
