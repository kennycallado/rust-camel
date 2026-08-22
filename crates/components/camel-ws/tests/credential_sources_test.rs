//! Integration tests for the credential-sources activation on the WS consumer.
//!
//! The component authenticates a token extracted from the route-declared
//! credential sources and passes the resulting principal to the policy; the
//! `trust_upstream_principal` flag is removed (pre-1.0 breaking) so these
//! routes no longer carry an exchange-property trust branch.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use axum::Router;
use camel_api::security_policy::{Principal, SecurityPolicy};
use camel_auth::CredentialSource;
use camel_auth::native_auth::{NativeCredential, NativeCredentialSecret, NativeCredentialStore};
use camel_auth::{RolePolicy, StaticTokenAuthenticator, TokenAuthenticator};
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{ExchangeEnvelope, SecurityContext};
use camel_component_ws::{WsAppState, WsPathConfig, dispatch_handler};
use tokio::sync::{RwLock, mpsc};
use tokio_tungstenite::tungstenite::ClientRequestBuilder;
use tokio_tungstenite::tungstenite::Error as WsError;

const SENTINEL_CRED_1: &str = "SENTINEL_CRED_1";
const SENTINEL_WS_Q_1: &str = "SENTINEL_WS_Q_1";
const ROLE: &str = "admin";
const PATH: &str = "/ws/auth";

/// Principal minted by the native store for `SENTINEL_CRED_1`.
fn principal() -> Principal {
    Principal {
        subject: "test-user".into(),
        issuer: "test".into(),
        audience: vec![],
        roles: vec![ROLE.into()],
        scopes: vec![],
        claims: serde_json::Value::Null,
    }
}

/// Build the consumer `SecurityContext`: the `RolePolicy` requires the role and
/// the consumer `SecurityContext` carries the declared source list via
/// `with_credential_sources`. The native store holds `SENTINEL_CRED_1`.
fn security_context(sources: Vec<CredentialSource>) -> SecurityContext {
    security_context_with_token(sources, SENTINEL_CRED_1)
}

/// `security_context` with a caller-chosen native-store credential value.
fn security_context_with_token(sources: Vec<CredentialSource>, token: &str) -> SecurityContext {
    let store = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: token.to_string().into(),
        },
        principal: principal(),
    }])
    .unwrap();
    let authenticator: Arc<dyn TokenAuthenticator> = Arc::new(StaticTokenAuthenticator::new(store));
    let policy: Arc<dyn SecurityPolicy> = Arc::new(RolePolicy::new(vec![ROLE.into()], true));
    SecurityContext::from_arc(policy, authenticator).with_credential_sources(sources)
}

