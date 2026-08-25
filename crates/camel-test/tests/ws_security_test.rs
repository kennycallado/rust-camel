use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, Version, header};
use camel_api::security_policy::{
    AccessMode, AuthContext, AuthorizationDecision, Principal, RouteSecurityPlan, SecurityPolicy,
    TransportId,
};
use camel_api::{CamelError, Exchange};
use camel_auth::{CredentialSource, ProviderEntry, ProviderRegistry, TokenAuthenticator};
use camel_component_api::{ExchangeEnvelope, SecurityContext};
use camel_component_ws::{WsAppState, WsPathConfig, dispatch_handler};
use serde_json::json;
use tokio::sync::{RwLock, mpsc};
use tower::ServiceExt;

// --- Test helpers ---

/// What the mock provider does with a presented bearer token.
enum MockOutcome {
    /// Mints the test principal.
    Principal,
    /// Rejects the token — the 401 idiom.
    InvalidToken,
    /// Fails with `CamelError::AuthProviderUnavailable`; the variant drives the 503 mapping in the
    /// transport's upgrade-error conversion.
    ProviderUnavailable,
    /// generic `ProcessorError` — always the 500 path; status selection is
    /// variant-based
    Error(&'static str),
}

const MOCK_PROVIDER: &str = "idp-mock";

struct MockAuthenticator {
    outcome: MockOutcome,
}

#[async_trait]
impl TokenAuthenticator for MockAuthenticator {
    async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
        match &self.outcome {
            MockOutcome::Principal => Ok(test_principal()),
            MockOutcome::InvalidToken => Err(CamelError::Unauthenticated("invalid token".into())),
            MockOutcome::ProviderUnavailable => Err(CamelError::AuthProviderUnavailable(
                "fixture: idp unreachable".into(),
            )),
            MockOutcome::Error(msg) => Err(CamelError::ProcessorError((*msg).into())),
        }
    }
}

/// Placeholder policy — inert at the transport since the kernel flip: the
/// handshake reads only the plan + providers, authorization belongs to the
/// pipeline.
struct AlwaysGrantPolicy;

#[async_trait]
impl SecurityPolicy for AlwaysGrantPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
        _auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        Ok(AuthorizationDecision::Granted {
            principal: test_principal(),
        })
    }
}

fn test_principal() -> Principal {
    Principal {
        subject: "test-user".into(),
        issuer: "test-issuer".into(),
        audience: vec!["api".into()],
        scopes: vec!["read".into(), "write".into()],
        roles: vec!["admin".into()],
        claims: json!({"sub": "test-user"}),
    }
}

/// Kernel-path security context: an `Authenticated` plan over a single
/// mock provider (`MOCK_PROVIDER`), AuthorizationHeader credential source.
///
/// Ported from the legacy `SecurityContext::new(policy, authenticator)`
/// fixture when the authenticator hand-off died (`finish-auth-flip` 1.3):
/// the handshake authenticates through the kernel plan + provider
/// registry, never a context-held authenticator.
fn kernel_security_context(outcome: MockOutcome) -> SecurityContext {
    let registry = ProviderRegistry::new();
    registry.register(
        MOCK_PROVIDER,
        ProviderEntry {
            authenticator: Arc::new(MockAuthenticator { outcome }),
            audience_binding: None,
        },
    );
    let plan = RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(MOCK_PROVIDER.into()),
        transport: TransportId::Ws,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    };
    SecurityContext::from_arc(Arc::new(AlwaysGrantPolicy))
        .with_plan(plan)
        .with_providers(Arc::new(registry))
}

