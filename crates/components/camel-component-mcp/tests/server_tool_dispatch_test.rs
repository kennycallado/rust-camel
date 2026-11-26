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
use std::time::Duration;

use camel_api::security_policy::{
    AccessMode, AuthContext, AuthorizationDecision, CredentialSource, Principal, RouteSecurityPlan,
    SecurityPolicy, TransportId,
};
use camel_api::{Body, CamelError, Exchange, Message};
use camel_auth::{ProviderEntry, ProviderRegistry, ScopePolicy, TokenAuthenticator};
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
use camel_component_mcp::registry::McpListenerHandle;
use camel_component_mcp::types::{McpToolInvocation, McpToolResult};
use rmcp::ClientLifecycleMode;
use rmcp::model::{CallToolRequestParams, ProtocolVersion};
use rmcp::service::ClientServiceExt;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use tokio::sync::mpsc;
use tokio::time::timeout;
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

/// Server config without a `security_policy` block: a policy-less bind whose
/// tools carry no plans (every caller lists everything).
fn plain_server_cfg(bind: &str) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({ "bind": bind })).expect("valid plain server config")
}

/// Spawn the real listener for a policy-less `bind`. The existing
/// `spawn_server` hardcodes `server_cfg` (with `require: auth`), so filters
/// with no policy use this helper.
async fn spawn_plain_server(bind: &str) -> (SocketAddr, Arc<McpListenerHandle>) {
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &plain_server_cfg(bind))
        .await
        .expect("plain listener must spawn");
    (handle.local_addr, handle)
}

/// A fresh discover-lifecycle rmcp client that authenticates every request
/// with `token` (rmcp sends it as `Authorization: Bearer <token>`).
async fn rmcp_client_with_token(
    addr: SocketAddr,
    token: &str,
) -> rmcp::service::RunningService<rmcp::service::RoleClient, ()> {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
            .auth_header(token),
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
            allow_internal: true, // tests bind loopback
            allow_cleartext: false,
            request_timeout_secs: 120,
            connect_timeout_secs: 15,
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
        let invocation = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("route must receive the invocation within 2s")
            .expect("tool route channel alive");
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

    let (name, arguments) = route.await.expect("route task must finish"); // allow-test-wait: route task join bounded by inner 2s recv timeout
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
            let invocation = timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("route must receive the invocation within 2s")
                .expect("tool route channel alive");
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
    route.await.expect("route task must finish"); // allow-test-wait: route task join bounded by inner 2s recv timeout
}

#[tokio::test]
async fn list_tools_hides_scoped_tools_from_unauthenticated_caller() {
    let (addr, handle) = spawn_server("127.0.0.71:0").await;
    let owner = Arc::new(());
    let providers = provider_registry(&[("sec-token", test_principal(&["secret:read"]))]);

    let _open = register_tool(
        &handle,
        &owner,
        "open",
        "open-route",
        Some(public_plan()),
        None,
    );
    let _secret = register_tool(
        &handle,
        &owner,
        "secret",
        "secret-route",
        Some(scoped_plan(&["secret:read"])),
        Some(providers),
    );

    let listed = rmcp_client(addr)
        .await
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list must succeed");
    let mut names: Vec<String> = listed
        .tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["open"],
        "an unauthenticated caller must see only public tools, got {names:?}"
    );
}

#[tokio::test]
async fn list_tools_shows_scoped_tools_to_authorized_caller() {
    let (addr, handle) = spawn_server("127.0.0.72:0").await;
    let owner = Arc::new(());
    let providers = provider_registry(&[("sec-token", test_principal(&["secret:read"]))]);

    let _open = register_tool(
        &handle,
        &owner,
        "open",
        "open-route",
        Some(public_plan()),
        None,
    );
    let _secret = register_tool(
        &handle,
        &owner,
        "secret",
        "secret-route",
        Some(scoped_plan(&["secret:read"])),
        Some(providers),
    );

    let listed = rmcp_client_with_token(addr, "sec-token")
        .await
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list must succeed");
    let mut names: Vec<String> = listed
        .tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["open", "secret"],
        "a caller holding the route's scope must see its tool, got {names:?}"
    );
}

