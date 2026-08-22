//! Kernel-converged WS authentication (`unify-transport-auth`, Task 2.8).
//!
//! The handshake authenticates through the auth kernel: the route's
//! `RouteSecurityPlan` + `ProviderRegistry` ride the `SecurityContext`
//! captured at consumer construction (before any handshake can run). A
//! `Public` plan skips credential extraction entirely; any other mode
//! extracts per the plan sources and mints the sealed principal via
//! `kernel_authenticate`. The minted carrier lives in connection state and
//! `install_carrier` stamps it on EVERY per-message exchange (the text and
//! binary construction sites) — Task 2.9's dispatch enforcement reads it
//! off every exchange, not just the first.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use axum::Router;
use camel_api::AuthPrincipal;
use camel_api::security_policy::{
    AccessMode, Principal, RouteSecurityPlan, SecurityPolicy, TransportId,
};
use camel_auth::native_auth::{NativeCredential, NativeCredentialSecret, NativeCredentialStore};
use camel_auth::{
    CredentialSource, ProviderEntry, ProviderRegistry, RolePolicy, StaticTokenAuthenticator,
    TokenAuthenticator, read_carrier,
};
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{ExchangeEnvelope, SecurityContext};
use camel_component_ws::{WsAppState, WsPathConfig, dispatch_handler};
use futures::SinkExt;
use tokio::sync::{RwLock, mpsc};
use tokio_tungstenite::tungstenite::ClientRequestBuilder;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::protocol::Message as ClientMessage;

const PATH: &str = "/ws/kernel";
const PROVIDER: &str = "idp-ws";
const TOKEN: &str = "SENTINEL_KERNEL_TOKEN";
const SUBJECT: &str = "svc-kernel-ws";

/// Principal minted by the native store for the kernel sentinel token.
fn native_principal() -> Principal {
    Principal {
        subject: SUBJECT.into(),
        issuer: "ws-kernel-test".into(),
        audience: vec![],
        scopes: vec![],
        roles: vec![],
        claims: serde_json::Value::Null,
    }
}

/// Registry holding one static-token provider (`PROVIDER` → `TOKEN`).
fn provider_registry() -> Arc<ProviderRegistry> {
    let store = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: TOKEN.to_string().into(),
        },
        principal: native_principal(),
    }])
    .unwrap();
    let registry = ProviderRegistry::new();
    registry.register(
        PROVIDER,
        ProviderEntry {
            authenticator: Arc::new(StaticTokenAuthenticator::new(store)),
            audience_binding: None,
        },
    );
    Arc::new(registry)
}

fn authenticated_plan() -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(PROVIDER.into()),
        transport: TransportId::Ws,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    }
}

fn public_plan() -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Public,
        provider_ref: None,
        transport: TransportId::Ws,
        credential_sources: vec![],
        audience_binding: None,
    }
}

/// Consumer security context carrying plan + providers (the kernel path);
/// the legacy policy/authenticator fields stay populated so a wiring bug
/// that falls back to the legacy path is visible in the carrier asserts,
/// not masked by an unrelated denial.
fn kernel_security_context(
    plan: RouteSecurityPlan,
    providers: Arc<ProviderRegistry>,
) -> SecurityContext {
    let store = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: TOKEN.to_string().into(),
        },
        principal: native_principal(),
    }])
    .unwrap();
    let authenticator: Arc<dyn TokenAuthenticator> = Arc::new(StaticTokenAuthenticator::new(store));
    let policy: Arc<dyn SecurityPolicy> = Arc::new(RolePolicy::new(vec![], true));
    SecurityContext::from_arc(policy, authenticator)
        .with_plan(plan)
        .with_providers(providers)
}

/// Build the WS app state for one path, keeping the dispatch receiver so
/// tests can inspect the per-message exchanges.
fn make_app_state(
    path: &str,
    sec_ctx: SecurityContext,
) -> (WsAppState, mpsc::Receiver<ExchangeEnvelope>) {
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(64);
    let dispatch: Arc<RwLock<HashMap<String, mpsc::Sender<ExchangeEnvelope>>>> =
        Arc::new(RwLock::new([(path.to_string(), tx)].into_iter().collect()));
    let path_configs = Arc::new(dashmap::DashMap::new());
    path_configs.insert(
        path.to_string(),
        WsPathConfig {
            max_connections: 100,
            max_message_size: 65536,
            heartbeat_interval: Duration::ZERO,
            idle_timeout: Duration::ZERO,
            allow_origin: "*".into(),
        },
    );
    let path_policies = Arc::new(dashmap::DashMap::new());
    path_policies.insert(path.to_string(), sec_ctx);
    (
        WsAppState {
            dispatch,
            path_configs,
            path_policies,
            server_error: Arc::new(AtomicBool::new(false)),
            runtime: Arc::new(NoopRuntimeObservability),
            route_id: "ws-kernel-test-route".to_string(),
        },
        rx,
    )
}

