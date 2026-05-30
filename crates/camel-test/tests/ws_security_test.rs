use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, Version, header};
use camel_api::security_policy::{AuthorizationDecision, Principal, SecurityPolicy};
use camel_api::{CamelError, Exchange};
use camel_auth::TokenAuthenticator;
use camel_component_api::{ExchangeEnvelope, SecurityContext};
use camel_component_ws::{WsAppState, WsPathConfig, dispatch_handler};
use serde_json::json;
use tokio::sync::{RwLock, mpsc};
use tower::ServiceExt;

// --- Test helpers ---

struct MockAuthenticator {
    should_succeed: bool,
}

#[async_trait]
impl TokenAuthenticator for MockAuthenticator {
    async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
        if self.should_succeed {
            Ok(test_principal())
        } else {
            Err(CamelError::Unauthenticated("invalid token".into()))
        }
    }
}

struct AlwaysGrantPolicy;

#[async_trait]
impl SecurityPolicy for AlwaysGrantPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
    ) -> Result<AuthorizationDecision, CamelError> {
        Ok(AuthorizationDecision::Granted {
            principal: test_principal(),
        })
    }
}

struct AlwaysDenyPolicy;

#[async_trait]
impl SecurityPolicy for AlwaysDenyPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
    ) -> Result<AuthorizationDecision, CamelError> {
        Ok(AuthorizationDecision::Denied {
            reason: "no roles assigned".into(),
            required: vec!["admin".into()],
            actual: vec![],
        })
    }
}

struct FailPolicy;

#[async_trait]
impl SecurityPolicy for FailPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
    ) -> Result<AuthorizationDecision, CamelError> {
        Err(CamelError::ProcessorError("policy error".into()))
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

fn test_security_context(
    authenticator: impl TokenAuthenticator + 'static,
    policy: impl SecurityPolicy + 'static,
) -> SecurityContext {
    SecurityContext::new(policy, Arc::new(authenticator))
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

/// Verifies that a WebSocket upgrade request without an Authorization header
/// returns 401 when the path has a security policy configured.
#[tokio::test]
async fn test_ws_401_without_auth() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(
        path,
        Some(test_security_context(
            MockAuthenticator {
                should_succeed: true,
            },
            AlwaysGrantPolicy,
        )),
    );

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Verifies that a WebSocket upgrade request with an invalid token
/// returns 401 when the path has a security policy configured.
#[tokio::test]
async fn test_ws_401_invalid_token() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(
        path,
        Some(test_security_context(
            MockAuthenticator {
                should_succeed: false,
            },
            AlwaysGrantPolicy,
        )),
    );

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Bearer invalid-token"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Verifies that a WebSocket upgrade request with a valid token but
/// a denying security policy returns 403.
#[tokio::test]
async fn test_ws_403_policy_deny() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(
        path,
        Some(test_security_context(
            MockAuthenticator {
                should_succeed: true,
            },
            AlwaysDenyPolicy,
        )),
    );

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Bearer valid-token"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// Verifies that a WebSocket upgrade request with a valid token and
/// granting policy passes auth (response is not 401/403).
/// Note: Full 101 upgrade requires hyper's OnUpgrade extension which is
/// only available with a real server, not via tower::ServiceExt::oneshot.
#[tokio::test]
async fn test_ws_auth_passes_with_valid_token() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(
        path,
        Some(test_security_context(
            MockAuthenticator {
                should_succeed: true,
            },
            AlwaysGrantPolicy,
        )),
    );

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Bearer valid-token"));
    let resp = app.oneshot(req).await.unwrap();
    // Auth passed — response is not 401 or 403.
    // The 400 we get is from missing hyper::upgrade::OnUpgrade extension,
    // which can only be provided by a real HTTP server.
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);
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

/// Verifies that a WebSocket upgrade request with a valid token but
/// a failing security policy (evaluate returns Err) returns 500.
#[tokio::test]
async fn test_ws_500_policy_error() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(
        path,
        Some(test_security_context(
            MockAuthenticator {
                should_succeed: true,
            },
            FailPolicy,
        )),
    );

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Bearer valid-token"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// Verifies that a WebSocket upgrade request with a malformed Authorization
/// header (wrong scheme, e.g. "Basic" instead of "Bearer") returns 401.
#[tokio::test]
async fn test_ws_401_malformed_auth_scheme() {
    let port = free_port();
    let path = "/ws/auth";
    let state = make_app_state(
        path,
        Some(test_security_context(
            MockAuthenticator {
                should_succeed: true,
            },
            AlwaysGrantPolicy,
        )),
    );

    let app = Router::new().fallback(dispatch_handler).with_state(state);

    let req = ws_upgrade_request(port, path, Some("Basic abc123"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