#[tokio::test]
async fn list_tools_policy_less_bind_lists_everything() {
    let (addr, handle) = spawn_plain_server("127.0.0.73:0").await;
    let owner = Arc::new(());
    let mut routes = Vec::new();
    for name in ["alpha", "beta"] {
        let (tx, rx) = mpsc::channel::<McpToolInvocation>(8);
        handle
            .tool_registry
            .register(
                name.to_string(),
                format!("{name}-route"),
                tx,
                id_schema(),
                Arc::downgrade(&owner),
            )
            .expect("tool must register");
        handle.tool_registry.mark_ready(name);
        routes.push(rx);
    }

    for (token, what) in [
        (None, "an unauthenticated caller"),
        (Some("garbage"), "a garbage-credential caller"),
    ] {
        let listed = match token {
            Some(token) => rmcp_client_with_token(addr, token).await,
            None => rmcp_client(addr).await,
        }
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list must succeed");
        let mut names: Vec<String> = listed
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["alpha", "beta"],
            "{what} must see every plan-less tool on a policy-less bind, got {names:?}"
        );
    }
    assert_eq!(routes.len(), 2, "both route channels must stay alive");
}

#[tokio::test]
async fn list_tools_omission_does_not_weaken_call_denial() {
    let (addr, handle) = spawn_server("127.0.0.74:0").await;
    let owner = Arc::new(());
    let providers = provider_registry(&[("sec-token", test_principal(&["secret:read"]))]);
    let mut rx = register_tool(
        &handle,
        &owner,
        "secret",
        "secret-route",
        Some(scoped_plan(&["secret:read"])),
        Some(providers),
    );

    let listed = rmcp_client(addr)
        .await
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list must succeed");
    assert!(
        !listed.tools.iter().any(|tool| tool.name == "secret"),
        "an unauthenticated caller must not see the scoped tool, got {:?}",
        listed.tools
    );

    // Catalog omission is discovery-only: dispatch keeps its own kernel gate.
    let result = component_client(addr)
        .await
        .call_tool("secret", serde_json::json!({ "id": "42" }))
        .await
        .expect("a denial must return as an isError result, not a transport error");
    assert!(
        result.is_error,
        "the denial must be flagged as an error result"
    );
    let text = result.content[0]["text"]
        .as_str()
        .expect("denial content must be a text block");
    assert!(
        text.contains("Unauthenticated") && text.contains("no credential found"),
        "the denial must name the failed credential sources, got: {text}"
    );
    assert_no_invocations(&mut rx, "the route handler");
}

#[tokio::test]
async fn list_tools_hides_scoped_tool_from_authenticated_caller_without_scope() {
    let (addr, handle) = spawn_server("127.0.0.75:0").await;
    let owner = Arc::new(());
    let providers = provider_registry(&[
        ("sec-token", test_principal(&["secret:read"])),
        ("other-token", test_principal(&["other:read"])),
    ]);

    let _open = register_tool(
        &handle,
        &owner,
        "open",
        "open-route",
        Some(public_plan()),
        None,
    );
    let _secret = register_tool(
        &handle,
        &owner,
        "secret",
        "secret-route",
        Some(scoped_plan(&["secret:read"])),
        Some(providers),
    );

    let listed = rmcp_client_with_token(addr, "other-token")
        .await
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list must succeed");
    let mut names: Vec<String> = listed
        .tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["open"],
        "an authenticated caller without the scope must not see the scoped tool, got {names:?}"
    );
}

#[tokio::test]
async fn list_tools_hides_request_dependent_policy_from_authorized_caller() {
    let (addr, handle) = spawn_server("127.0.0.76:0").await;
    let owner = Arc::new(());
    let providers = provider_registry(&[("sec-token", test_principal(&["secret:read"]))]);
    // One policy object shared by the route plan and the route task: the
    // catalog must hide it (not request-independent), yet dispatch must still
    // reach the route and be denied by the same object on real content.
    let policy = Arc::new(EmptyGrantPolicy);
    let plan = RouteSecurityPlan {
        access_mode: AccessMode::Authorized(policy.clone()),
        provider_ref: Some("test-idp".into()),
        transport: TransportId::Mcp,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    };
    let mut rx = register_tool(
        &handle,
        &owner,
        "secret",
        "secret-route",
        Some(plan),
        Some(providers),
    );

    let client = rmcp_client_with_token(addr, "sec-token").await;
    let listed = client
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list must succeed");
    assert!(
        !listed.tools.iter().any(|tool| tool.name == "secret"),
        "a request-dependent policy must stay out of discovery, got {:?}",
        listed.tools
    );

    let route_policy = Arc::clone(&policy);
    let route = tokio::spawn(async move {
        let invocation = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("route must receive the invocation within 2s")
            .expect("tool route channel alive");
        let principal = invocation
            .principal
            .expect("an authenticated caller must dispatch a minted principal");

        // The adapter's kernel path granted the valid principal unchanged;
        // the content-dependent denial is pipeline-owned, and this runs the
        // same policy object at that boundary.
        let mut exchange =
            Exchange::new(Message::new(Body::Json(serde_json::json!({ "id": "42" }))));
        camel_auth::install_carrier(&mut exchange, &principal);
        let decision = route_policy
            .evaluate(
                &mut exchange,
                &AuthContext {
                    principal: &principal,
                    transport: TransportId::Mcp,
                },
            )
            .await
            .expect("policy evaluation must run");
        let reason = match decision {
            AuthorizationDecision::Denied { reason, .. } => reason,
            other => panic!("the request-dependent policy must deny content, got {other:?}"),
        };
        invocation
            .reply
            .send(McpToolResult {
                content: serde_json::json!({ "error": reason }),
                is_error: true,
            })
            .expect("route must answer");
    });

    let params = CallToolRequestParams::new("secret").with_arguments(
        serde_json::json!({ "id": "42" })
            .as_object()
            .cloned()
            .expect("object"),
    );
    let result = client
        .peer()
        .call_tool(params)
        .await
        .expect("tools/call must succeed");
    assert_eq!(
        result.is_error,
        Some(true),
        "the pipeline denial must surface as an error result"
    );
    let text = &result.content[0]
        .as_text()
        .expect("text content block")
        .text;
    assert!(
        text.contains("request content is not permitted"),
        "the denial text must come from the shared policy, got: {text}"
    );
    route.await.expect("route task must finish"); // allow-test-wait: route task join bounded by inner 2s recv timeout
}