fn make_app_state(path: &str, sec_ctx: Option<SecurityContext>) -> WsAppState {
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
    if let Some(ctx) = sec_ctx {
        path_policies.insert(path.to_string(), ctx);
    }
    WsAppState {
        dispatch,
        path_configs,
        path_policies,
        server_error: Arc::new(AtomicBool::new(false)),
        runtime: Arc::new(camel_component_api::test_support::NoopRuntimeObservability),
        route_id: "test-route".to_string(),
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn ws_upgrade_request(port: u16, path: &str, auth_header: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri(format!("http://127.0.0.1:{}{}", port, path))
        .version(Version::HTTP_11)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
        .header(header::SEC_WEBSOCKET_VERSION, "13");

    if let Some(auth) = auth_header {
        builder = builder.header(header::AUTHORIZATION, auth);
    }

    builder.body(Body::empty()).unwrap()
}

// --- Tests ---

/// Verifies that a WebSocket upgrade request without an Authorization
/// header returns 401 when the path has an Authenticated kernel plan.
#[tokio::test]
async fn test_ws_401_without_auth() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(path, Some(kernel_security_context(MockOutcome::Principal)));

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Verifies that a WebSocket upgrade request with an invalid token
/// returns 401 when the kernel provider rejects it.
#[tokio::test]
async fn test_ws_401_invalid_token() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(
        path,
        Some(kernel_security_context(MockOutcome::InvalidToken)),
    );

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Bearer invalid-token"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Verifies that a provider reported unavailable surfaces as 503 at the
/// upgrade. Ported from the legacy 403 policy-deny test: handshake-level
/// policy evaluation is gone (authorization belongs to the pipeline), so
/// the provider-unavailable branch is the kernel path's non-401 denial.
#[tokio::test]
async fn test_ws_503_provider_unavailable() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(
        path,
        Some(kernel_security_context(MockOutcome::ProviderUnavailable)),
    );

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Bearer valid-token"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

/// Marker substring alone must NOT yield 503 — status selection is variant-based (spec: wording independence).
#[tokio::test]
async fn test_ws_500_generic_processor_error_with_marker() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(
        path,
        Some(kernel_security_context(MockOutcome::Error(
            "auth provider unavailable: fixture",
        ))),
    );

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Bearer valid-token"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// Verifies that a WebSocket upgrade request with a valid token accepted
/// by the kernel provider passes auth (response is not 401/403).
/// Note: Full 101 upgrade requires hyper's OnUpgrade extension which is
/// only available with a real server, not via tower::ServiceExt::oneshot.
#[tokio::test]
async fn test_ws_auth_passes_with_valid_token() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(path, Some(kernel_security_context(MockOutcome::Principal)));

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Bearer valid-token"));
    let resp = app.oneshot(req).await.unwrap();
    // Auth passed — the upgrade proceeds; the 400 comes from the missing
    // hyper::upgrade::OnUpgrade extension (only a real HTTP server provides
    // it). Post-flip, 401/500/503 at this point would be a kernel or
    // provider regression.
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Verifies that a WebSocket upgrade request to an unprotected path
/// (no security policy) passes auth without any Authorization header.
#[tokio::test]
async fn test_ws_unprotected_path_skips_auth() {
    let port = free_port();
    let path = "/ws/open";
    // No security context — path_policies stays empty
    let state = make_app_state(path, None);

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, None);
    let resp = app.oneshot(req).await.unwrap();
    // No auth required — response is not 401 or 403.
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);
}

/// Verifies that a WebSocket upgrade request whose provider fails with an
/// unexpected error (not unauthenticated, not unavailable) returns 500.
#[tokio::test]
async fn test_ws_500_provider_error() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(
        path,
        Some(kernel_security_context(MockOutcome::Error("policy error"))),
    );

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Bearer valid-token"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// Verifies that a WebSocket upgrade request with a malformed Authorization
/// header (wrong scheme, e.g. "Basic" instead of "Bearer") extracts no
/// credential from the AuthorizationHeader source and returns 401.
#[tokio::test]
async fn test_ws_401_malformed_auth_scheme() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(path, Some(kernel_security_context(MockOutcome::Principal)));

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Basic abc123"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
