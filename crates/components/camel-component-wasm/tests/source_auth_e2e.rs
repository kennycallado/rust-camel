//! Host-edge handshake e2e tests for [`WasmSourceConsumer`]
//! (`wasm-source-auth-kernel`, Tasks 2.2 + 2.3): the axum handler denies
//! unauthenticated requests with 401 BEFORE the request channel is touched,
//! so the guest never wakes. A blessed `Public` classification (plan-only
//! context) passes through with the 202-immediate-ack semantics unchanged.
//! Task 2.3 adds the credential-source matrix — one permitted source per
//! plan (AuthorizationHeader, named Header, Cookie, QueryParam), each
//! authenticating against a real two-provider static-token fixture
//! registry — plus the provider-substitution denial (valid token from the
//! wrong provider → 401, no guest wakeup).
//!
//! # Prerequisites
//!
//! All tests in this target are `#[ignore]` (ADR-0054): they start real
//! guest fixtures.
//!
//! 1. Webhook guest at
//!    `examples/wasm-source-webhook/guest/target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm`
//!    ```sh
//!    cd examples/wasm-source-webhook/guest
//!    cargo build --target wasm32-wasip2
//!    ```
//!
//! 2. A free TCP port per test (tests use unique ports to run in parallel).
//!
//! Run with:
//! ```sh
//! cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use camel_api::security_policy::{
    AccessMode, CredentialSource, Principal, RouteSecurityPlan, TransportId,
};
use camel_auth::native_auth::{
    NativeCredential, NativeCredentialSecret, NativeCredentialStore, StaticTokenAuthenticator,
};
use camel_auth::{ProviderEntry, ProviderRegistry, RolePolicy, read_carrier};
use camel_component_api::consumer::{ConsumerContext, ExchangeEnvelope};
use camel_component_api::{Consumer, SecurityContext};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use camel_component_wasm::config::WasmConfig;
use camel_component_wasm::source_consumer::WasmSourceConsumer;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Pre-built webhook guest wasm (relative to workspace root).
const WEBHOOK_GUEST_WASM_REL: &str =
    "examples/wasm-source-webhook/guest/target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm";

/// Timeout for the guest to bind its HTTP listener and be ready.
const BIND_WAIT: Duration = Duration::from_secs(5);

/// Timeout for stop() to complete cleanly.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Settling window before asserting the pipeline channel stayed empty: any
/// forwarded request would reach it near-instantly (202 is immediate).
const SETTLE: Duration = Duration::from_millis(300);

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Resolve a pre-built guest wasm path (same two-location scheme as
/// `tests/source_bind_gate.rs`): `$CARGO_TARGET_DIR/wasm32-wasip2/debug/…`
/// first, then the per-guest default location relative to the workspace
/// root. Returns `None` if the file doesn't exist (prerequisite not met).
fn guest_wasm_path(file: &str, default_rel: &str) -> Option<PathBuf> {
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let path = PathBuf::from(target_dir)
            .join("wasm32-wasip2/debug")
            .join(file);
        if path.exists() {
            return Some(path);
        }
    }

    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir
        .parent() // components/
        .and_then(|p| p.parent()) // crates/
        .and_then(|p| p.parent()); // workspace root

    let path = workspace_root?.join(default_rel);
    if path.exists() { Some(path) } else { None }
}

fn require_webhook_guest_wasm() -> PathBuf {
    guest_wasm_path("wasm_source_webhook_guest.wasm", WEBHOOK_GUEST_WASM_REL).expect(
        "Webhook guest wasm not found. Build with:\n\
         cd examples/wasm-source-webhook/guest && cargo build --target wasm32-wasip2",
    )
}

/// Authenticated `wasm:` classification (same fixture as
/// `tests/source_auth.rs`, now driven through a real guest start).
fn authenticated_plan() -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some("idp-wasm".to_string()),
        transport: TransportId::Wasm,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    }
}