/// Bind a real TCP listener and serve `dispatch_handler`, returning the port.
async fn spawn_server(state: WsAppState) -> (u16, tokio::task::JoinHandle<()>) {
    let app = Router::new().fallback(dispatch_handler).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (port, handle)
}

fn upgrade_uri(port: u16) -> http::Uri {
    format!("ws://127.0.0.1:{port}{PATH}").parse().unwrap()
}

#[tokio::test]
async fn ws_kernel_handshake_grants() {
    let (state, mut rx) = make_app_state(
        PATH,
        kernel_security_context(authenticated_plan(), provider_registry()),
    );
    let (port, server) = spawn_server(state).await;

    // Valid token in the plan-declared source: the upgrade completes.
    let builder = ClientRequestBuilder::new(upgrade_uri(port))
        .with_header("Authorization", format!("Bearer {TOKEN}"));
    let (mut client, response) = tokio_tungstenite::connect_async(builder).await.unwrap();
    assert_eq!(response.status(), 101, "kernel handshake must authorize");

    // TWO messages, one through each construction site (text + binary): a
    // FRESH exchange is built per inbound message, so the typed carrier
    // must ride BOTH — Task 2.9's dispatch enforcement reads it off every
    // exchange, not just the first.
    client
        .send(ClientMessage::Text("first".into()))
        .await
        .unwrap();
    client
        .send(ClientMessage::Binary("second".as_bytes().to_vec().into()))
        .await
        .unwrap();

    // First (text site): body + typed carrier preserved.
    let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("first message envelope dispatched")
        .expect("dispatch channel open");
    assert_eq!(
        envelope.exchange.input.body.as_text(),
        Some("first"),
        "first message body must reach the route"
    );
    let carrier =
        read_carrier(&envelope.exchange).expect("typed carrier missing on FIRST message exchange");
    assert_eq!(carrier.provider_id(), PROVIDER);
    assert_eq!(carrier.principal().subject, SUBJECT);

    // Second (binary site): the carrier mandate — still present.
    let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("second message envelope dispatched")
        .expect("dispatch channel open");
    assert!(matches!(
        envelope.exchange.input.body,
        camel_api::Body::Bytes(ref b) if b.as_ref() == b"second"
    ));
    let carrier =
        read_carrier(&envelope.exchange).expect("typed carrier missing on SECOND message exchange");
    assert_eq!(carrier.provider_id(), PROVIDER);
    assert_eq!(carrier.principal().subject, SUBJECT);

    server.abort();
}

#[tokio::test]
async fn ws_kernel_handshake_denies() {
    let (state, _rx) = make_app_state(
        PATH,
        kernel_security_context(authenticated_plan(), provider_registry()),
    );
    let (port, server) = spawn_server(state).await;

    // Invalid token: the kernel denies and the handshake is refused with
    // the ws denial idiom (HTTP 401 — the upgrade never completes).
    let builder = ClientRequestBuilder::new(upgrade_uri(port))
        .with_header("Authorization", "Bearer wrong-token");
    match tokio_tungstenite::connect_async(builder).await {
        Err(WsError::Http(response)) => {
            assert_eq!(response.status().as_u16(), 401, "kernel denial status");
        }
        Err(e) => panic!("expected HTTP denial, got: {e}"),
        Ok(_) => panic!("invalid token must not complete the handshake"),
    }

    server.abort();
}

#[tokio::test]
async fn ws_public_route_passes_without_extraction() {
    // Public plan, no credentials on the wire: the upgrade completes and
    // the route body runs. Extraction is never attempted — the legacy
    // no-credential branch would deny 401 here, so reaching the route
    // proves the pass-through.
    let (state, mut rx) = make_app_state(
        PATH,
        kernel_security_context(public_plan(), provider_registry()),
    );
    let (port, server) = spawn_server(state).await;

    let builder = ClientRequestBuilder::new(upgrade_uri(port));
    let (mut client, response) = tokio_tungstenite::connect_async(builder).await.unwrap();
    assert_eq!(response.status(), 101, "public plan must pass through");

    client
        .send(ClientMessage::Text("public-body".into()))
        .await
        .unwrap();
    let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("public route body must run")
        .expect("dispatch channel open");
    assert_eq!(envelope.exchange.input.body.as_text(), Some("public-body"));

    // Pass-through mints no principal: no carrier is expected.
    assert!(
        read_carrier(&envelope.exchange).is_none(),
        "public pass-through must not mint a carrier"
    );

    server.abort();
}
