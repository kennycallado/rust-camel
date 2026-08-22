//! Kernel fail-closed E2E (`unify-transport-auth`, Task 1.11).
//!
//! Cross-plane denial proof over a real HTTP server: a route secured by a
//! roles policy resolved to the fixture provider denies requests without
//! credentials (401, body never runs) and grants valid fixture tokens
//! (2xx, body runs, mock receives the exchange). Since Task 2.9 the grant
//! path is boundary authentication: the HTTP consumer extracts per the
//! plan's sources, mints the typed carrier through `kernel_authenticate`,
//! and the strict-mode dispatch check requires that carrier before the
//! pipeline runs.
//!
//! Requires `integration-tests` feature to compile and run.

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;

use std::sync::Arc;
use std::time::Duration;

use camel_api::AuthPrincipal;
use camel_api::Value;
use camel_api::security_policy::{CredentialSource, SecurityPolicyConfig};
use camel_auth::{RolePolicy, TokenAuthenticator, read_carrier};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::HttpComponent;
use camel_test::{CamelTestContext, SecurityConfigFixture};

fn find_free_port() -> u16 {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind to free port");
    listener.local_addr().unwrap().port()
}

/// Secure HTTP route: roles policy `["test-role"]` over the fixture
/// provider `idp-e2e` (token `test-token-idp-e2e`), body → `mock:result`.
async fn build_secured_route() -> (CamelTestContext, u16) {
    install_crypto_provider();
    let port = find_free_port();

    let fixture = SecurityConfigFixture::single_static_provider("idp-e2e");
    let provider_registry = Arc::new(fixture.providers());
    let entry = provider_registry
        .resolve("idp-e2e")
        .expect("fixture provider"); // allow-unwrap
    let authenticator: Arc<dyn TokenAuthenticator> = Arc::clone(&entry.authenticator);

    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let policy = RolePolicy::new(vec!["test-role".to_string()], true);
    let config = SecurityPolicyConfig::new(policy)
        .with_credential_sources(vec![CredentialSource::AuthorizationHeader]);

    let route = RouteBuilder::from(&format!("http://127.0.0.1:{port}/secured"))
        .route_id("kernel-fail-closed-http")
        .security_policy(config)
        .security_authenticator(authenticator)
        .provider_registry(provider_registry)
        .set_body(Value::String("ok".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to("mock:result".to_string())
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    (h, port)
}

#[tokio::test(flavor = "multi_thread")]
async fn secured_route_denies_missing_credentials_e2e() {
    let (h, port) = build_secured_route().await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/secured"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "request without Authorization must be unauthenticated"
    );
    let inbox = h.mock().get_endpoint("result").expect("mock endpoint"); // allow-unwrap
    assert_eq!(
        inbox.received_count().await,
        0,
        "denied request must not reach the body"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn secured_route_grants_valid_token_e2e() {
    let (h, port) = build_secured_route().await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/secured"))
        .header("Authorization", "Bearer test-token-idp-e2e")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "fixture token with test-role must be authorized"
    );
    let inbox = h.mock().get_endpoint("result").expect("mock endpoint"); // allow-unwrap
    assert_eq!(
        inbox.received_count().await,
        1,
        "granted request must reach the body"
    );
}

/// Task 2.9: the HTTP consumer authenticates at the request boundary —
/// denial is the component's 401 idiom and the body never runs.
#[tokio::test(flavor = "multi_thread")]
async fn http_kernel_denies_without_credentials() {
    let (h, port) = build_secured_route().await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/secured"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "Authenticated route without credentials must be denied at the boundary"
    );
    let inbox = h.mock().get_endpoint("result").expect("mock endpoint"); // allow-unwrap
    assert_eq!(
        inbox.received_count().await,
        0,
        "denied request must never reach the route body"
    );
}

/// Task 2.9: a valid token grants at the boundary and the kernel-minted
/// typed carrier rides EVERY Exchange — asserted on two sequential
/// requests (the second request proves the carrier is minted per request,
/// not cached from the first; e_opus second-dispatch pattern).
#[tokio::test(flavor = "multi_thread")]
async fn http_kernel_grants_with_token() {
    let (h, port) = build_secured_route().await;

    let client = reqwest::Client::new();
    for _ in 0..2 {
        let resp = client
            .get(format!("http://127.0.0.1:{port}/secured"))
            .header("Authorization", "Bearer test-token-idp-e2e")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "each granted request must succeed");
    }

    let inbox = h.mock().get_endpoint("result").expect("mock endpoint"); // allow-unwrap
    inbox.assert_exchange_count(2).await;
    let received = inbox.get_received_exchanges().await;
    assert_eq!(received.len(), 2);
    for (index, exchange) in received.iter().enumerate() {
        let carrier = read_carrier(exchange).unwrap_or_else(|| {
            panic!("request {index}: the granted Exchange must carry the kernel-minted carrier")
        });
        assert_eq!(
            carrier.provider_id(),
            "idp-e2e",
            "request {index}: the carrier must be minted by the route's provider"
        );
        assert_eq!(
            carrier.principal().subject,
            "test-user-idp-e2e",
            "request {index}: the carrier must hold the fixture principal"
        );
    }
}