/// Public `wasm:` classification — the blessed `Some(Public)` pass-through
/// scenario (plan-only context, no providers).
fn public_plan() -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Public,
        provider_ref: None,
        transport: TransportId::Wasm,
        credential_sources: Vec::new(),
        audience_binding: None,
    }
}

// ─── Task 2.3: credential-source matrix fixtures ────────────────────────────

/// Registry with two static-token providers (`idp-a`/`t-a` → `svc-a`,
/// `idp-b`/`t-b` → `svc-b`). Same fixture as the in-crate unit tests in
/// `source_host.rs` (Task 2.1), replicated here because that module is
/// private to the lib's `#[cfg(test)]` build.
fn fixture_registry() -> ProviderRegistry {
    fn register_static(registry: &ProviderRegistry, id: &str, token: &str, subject: &str) {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new(token.to_string()),
            },
            principal: Principal {
                subject: subject.to_string(),
                issuer: "test".to_string(),
                audience: vec![],
                scopes: vec![],
                roles: vec![],
                claims: serde_json::Value::Null,
            },
        }])
        .expect("static credential store fixture");
        registry.register(
            id,
            ProviderEntry {
                authenticator: Arc::new(StaticTokenAuthenticator::new(store)),
                audience_binding: None,
            },
        );
    }

    let registry = ProviderRegistry::new();
    register_static(&registry, "idp-a", "t-a", "svc-a");
    register_static(&registry, "idp-b", "t-b", "svc-b");
    registry
}

/// Authenticated `wasm:` plan bound to `provider_ref` permitting exactly
/// one credential source — the Task 2.3 matrix varies the source per test.
fn auth_plan(provider_ref: &str, source: CredentialSource) -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(provider_ref.to_string()),
        transport: TransportId::Wasm,
        credential_sources: vec![source],
        audience_binding: None,
    }
}

/// Create a WasmSourceConsumer bound to a free port with the webhook guest
/// (the listener behavior under test is guest-agnostic for denials).
fn make_consumer(port: u16) -> WasmSourceConsumer {
    let bind = format!("127.0.0.1:{port}");
    let uri = format!("wasm:webhook.wasm?bind={bind}&path=/webhook");
    let guest_config = vec![("bind".into(), bind), ("path".into(), "/webhook".into())];
    WasmSourceConsumer::new(
        require_webhook_guest_wasm(),
        uri,
        WasmConfig {
            timeout_secs: 5,
            ..WasmConfig::default()
        },
        guest_config,
        Arc::new(camel_component_api::NoOpComponentContext),
    )
}

/// Create a ConsumerContext backed by a bounded channel.
/// Returns (context, receiver, cancel_token).
fn make_consumer_context(
    route_id: &str,
    capacity: usize,
) -> (
    ConsumerContext,
    mpsc::Receiver<ExchangeEnvelope>,
    CancellationToken,
) {
    let (tx, rx) = mpsc::channel(capacity);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), route_id.to_string());
    (ctx, rx, cancel)
}

