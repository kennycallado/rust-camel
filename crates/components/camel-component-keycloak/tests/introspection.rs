// Real Keycloak JWKS validation is covered in camel-test's integration-tests suite.
// These tests stay wiremock-based to verify Keycloak-shaped introspection responses and error mapping deterministically.

use camel_api::CamelError;
use camel_auth::claims::{ClaimsMapper, JsonPointerClaimsMapper};
use camel_auth::introspection::{CachingTokenIntrospector, IntrospectionCacheOptions};
use camel_auth::{IntrospectionAuthenticator, TokenAuthenticator};
use camel_component_keycloak::keycloak_claim_paths;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn build_auth(
    server: &MockServer,
    client_id: &str,
    options: IntrospectionCacheOptions,
) -> IntrospectionAuthenticator {
    let introspector = CachingTokenIntrospector::new_unchecked_for_test(
        server.uri(),
        client_id.into(),
        "test-secret".into(),
        options,
    );
    let mapper: Arc<dyn ClaimsMapper> = Arc::new(JsonPointerClaimsMapper::new(
        keycloak_claim_paths(client_id),
    ));
    IntrospectionAuthenticator::new(Arc::new(introspector), mapper)
}

#[tokio::test]
async fn introspection_active_opaque_token_returns_principal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "active": true,
            "sub": "user-42",
            "scope": "read write",
            "realm_access": {"roles": ["admin"]},
            "resource_access": {"svc": {"roles": ["svc-role"]}}
        })))
        .mount(&server)
        .await;

    let auth = build_auth(&server, "svc", IntrospectionCacheOptions::default());

    let principal = auth.authenticate_bearer("opaque-token-123").await.unwrap();
    assert_eq!(principal.subject, "user-42");
    assert!(principal.has_role("admin"));
    assert!(principal.has_role("svc-role"));
    assert_eq!(principal.scopes, vec!["read", "write"]);
}

#[tokio::test]
async fn introspection_inactive_token_returns_unauthenticated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"active": false})))
        .mount(&server)
        .await;

    let auth = build_auth(&server, "svc", IntrospectionCacheOptions::default());

    let err = auth.authenticate_bearer("revoked-token").await.unwrap_err();
    match err {
        CamelError::Unauthenticated(msg) => assert!(msg.contains("not active")),
        other => panic!("expected Unauthenticated, got: {other:?}"),
    }
}

#[tokio::test]
async fn introspection_provider_401_maps_to_provider_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let auth = build_auth(&server, "svc", IntrospectionCacheOptions::default());

    let err = auth.authenticate_bearer("any-token").await.unwrap_err();
    match err {
        CamelError::ProcessorError(msg) => {
            assert!(msg.contains("auth provider unavailable"));
        }
        other => panic!("expected ProcessorError, got: {other:?}"),
    }
}

#[tokio::test]
async fn introspection_provider_500_maps_to_provider_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let auth = build_auth(&server, "svc", IntrospectionCacheOptions::default());

    let err = auth.authenticate_bearer("any-token").await.unwrap_err();
    match err {
        CamelError::ProcessorError(msg) => {
            assert!(msg.contains("auth provider unavailable"));
        }
        other => panic!("expected ProcessorError, got: {other:?}"),
    }
}

#[tokio::test]
async fn introspection_caches_result_one_http_call_for_two_auths() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "active": true,
            "sub": "cached-user"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let auth = build_auth(
        &server,
        "svc",
        IntrospectionCacheOptions {
            max_entries: 100,
            default_ttl: Duration::from_secs(60),
            negative_ttl: Duration::from_secs(5),
        },
    );

    let p1 = auth.authenticate_bearer("same-opaque-token").await.unwrap();
    let p2 = auth.authenticate_bearer("same-opaque-token").await.unwrap();
    assert_eq!(p1.subject, "cached-user");
    assert_eq!(p2.subject, "cached-user");
}

#[tokio::test]
async fn introspection_keycloak_realm_access_via_json_pointers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "active": true,
            "sub": "alice",
            "realm_access": {"roles": ["realm-admin"]},
            "resource_access": {"my-app": {"roles": ["app-user"]}},
            "scope": "profile email"
        })))
        .mount(&server)
        .await;

    let auth = build_auth(&server, "my-app", IntrospectionCacheOptions::default());

    let principal = auth.authenticate_bearer("tok").await.unwrap();
    assert_eq!(principal.subject, "alice");
    assert!(principal.has_role("realm-admin"));
    assert!(principal.has_role("app-user"));
    assert_eq!(principal.scopes, vec!["profile", "email"]);
}