/// Build the WS app state for one path with the given security context.
fn make_app_state(path: &str, sec_ctx: SecurityContext) -> WsAppState {
    let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(64);
    let dispatch: Arc<RwLock<HashMap<String, mpsc::Sender<ExchangeEnvelope>>>> =
        Arc::new(RwLock::new([(path.to_string(), tx)].into_iter().collect()));
    let path_configs = Arc::new(dashmap::DashMap::new());
    path_configs.insert(
        path.to_string(),
        WsPathConfig {
            max_connections: 100,
            max_message_size: 65536,
            heartbeat_interval: std::time::Duration::ZERO,
            idle_timeout: std::time::Duration::ZERO,
            allow_origin: "*".into(),
        },
    );
    let path_policies = Arc::new(dashmap::DashMap::new());
    path_policies.insert(path.to_string(), sec_ctx);
    WsAppState {
        dispatch,
        path_configs,
        path_policies,
        server_error: Arc::new(AtomicBool::new(false)),
        runtime: Arc::new(NoopRuntimeObservability),
        route_id: "test-route".to_string(),
    }
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

/// Perform a real WS upgrade handshake with an optional `Cookie` header and an
/// optional `Authorization` header.
///
/// `Ok(())` means the server answered `101 Switching Protocols` (authorized);
/// `Err(status)` carries the HTTP rejection status otherwise.
async fn upgrade(port: u16, cookie: Option<&str>, auth: Option<&str>) -> Result<(), u16> {
    let uri: http::Uri = format!("ws://127.0.0.1:{port}{PATH}").parse().unwrap();
    let mut builder = ClientRequestBuilder::new(uri);
    if let Some(c) = cookie {
        builder = builder.with_header("Cookie", c);
    }
    if let Some(a) = auth {
        builder = builder.with_header("Authorization", a);
    }
    match tokio_tungstenite::connect_async(builder).await {
        Ok((_stream, response)) => {
            assert_eq!(response.status(), 101, "expected 101 upgrade");
            Ok(())
        }
        Err(WsError::Http(response)) => Err(response.status().as_u16()),
        Err(e) => panic!("unexpected WS connect error: {e}"),
    }
}

/// Like `upgrade`, but appends a raw query string to the WS URI (no cookie or
/// Authorization header), for exercising the `QueryParam` credential source.
async fn upgrade_query(port: u16, query: &str) -> Result<(), u16> {
    let uri: http::Uri = format!("ws://127.0.0.1:{port}{PATH}?{query}")
        .parse()
        .unwrap();
    let builder = ClientRequestBuilder::new(uri);
    match tokio_tungstenite::connect_async(builder).await {
        Ok((_stream, response)) => {
            assert_eq!(response.status(), 101, "expected 101 upgrade");
            Ok(())
        }
        Err(WsError::Http(response)) => Err(response.status().as_u16()),
        Err(e) => panic!("unexpected WS connect error: {e}"),
    }
}

#[tokio::test]
async fn ws_cookie_source_authenticates() {
    let state = make_app_state(
        PATH,
        security_context(vec![CredentialSource::Cookie {
            name: "session".into(),
        }]),
    );
    let (port, server) = spawn_server(state).await;
    let cookie = format!("session={SENTINEL_CRED_1}");
    let result = upgrade(port, Some(&cookie), None).await;
    server.abort();
    match result {
        Ok(()) => {}
        Err(status) => panic!("expected 101 (authorized), got rejection status {status}"),
    }
}

#[tokio::test]
async fn ws_token_outside_declared_sources_rejected() {
    // Cookie-only route. A valid token presented in the Authorization header
    // (not a declared source) is rejected: the component extracts credentials
    // only from declared sources, so the route stays fail-closed.
    let state = make_app_state(
        PATH,
        security_context(vec![CredentialSource::Cookie {
            name: "session".into(),
        }]),
    );
    let (port, server) = spawn_server(state).await;
    let auth = format!("Bearer {SENTINEL_CRED_1}");
    let result = upgrade(port, None, Some(&auth)).await;
    server.abort();
    assert_eq!(result, Err(401), "expected 401 (unauthenticated) rejection");
}

#[tokio::test]
async fn ws_default_header_only_unchanged() {
    // Roles route with NO `credential_sources` — the default source list stays
    // header-only.
    let state = make_app_state(
        PATH,
        security_context(vec![CredentialSource::AuthorizationHeader]),
    );
    let (port, server) = spawn_server(state).await;

    // Valid Bearer -> 101.
    let auth = format!("Bearer {SENTINEL_CRED_1}");
    let ok = upgrade(port, None, Some(&auth)).await;
    assert_eq!(
        ok,
        Ok(()),
        "valid Bearer must authorize (default header source)"
    );

    // Valid cookie only -> rejected (cookie is not a default source).
    let cookie = format!("session={SENTINEL_CRED_1}");
    let rejected = upgrade(port, Some(&cookie), None).await;
    assert_eq!(
        rejected,
        Err(401),
        "cookie must not authorize when the route has no cookie source"
    );

    server.abort();
}

#[tokio::test]
async fn ws_no_credential_rejected_before_eval() {
    // Declared cookie source, but the client presents no credential in any
    // declared source. The WS no-source path rejects with 401 before policy
    // evaluation runs.
    let state = make_app_state(
        PATH,
        security_context(vec![CredentialSource::Cookie {
            name: "session".into(),
        }]),
    );
    let (port, server) = spawn_server(state).await;
    let result = upgrade(port, None, None).await;
    server.abort();
    assert_eq!(
        result,
        Err(401),
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

    let state = make_app_state(
        PATH,
        security_context_with_token(
            vec![CredentialSource::QueryParam {
                param: "sess".into(),
            }],
            SENTINEL_WS_Q_1,
        ),
    );
    let (port, server) = spawn_server(state).await;
    let result = upgrade_query(port, &format!("sess={SENTINEL_WS_Q_1}")).await;
    server.abort();
    assert_eq!(result, Ok(()), "expected 101 (query source authorized)");

    assert!(
        !captured_logs_contain(&buf, SENTINEL_WS_Q_1),
        "no camel_component_ws record during WS upgrade may render the query credential"
    );
    // Permanent positive control: the redacted query-token debug line fires on
    // the successful path and must be captured by the test subscriber.
    assert!(
        captured_logs_contain(&buf, "WS upgrade with query token (redacted)"),
        "positive control: the redacted query-token debug line must be captured"
    );
}
