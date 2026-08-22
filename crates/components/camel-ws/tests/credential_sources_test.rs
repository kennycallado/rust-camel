//! Kernel-path credential-sources tests for the WS consumer handshake.
//!
//! `finish-auth-flip` Task 1.1 port of the legacy credential-sources
//! suite: the component-owned auth arm is deleted, so the handshake
//! authenticates exclusively through the auth kernel — the route's
//! `RouteSecurityPlan` + `ProviderRegistry` ride the `SecurityContext`.
//! Extraction honors only the plan-declared credential sources and the
//! minted carrier rides every message exchange. The closing tests guard
//! the deletion itself: the production source stays free of the legacy
//! arm, and a plan-less context is Public pass-through (no extraction).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use camel_api::AuthPrincipal;
use camel_api::CamelError;
use camel_api::security_policy::{
    AccessMode, Principal, RouteSecurityPlan, SecurityPolicy, TransportId,
};
use camel_auth::CredentialSource;
use camel_auth::native_auth::{NativeCredential, NativeCredentialSecret, NativeCredentialStore};
use camel_auth::{
    ProviderEntry, ProviderRegistry, RolePolicy, StaticTokenAuthenticator, TokenAuthenticator,
    read_carrier,
};
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{ExchangeEnvelope, SecurityContext};
use camel_component_ws::{WsAppState, WsPathConfig, dispatch_handler};
use futures::SinkExt;
use tokio::sync::{RwLock, mpsc};
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::tungstenite::ClientRequestBuilder;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::protocol::Message as ClientMessage;

const PATH: &str = "/ws/auth";
const PROVIDER: &str = "idp-ws-cred";
const TOKEN: &str = "SENTINEL_CRED_1";
const QUERY_TOKEN: &str = "SENTINEL_WS_Q_1";
const SUBJECT: &str = "test-user";

/// Principal minted by the native store for the sentinel credentials.
fn native_principal() -> Principal {
    Principal {
        subject: SUBJECT.into(),
        issuer: "ws-cred-test".into(),
        audience: vec![],
        roles: vec![],
        scopes: vec![],
        claims: serde_json::Value::Null,
    }
}

/// Single-provider registry: `PROVIDER` mints `native_principal()` for the
/// caller-chosen token.
fn provider_registry(token: &str) -> Arc<ProviderRegistry> {
    let store = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: token.to_string().into(),
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

/// Authenticated plan declaring the given credential sources (mirrors the
/// kernel fixture in `kernel_auth_test.rs`).
fn kernel_plan(sources: Vec<CredentialSource>) -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(PROVIDER.into()),
        transport: TransportId::Ws,
        credential_sources: sources,
        audience_binding: None,
    }
}

/// Consumer security context carrying plan + providers (the kernel path).
///
/// Since the Task 1.1 arm deletion the handshake reads ONLY the kernel
/// fields; the policy constructor argument is an unread placeholder
/// (the authenticator field died in Task 1.3).
fn kernel_security_context(
    plan: RouteSecurityPlan,
    providers: Arc<ProviderRegistry>,
) -> SecurityContext {
    let policy: Arc<dyn SecurityPolicy> = Arc::new(RolePolicy::new(vec![], true));
    SecurityContext::from_arc(policy)
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
            route_id: "ws-cred-test-route".to_string(),
        },
        rx,
    )
}

/// Bind a real TCP listener and serve `dispatch_handler`, returning the port.
///
/// The listener is bound before `axum::serve` is spawned so the port is fixed
/// and immediately connectable (no bind race).
async fn spawn_server(state: WsAppState) -> (u16, tokio::task::JoinHandle<()>) {
    let app = Router::new().fallback(dispatch_handler).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (port, handle)
}

type ClientStream = tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Perform a real WS upgrade handshake with optional `Cookie` and
/// `Authorization` headers and an optional raw query string.
///
/// `Ok((stream, 101))` means the server answered `101 Switching Protocols`
/// (authorized); `Err(status)` carries the HTTP rejection status otherwise.
async fn connect_upgrade(
    port: u16,
    cookie: Option<&str>,
    auth: Option<&str>,
    query: Option<&str>,
) -> Result<(ClientStream, u16), u16> {
    let uri: http::Uri = match query {
        Some(q) => format!("ws://127.0.0.1:{port}{PATH}?{q}").parse().unwrap(),
        None => format!("ws://127.0.0.1:{port}{PATH}").parse().unwrap(),
    };
    let mut builder = ClientRequestBuilder::new(uri);
    if let Some(c) = cookie {
        builder = builder.with_header("Cookie", c);
    }
    if let Some(a) = auth {
        builder = builder.with_header("Authorization", a);
    }
    match tokio_tungstenite::connect_async(builder).await {
        Ok((stream, response)) => Ok((stream, response.status().as_u16())),
        Err(WsError::Http(response)) => Err(response.status().as_u16()),
        Err(e) => panic!("unexpected WS connect error: {e}"),
    }
}