/// Grants only the content-free (catalog) shape and denies every exchange
/// with a body. It does NOT override `is_request_independent`, so catalog
/// discovery must treat it as request-dependent and hide the tool.
struct EmptyGrantPolicy;

#[async_trait::async_trait]
impl SecurityPolicy for EmptyGrantPolicy {
    async fn evaluate(
        &self,
        exchange: &mut Exchange,
        auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, camel_api::CamelError> {
        if matches!(exchange.input.body, Body::Empty) {
            Ok(AuthorizationDecision::Granted {
                principal: auth.principal.principal().clone(),
            })
        } else {
            Ok(AuthorizationDecision::Denied {
                reason: "request content is not permitted".into(),
                required: vec![],
                actual: vec![],
            })
        }
    }
}

fn test_principal(scopes: &[&str]) -> Principal {
    Principal {
        subject: "tester".into(),
        issuer: "test".into(),
        audience: vec![],
        scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
        roles: vec![],
        claims: serde_json::json!({}),
    }
}

struct TestAuthenticator {
    tokens: Vec<(String, Principal)>,
}

#[async_trait::async_trait]
impl TokenAuthenticator for TestAuthenticator {
    async fn authenticate_bearer(&self, token: &str) -> Result<Principal, CamelError> {
        self.tokens
            .iter()
            .find(|(candidate, _)| candidate == token)
            .map(|(_, principal)| principal.clone())
            .ok_or_else(|| CamelError::Unauthenticated("invalid credential".into()))
    }
}

fn provider_registry(tokens: &[(&str, Principal)]) -> Arc<ProviderRegistry> {
    let registry = ProviderRegistry::new();
    registry.register(
        "test-idp",
        ProviderEntry {
            authenticator: Arc::new(TestAuthenticator {
                tokens: tokens
                    .iter()
                    .map(|(token, principal)| ((*token).to_string(), principal.clone()))
                    .collect(),
            }),
            audience_binding: None,
        },
    );
    Arc::new(registry)
}

fn scoped_plan(scopes: &[&str]) -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authorized(Arc::new(ScopePolicy::new(
            scopes.iter().map(|scope| (*scope).to_string()).collect(),
            false,
        ))),
        provider_ref: Some("test-idp".into()),
        transport: TransportId::Mcp,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    }
}

fn public_plan() -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Public,
        provider_ref: None,
        transport: TransportId::Mcp,
        credential_sources: vec![],
        audience_binding: None,
    }
}

fn register_tool(
    handle: &McpListenerHandle,
    owner: &Arc<()>,
    name: &str,
    route: &str,
    plan: Option<RouteSecurityPlan>,
    providers: Option<Arc<ProviderRegistry>>,
) -> mpsc::Receiver<McpToolInvocation> {
    let (tx, rx) = mpsc::channel(8);
    handle
        .tool_registry
        .register_with_plan(
            name.to_string(),
            route.to_string(),
            tx,
            id_schema(),
            Arc::downgrade(owner),
            plan.clone(),
        )
        .expect("tool must register");
    handle.tool_registry.mark_ready(name);
    if let Some(plan) = plan {
        handle
            .security
            .register_plan(route, plan, providers, Arc::downgrade(owner));
    }
    rx
}
