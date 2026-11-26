//! Task 2.7 — server resource dispatch: listing, readiness gating, resource
//! reads through the registry, the declined prompt/subscription surfaces,
//! and Task 3.2 catalog auth-filtering parity.
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

use camel_api::CamelError;
use camel_api::security_policy::AccessMode;
use camel_api::security_policy::CredentialSource;
use camel_api::security_policy::Principal;
use camel_api::security_policy::RouteSecurityPlan;
use camel_api::security_policy::TransportId;
use camel_auth::ProviderEntry;
use camel_auth::ProviderRegistry;
use camel_auth::ScopePolicy;
use camel_auth::TokenAuthenticator;
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
async fn unknown_resource_read_rejected_no_exchange() {
    let (addr, handle) = spawn_server("127.0.0.21:0").await;

    // A live resource route exists — the unknown URI must not reach it.
    let (tx, mut rx) = mpsc::channel::<McpResourceRead>(8);
    let owner = Arc::new(());
    handle
        .resource_registry
        .register(
            "crm://customers".to_string(),
            "dispatch-route".to_string(),
            tx,
            Arc::downgrade(&owner),
        )
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

#[tokio::test]
async fn list_resources_filters_scoped_resources_by_authentication() {
    let (addr, handle) = spawn_server("127.0.0.77:0").await;
    let owner = Arc::new(());
    // One provider registry per bind (the last snapshot wins), accepting
    // both tokens so a single listener can distinguish the two callers.
    let providers = provider_registry(&[
        ("sec-token", test_principal(&["secret:read"])),
        ("other-token", test_principal(&["other:read"])),
    ]);

    let _open = register_resource(
        &handle,
        &owner,
        "crm://open",
        "open-route",
        Some(public_plan()),
        None,
    );
    let _secret = register_resource(
        &handle,
        &owner,
        "crm://secret",
        "secret-route",
        Some(scoped_plan(&["secret:read"])),
        Some(providers),
    );

    assert_eq!(
        list_resource_uris(addr, None).await,
        ["crm://open"],
        "an unauthenticated caller must see only public resources"
    );
    assert_eq!(
        list_resource_uris(addr, Some("sec-token")).await,
        ["crm://open", "crm://secret"],
        "a caller holding the route's scope must see its resource"
    );
    assert_eq!(
        list_resource_uris(addr, Some("other-token")).await,
        ["crm://open"],
        "an authenticated caller without the scope must not see the scoped resource"
    );
}

#[tokio::test]
async fn list_resources_policy_less_bind_lists_everything() {
    let (addr, handle) = spawn_plain_server("127.0.0.78:0").await;
    let owner = Arc::new(());
    let mut routes = Vec::new();
    for uri in ["crm://alpha", "crm://beta"] {
        let (tx, rx) = mpsc::channel::<McpResourceRead>(8);
        handle
            .resource_registry
            .register(
                uri.to_string(),
                format!("{uri}-route"),
                tx,
                Arc::downgrade(&owner),
            )
            .expect("resource must register");
        handle.resource_registry.mark_ready(uri);
        routes.push(rx);
    }

    for (token, what) in [
        (None, "an unauthenticated caller"),
        (Some("garbage"), "a garbage-credential caller"),
    ] {
        assert_eq!(
            list_resource_uris(addr, token).await,
            ["crm://alpha", "crm://beta"],
            "{what} must see every plan-less resource on a policy-less bind"
        );
    }
    assert_eq!(routes.len(), 2, "both route channels must stay alive");
}

#[tokio::test]
async fn list_resources_omission_does_not_weaken_read_denial() {
    let (addr, handle) = spawn_server("127.0.0.79:0").await;
    let owner = Arc::new(());
    let providers = provider_registry(&[("sec-token", test_principal(&["secret:read"]))]);
    let mut rx = register_resource(
        &handle,
        &owner,
        "crm://secret",
        "secret-route",
        Some(scoped_plan(&["secret:read"])),
        Some(providers),
    );

    assert!(
        list_resource_uris(addr, None).await.is_empty(),
        "an unauthenticated caller must not see the scoped resource"
    );

    // Catalog omission is discovery-only: the read keeps its own kernel gate
    // and answers the denial as the resource's error body.
    let resource = component_client(addr)
        .await
        .read_resource("crm://secret")
        .await
        .expect("a denial must return as resource contents, not a transport error");
    let text = std::str::from_utf8(&resource.content).expect("the denial body must be UTF-8");
    assert!(
        text.contains("Unauthenticated") && text.contains("no credential found"),
        "the denial must name the failed credential sources, got: {text}"
    );
    assert_no_invocations(&mut rx, "the resource route handler");
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

/// Server config without a `security_policy` block: a policy-less bind whose
/// resources carry no plans (every caller lists everything).
fn plain_server_cfg(bind: &str) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({ "bind": bind })).expect("valid plain server config")
}

/// Spawn the real listener for a policy-less `bind`. `spawn_server` hardcodes
/// `server_cfg` (with `require: auth`), so plan-less filters use this helper.
async fn spawn_plain_server(bind: &str) -> (SocketAddr, Arc<McpListenerHandle>) {
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &plain_server_cfg(bind))
        .await
        .expect("plain listener must spawn");
    (handle.local_addr, handle)
}

/// Lists the resource URIs `addr` advertises to `token`'s caller (fresh
/// client per call so no client-side response cache serves a stale list).
async fn list_resource_uris(addr: SocketAddr, token: Option<&str>) -> Vec<String> {
    let client = match token {
        Some(token) => rmcp_client_with_token(addr, token).await,
        None => rmcp_client(addr).await,
    };
    let listed = client
        .peer()
        .list_resources(None)
        .await
        .expect("resources/list must succeed");
    let mut uris: Vec<String> = listed
        .resources
        .iter()
        .map(|resource| resource.uri.to_string())
        .collect();
    uris.sort();
    uris
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

fn register_resource(
    handle: &McpListenerHandle,
    owner: &Arc<()>,
    uri: &str,
    route: &str,
    plan: Option<RouteSecurityPlan>,
    providers: Option<Arc<ProviderRegistry>>,
) -> mpsc::Receiver<McpResourceRead> {
    let (tx, rx) = mpsc::channel(8);
    handle
        .resource_registry
        .register_with_plan(
            uri.to_string(),
            route.to_string(),
            tx,
            Arc::downgrade(owner),
            plan.clone(),
        )
        .expect("resource must register");
    handle.resource_registry.mark_ready(uri);
    if let Some(plan) = plan {
        handle
            .security
            .register_plan(route, plan, providers, Arc::downgrade(owner));
    }
    rx
}