/// Task 2.10: a `rest:` block whose `security_policy` names the fixture
/// provider secures EVERY lowered route. The block policy is copied onto
/// each lowered `http:` route at lowering time, so the kernel treats the
/// endpoint exactly like a hand-declared secured route: no token → 401
/// with an empty mock inbox; valid fixture token → 2xx with the exchange
/// delivered.
async fn build_secured_rest_block() -> (CamelTestContext, u16) {
    install_crypto_provider();
    let port = find_free_port();

    let fixture = SecurityConfigFixture::single_static_provider("idp-e2e");
    let provider_registry = fixture.providers();
    let entry = provider_registry
        .resolve("idp-e2e")
        .expect("fixture provider"); // allow-unwrap

    let yaml = format!(
        r#"
rest:
  - host: 127.0.0.1
    port: {port}
    path: /api/orders
    security_policy:
      roles: ["test-role"]
      provider: "idp-e2e"
    operations:
      - method: GET
        operation_id: listOrders
        steps:
          - set_body: '{{"orders":[]}}'
          - to: "mock:result"
"#
    );

    // Compile with the fixture provider registered under its name: the
    // rest-lowered routes then carry the plan inputs (policy + provider
    // "idp-e2e" + registry) exactly like an equivalent `http:` route.
    let security = camel_dsl::SecurityCompileContext::default()
        .with_named_authenticator("idp-e2e", std::sync::Arc::clone(&entry.authenticator));
    let definitions = camel_dsl::parse_yaml_with_threshold_and_security(
        &yaml,
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        security,
    )
    .expect("rest block with policy must lower + compile");

    assert_eq!(definitions.len(), 1, "one operation → one lowered route");

    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    for def in definitions {
        h.add_route(def).await.unwrap();
    }
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    (h, port)
}

#[tokio::test(flavor = "multi_thread")]
async fn rest_e2e_secured_endpoint_denies_and_grants() {
    let (h, port) = build_secured_rest_block().await;

    // Deny: no token → 401 at the boundary, body never runs.
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/orders"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "rest-lowered endpoint without Authorization must be unauthenticated"
    );
    let inbox = h.mock().get_endpoint("result").expect("mock endpoint"); // allow-unwrap
    assert_eq!(
        inbox.received_count().await,
        0,
        "denied request must not reach the rest operation body"
    );

    // Grant: valid fixture token → 2xx, body runs once.
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/orders"))
        .header("Authorization", "Bearer test-token-idp-e2e")
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "fixture token with test-role must be authorized on the rest endpoint, got {}",
        resp.status()
    );
    assert_eq!(
        inbox.received_count().await,
        1,
        "granted request must reach the rest operation body"
    );
}