/// Drive one granted handshake end-to-end: upgrade must answer 101, one
/// text message must reach the route body, and the dispatched exchange
/// must carry the kernel-minted carrier (`read_carrier` + provider id).
async fn assert_grant_with_carrier(
    port: u16,
    cookie: Option<&str>,
    auth: Option<&str>,
    query: Option<&str>,
    rx: &mut mpsc::Receiver<ExchangeEnvelope>,
) {
    let (mut client, status) = connect_upgrade(port, cookie, auth, query)
        .await
        .expect("kernel handshake must authorize (101)");
    assert_eq!(status, 101, "expected 101 upgrade");

    client
        .send(ClientMessage::Text("cred-body".into()))
        .await
        .unwrap();
    let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("message envelope dispatched")
        .expect("dispatch channel open");
    assert_eq!(
        envelope.exchange.input.body.as_text(),
        Some("cred-body"),
        "route body must run after the granted handshake"
    );
    let carrier =
        read_carrier(&envelope.exchange).expect("typed carrier missing on the message exchange");
    assert_eq!(carrier.provider_id(), PROVIDER);
    assert_eq!(carrier.principal().subject, SUBJECT);
}

#[tokio::test]
async fn ws_kernel_bearer_header_authenticates() {
    // Ported from `ws_default_header_only_unchanged` (positive arm): the
    // plan declares the Authorization header; a valid Bearer token
    // authenticates via the kernel and the carrier rides the exchange.
    let (state, mut rx) = make_app_state(
        PATH,
        kernel_security_context(
            kernel_plan(vec![CredentialSource::AuthorizationHeader]),
            provider_registry(TOKEN),
        ),
    );
    let (port, server) = spawn_server(state).await;
    let auth = format!("Bearer {TOKEN}");
    assert_grant_with_carrier(port, None, Some(&auth), None, &mut rx).await;
    server.abort();
}

#[tokio::test]
async fn ws_kernel_cookie_authenticates() {
    // Ported from `ws_cookie_source_authenticates`: a declared cookie
    // source with a matching cookie authorizes and mints the carrier.
    let (state, mut rx) = make_app_state(
        PATH,
        kernel_security_context(
            kernel_plan(vec![CredentialSource::Cookie {
                name: "session".into(),
            }]),
            provider_registry(TOKEN),
        ),
    );
    let (port, server) = spawn_server(state).await;
    let cookie = format!("session={TOKEN}");
    assert_grant_with_carrier(port, Some(&cookie), None, None, &mut rx).await;
    server.abort();
}

#[tokio::test]
async fn ws_kernel_token_outside_declared_sources_rejected() {
    // Ported from `ws_token_outside_declared_sources_rejected`: the plan
    // declares ONLY the Authorization header; a valid token presented in a
    // cookie (undeclared source) is ignored — extraction reads declared
    // sources only, so the handshake is refused 401 before any
    // authentication runs.
    let (state, _rx) = make_app_state(
        PATH,
        kernel_security_context(
            kernel_plan(vec![CredentialSource::AuthorizationHeader]),
            provider_registry(TOKEN),
        ),
    );
    let (port, server) = spawn_server(state).await;
    let cookie = format!("session={TOKEN}");
    let result = connect_upgrade(port, Some(&cookie), None, None).await;
    server.abort();
    assert_eq!(
        result.err(),
        Some(401),
        "token in an undeclared source must be ignored (declared-sources negative)"
    );
}

#[tokio::test]
async fn ws_kernel_missing_credential_denies() {
    // Ported from `ws_no_credential_rejected_before_eval`: a declared
    // cookie source but no credential on the wire — the handshake is
    // refused 401 before anything is evaluated.
    let (state, _rx) = make_app_state(
        PATH,
        kernel_security_context(
            kernel_plan(vec![CredentialSource::Cookie {
                name: "session".into(),
            }]),
            provider_registry(TOKEN),
        ),
    );
    let (port, server) = spawn_server(state).await;
    let result = connect_upgrade(port, None, None, None).await;
    server.abort();
    assert_eq!(
        result.err(),
        Some(401),
        "expected 401 (no credential in declared source)"
    );
}

/// Whether any tracing record captured into the shared buffer contains `needle`.
fn captured_logs_contain(buf: &std::sync::Mutex<Vec<u8>>, needle: &str) -> bool {
    String::from_utf8_lossy(&buf.lock().unwrap()).contains(needle)
}