/// Allocate a unique port by binding to port 0 and reading the assigned port.
async fn free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Wait until the given TCP port accepts connections, or panic after timeout.
async fn wait_for_bind(port: u16, timeout: Duration) {
    let start = std::time::Instant::now();
    loop {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        if start.elapsed() > timeout {
            panic!("port {port} did not bind within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Send a raw HTTP POST with arbitrary extra headers and return the
/// response bytes as a string. Raw TCP keeps the test dependency-free,
/// mirroring `tests/source_integration.rs`.
async fn send_http_post_with_headers(
    port: u16,
    path: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("failed to connect to source HTTP listener");

    let header_lines: String = extra_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         {header_lines}\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n",
        body.len()
    );

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(request.as_bytes())
        .await
        .expect("failed to write request headers");
    stream
        .write_all(body)
        .await
        .expect("failed to write request body");

    let mut buf = vec![0u8; 1024];
    let n = stream
        .read(&mut buf)
        .await
        .expect("failed to read response");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

/// Send a raw HTTP POST and return the response bytes as a string.
///
/// `authorization` optionally sets the `Authorization` header (e.g.
/// `"Bearer <token>"`).
async fn send_http_post(port: u16, path: &str, body: &[u8], authorization: Option<&str>) -> String {
    let extra_headers: Vec<(&str, &str)> = authorization
        .map(|value| vec![("Authorization", value)])
        .unwrap_or_default();
    send_http_post_with_headers(port, path, body, &extra_headers).await
}

/// Stop the consumer within the grace timeout.
async fn stop_consumer(consumer: WasmSourceConsumer) {
    let mut consumer = consumer;
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop timed out")
        .expect("stop should succeed");
}

/// Await the exchange the guest forwarded and assert the kernel-minted
/// carrier is installed on it — the same `read_carrier` assertion the
/// http/grpc transport tests use (camel-auth kernel.rs).
async fn expect_exchange_with_carrier(rx: &mut mpsc::Receiver<ExchangeEnvelope>) {
    let envelope = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for exchange")
        .expect("channel closed before exchange arrived");
    assert!(
        read_carrier(&envelope.exchange).is_some(),
        "exchange from an authenticated request must carry the kernel carrier"
    );
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// Authenticated route (kernel wired: plan + providers) + request without
/// any credential → 401, and the guest never woke: no exchange reached the
/// pipeline channel. The 401 (not 202) is itself the proof the request
/// channel was never touched — the handler renders 202 the moment
/// `tx.send` succeeds, so a denial response means the request never entered
/// the channel (item count 0).
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn authenticated_route_missing_credential_401() {
    let port = free_port().await;
    let mut consumer = make_consumer(port);
    // Same fixture shape as tests/source_auth.rs: policy-backed context with
    // the compiled plan plus a (provider-less) registry.
    consumer.set_security_context(
        SecurityContext::new(RolePolicy::new(vec![], true))
            .with_plan(authenticated_plan())
            .with_providers(Arc::new(ProviderRegistry::new())),
    );

    let (ctx, mut rx, _cancel) = make_consumer_context("missing-credential", 16);
    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    let response = send_http_post(port, "/webhook", b"{\"event\":\"test\"}", None).await;
    assert!(
        response.contains("401"),
        "expected 401 for a missing credential, got: {response}"
    );

    // Guest never woke: the pipeline channel stays empty.
    tokio::time::sleep(SETTLE).await;
    assert!(
        rx.try_recv().is_err(),
        "no exchange may reach the pipeline after a denial"
    );

    stop_consumer(consumer).await;
}

/// Authenticated route (kernel wired) + malformed `Authorization: Bearer`
/// token → 401 (mint failure), guest never woke.
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn authenticated_route_invalid_credential_401() {
    let port = free_port().await;
    let mut consumer = make_consumer(port);
    consumer.set_security_context(
        SecurityContext::new(RolePolicy::new(vec![], true))
            .with_plan(auth_plan("idp-a", CredentialSource::AuthorizationHeader))
            .with_providers(Arc::new(fixture_registry())),
    );

    let (ctx, mut rx, _cancel) = make_consumer_context("invalid-credential", 16);
    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    let response = send_http_post(
        port,
        "/webhook",
        b"{\"event\":\"test\"}",
        Some("Bearer garbage-token"),
    )
    .await;
    assert!(
        response.contains("401"),
        "expected 401 for an invalid credential, got: {response}"
    );

    // Guest never woke: the pipeline channel stays empty.
    tokio::time::sleep(SETTLE).await;
    assert!(
        rx.try_recv().is_err(),
        "no exchange may reach the pipeline after a denial"
    );

    stop_consumer(consumer).await;
}

/// Blessed `Some(Public)` scenario — a plan-only context classifying the
/// route Public → plain request passes through untouched: 202 immediate
/// ack and the guest processes it (mirrors tests/source_integration.rs
/// assertions).
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn public_plan_pass_through_unchanged() {
    let port = free_port().await;
    let mut consumer = make_consumer(port);
    consumer.set_security_context(SecurityContext::from_plan(public_plan()));

    let (ctx, mut rx, _cancel) = make_consumer_context("public-pass-through", 16);
    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    let response = send_http_post(port, "/webhook", b"{\"event\":\"test\"}", None).await;
    assert!(
        response.contains("202"),
        "expected 202 immediate ack for a Public plan, got: {response}"
    );

    // The guest processes it: an exchange arrives with the POST payload and
    // HTTP metadata (same assertions as tests/source_integration.rs).
    let envelope = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for exchange")
        .expect("channel closed before exchange arrived");
    let body_contains_event = match &envelope.exchange.input.body {
        camel_api::Body::Text(s) => s.contains("event"),
        camel_api::Body::Bytes(b) => String::from_utf8_lossy(b).contains("event"),
        _ => false,
    };
    assert!(
        body_contains_event,
        "exchange body should contain the POST payload, got: {:?}",
        envelope.exchange.input.body
    );
    assert!(
        envelope
            .exchange
            .properties
            .contains_key("camel.http.method"),
        "exchange should have camel.http.method property"
    );

    stop_consumer(consumer).await;
}

/// Plan-only context (non-Public plan, no providers → kernel absent but
/// classification captured) → request is denied 401: absent wiring never
/// yields pass-through for non-Public plans (fail-closed).
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn missing_wiring_non_public_denies() {
    let port = free_port().await;
    let mut consumer = make_consumer(port);
    consumer.set_security_context(SecurityContext::from_plan(authenticated_plan()));

    let (ctx, mut rx, _cancel) = make_consumer_context("missing-wiring", 16);
    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    let response = send_http_post(port, "/webhook", b"{\"event\":\"test\"}", None).await;
    assert!(
        response.contains("401"),
        "expected 401 for a non-Public route without wiring, got: {response}"
    );

    // Guest never woke: the pipeline channel stays empty.
    tokio::time::sleep(SETTLE).await;
    assert!(
        rx.try_recv().is_err(),
        "no exchange may reach the pipeline after a denial"
    );

    stop_consumer(consumer).await;
}

// ─── Task 2.3: credential-source matrix (success cases) ─────────────────────
//
// Each test declares exactly one permitted credential source, sends the
// valid `idp-a` fixture token (`t-a`) in that source, and asserts both the
// 202 immediate ack and the downstream Exchange carrying the kernel-minted
// carrier.

/// Source `AuthorizationHeader`: `Authorization: Bearer t-a` → 202 and
/// the pipeline Exchange carries the carrier.
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn auth_via_authorization_header() {
    let port = free_port().await;
    let mut consumer = make_consumer(port);
    consumer.set_security_context(
        SecurityContext::new(RolePolicy::new(vec![], true))
            .with_plan(auth_plan("idp-a", CredentialSource::AuthorizationHeader))
            .with_providers(Arc::new(fixture_registry())),
    );

    let (ctx, mut rx, _cancel) = make_consumer_context("auth-authorization-header", 16);
    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    let response = send_http_post(
        port,
        "/webhook",
        b"{\"event\":\"test\"}",
        Some("Bearer t-a"),
    )
    .await;
    assert!(
        response.contains("202"),
        "expected 202 for a valid AuthorizationHeader token, got: {response}"
    );

    expect_exchange_with_carrier(&mut rx).await;
    stop_consumer(consumer).await;
}

/// Source `Header` (plan's named header): `x-api-key: t-a` → 202 and the
/// pipeline Exchange carries the carrier.
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn auth_via_named_header() {
    let port = free_port().await;
    let mut consumer = make_consumer(port);
    consumer.set_security_context(
        SecurityContext::new(RolePolicy::new(vec![], true))
            .with_plan(auth_plan(
                "idp-a",
                CredentialSource::Header {
                    name: "x-api-key".to_string(),
                },
            ))
            .with_providers(Arc::new(fixture_registry())),
    );

    let (ctx, mut rx, _cancel) = make_consumer_context("auth-named-header", 16);
    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    let response = send_http_post_with_headers(
        port,
        "/webhook",
        b"{\"event\":\"test\"}",
        &[("x-api-key", "t-a")],
    )
    .await;
    assert!(
        response.contains("202"),
        "expected 202 for a valid named-header token, got: {response}"
    );

    expect_exchange_with_carrier(&mut rx).await;
    stop_consumer(consumer).await;
}

/// Source `Cookie` (plan's named cookie): `Cookie: session=t-a` → 202 and
/// the pipeline Exchange carries the carrier.
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn auth_via_cookie() {
    let port = free_port().await;
    let mut consumer = make_consumer(port);
    consumer.set_security_context(
        SecurityContext::new(RolePolicy::new(vec![], true))
            .with_plan(auth_plan(
                "idp-a",
                CredentialSource::Cookie {
                    name: "session".to_string(),
                },
            ))
            .with_providers(Arc::new(fixture_registry())),
    );

    let (ctx, mut rx, _cancel) = make_consumer_context("auth-cookie", 16);
    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    let response = send_http_post_with_headers(
        port,
        "/webhook",
        b"{\"event\":\"test\"}",
        &[("Cookie", "session=t-a")],
    )
    .await;
    assert!(
        response.contains("202"),
        "expected 202 for a valid cookie token, got: {response}"
    );

    expect_exchange_with_carrier(&mut rx).await;
    stop_consumer(consumer).await;
}

/// Source `QueryParam`: `POST /webhook?token=t-a` → 202 and the pipeline
/// Exchange carries the carrier.
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn auth_via_query_param() {
    let port = free_port().await;
    let mut consumer = make_consumer(port);
    consumer.set_security_context(
        SecurityContext::new(RolePolicy::new(vec![], true))
            .with_plan(auth_plan(
                "idp-a",
                CredentialSource::QueryParam {
                    param: "token".to_string(),
                },
            ))
            .with_providers(Arc::new(fixture_registry())),
    );

    let (ctx, mut rx, _cancel) = make_consumer_context("auth-query-param", 16);
    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    let response = send_http_post(port, "/webhook?token=t-a", b"{\"event\":\"test\"}", None).await;
    assert!(
        response.contains("202"),
        "expected 202 for a valid query-param token, got: {response}"
    );

    expect_exchange_with_carrier(&mut rx).await;
    stop_consumer(consumer).await;
}

// ─── Task 2.3: provider substitution (denial case) ───────────────────────────

/// Plan bound to fixture provider `idp-b`; request carries `t-a`, valid
/// material minted by `idp-a`. A token from a different provider never
/// authenticates → 401 and the guest never woke.
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn provider_substitution_denied() {
    let port = free_port().await;
    let mut consumer = make_consumer(port);
    consumer.set_security_context(
        SecurityContext::new(RolePolicy::new(vec![], true))
            .with_plan(auth_plan("idp-b", CredentialSource::AuthorizationHeader))
            .with_providers(Arc::new(fixture_registry())),
    );

    let (ctx, mut rx, _cancel) = make_consumer_context("provider-substitution", 16);
    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // `t-a` is genuine idp-a material; the route's provider is idp-b.
    let response = send_http_post(
        port,
        "/webhook",
        b"{\"event\":\"test\"}",
        Some("Bearer t-a"),
    )
    .await;
    assert!(
        response.contains("401"),
        "expected 401 for a token minted by a different provider, got: {response}"
    );

    // Guest never woke: the pipeline channel stays empty.
    tokio::time::sleep(SETTLE).await;
    assert!(
        rx.try_recv().is_err(),
        "no exchange may reach the pipeline after a substitution denial"
    );

    stop_consumer(consumer).await;
}