/// Sentinel for the redaction sink: a `QueryParam` credential source whose
/// value must never be rendered by any diagnostic record during the upgrade
/// path. The connection AUTHENTICATES (the token is in the native store), so
/// this asserts redaction on the successful path, not on a 401.
#[tokio::test(flavor = "multi_thread")]
async fn ws_query_param_sentinel_redacted_in_upgrade_logs() {
    // Global subscriber writing every record into a shared buffer, mirroring
    // camel-test's `error_context_redacts_custom_header_sentinel` (per-crate
    // env filter + shared buffer + positive control) without new deps.
    struct SharedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = std::sync::Arc::clone(&buf);
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "camel_component_ws=trace",
        ))
        .with_writer(move || SharedWriter(std::sync::Arc::clone(&writer)))
        .try_init();

    // Test-only plan construction (Task 1.1 step 5): plan COMPILATION
    // rejects QueryParam sources for ws transports (unify-transport-auth
    // Task 1.8), so the fixture builds the plan struct directly — driving
    // the kernel branch, which calls the same ADR-0051 redaction helper
    // the deleted legacy arm used.
    let plan = RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(PROVIDER.into()),
        transport: TransportId::Ws,
        credential_sources: vec![CredentialSource::QueryParam {
            param: "sess".into(),
        }],
        audience_binding: None,
    };
    let (state, _rx) = make_app_state(
        PATH,
        kernel_security_context(plan, provider_registry(QUERY_TOKEN)),
    );
    let (port, server) = spawn_server(state).await;
    let result = connect_upgrade(port, None, None, Some(&format!("sess={QUERY_TOKEN}"))).await;
    server.abort();
    let (_stream, status) = result.expect("expected 101 (query source authorized)");
    assert_eq!(status, 101);

    assert!(
        !captured_logs_contain(&buf, QUERY_TOKEN),
        "no camel_component_ws record during WS upgrade may render the query credential"
    );
    // Permanent positive control: the redacted query-token debug line fires on
    // the successful path and must be captured by the test subscriber.
    assert!(
        captured_logs_contain(&buf, "WS upgrade with query token (redacted)"),
        "positive control: the redacted query-token debug line must be captured"
    );
}

/// `TokenAuthenticator` that counts every authenticate call: a plan-less
/// context must never reach ANY provider.
struct CountingProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl TokenAuthenticator for CountingProvider {
    async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(native_principal())
    }
}

#[tokio::test]
async fn ws_public_route_no_extraction_counted_provider() {
    // Registry WITHOUT a plan: post-1.1 a context without a kernel plan is
    // Public pass-through — no extraction, so the provider must never be
    // consulted even though the client presents a credential the provider
    // would accept (the deleted legacy arm would have called it).
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = ProviderRegistry::new();
    registry.register(
        PROVIDER,
        ProviderEntry {
            authenticator: Arc::new(CountingProvider {
                calls: Arc::clone(&calls),
            }),
            audience_binding: None,
        },
    );
    let sec_ctx = SecurityContext::from_arc(Arc::new(RolePolicy::new(vec![], true)))
        .with_providers(Arc::new(registry));
    let (state, mut rx) = make_app_state(PATH, sec_ctx);
    let (port, server) = spawn_server(state).await;

    let auth = format!("Bearer {TOKEN}");
    let (mut client, status) = connect_upgrade(port, None, Some(&auth), None)
        .await
        .expect("plan-less route is Public pass-through");
    assert_eq!(status, 101);
    client
        .send(ClientMessage::Text("public-body".into()))
        .await
        .unwrap();
    let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("public route body must run")
        .expect("dispatch channel open");
    assert_eq!(envelope.exchange.input.body.as_text(), Some("public-body"));
    assert!(
        read_carrier(&envelope.exchange).is_none(),
        "pass-through must not mint a carrier"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "Public pass-through must never consult the provider"
    );

    server.abort();
}

/// Recursively collect the `.rs` files under `dir`.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Task 1.1 guard: the legacy component-owned auth arm stays deleted.
/// Scans every production source file under `src/` for the legacy markers
/// (test-file comment mentions are out of scope by construction — the
/// scan never leaves `src/`).
#[test]
fn ws_legacy_arm_deleted_source_scan() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    assert!(
        !files.is_empty(),
        "source scan must find production sources"
    );
    for file in files {
        let content = std::fs::read_to_string(&file).expect("read source file");
        assert!(
            !content.contains("LegacyPrincipal"),
            "legacy marker `LegacyPrincipal` must stay deleted: {}",
            file.display()
        );
        assert!(
            !content.contains("sec_ctx.authenticator"),
            "legacy marker `sec_ctx.authenticator` must stay deleted: {}",
            file.display()
        );
    }
}
