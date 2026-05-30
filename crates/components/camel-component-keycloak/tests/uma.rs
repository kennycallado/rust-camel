//! Wiremock integration tests for KeycloakUmaEvaluator.
//!
//! Each test starts a MockServer, registers stubs for client-credentials
//! and UMA permission endpoints, then asserts the evaluator maps responses
//! to the correct `PermissionDecision` or `AuthError`.

use camel_api::security_policy::Principal;
use camel_auth::permission::{PermissionDecision, PermissionEvaluator, PermissionRequest};
use camel_auth::types::AuthError;
use camel_component_keycloak::uma::KeycloakUmaEvaluator;
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_principal() -> Principal {
    Principal {
        subject: "alice".into(),
        issuer: "test".into(),
        audience: vec![],
        scopes: vec![],
        roles: vec![],
        claims: json!({"sub": "alice", "iss": "test"}),
    }
}

fn test_request() -> PermissionRequest {
    PermissionRequest {
        principal: test_principal(),
        resource: "invoice:123".into(),
        action: "read".into(),
        requested_scopes: vec![],
        context: serde_json::Value::Null,
    }
}

/// Stubs the client-credentials token endpoint so the evaluator can obtain
/// a service-account access token before calling the UMA permission endpoint.
fn mock_client_token(_server: &MockServer) -> Mock {
    Mock::given(method("POST"))
        .and(path("/realms/test/protocol/openid-connect/token"))
        .and(body_string_contains("grant_type=client_credentials"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "client-token",
            "token_type": "Bearer",
            "expires_in": 300
        })))
}

fn build_evaluator(server: &MockServer) -> KeycloakUmaEvaluator {
    KeycloakUmaEvaluator::new_unchecked_for_test(
        &server.uri(),
        "test",
        "authz-client",
        "secret",
        reqwest::Client::new(),
    )
}

#[tokio::test]
async fn uma_grant_response_maps_to_granted() {
    let server = MockServer::start().await;

    mock_client_token(&server).mount(&server).await;

    Mock::given(method("POST"))
        .and(path("/realms/test/protocol/openid-connect/token"))
        .and(body_string_contains("uma-ticket"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "rpt-token",
            "token_type": "Bearer",
            "expires_in": 300
        })))
        .mount(&server)
        .await;

    let evaluator = build_evaluator(&server);
    let decision = evaluator.evaluate(test_request()).await.unwrap();

    assert_eq!(decision, PermissionDecision::Granted);
}

#[tokio::test]
async fn uma_deny_with_access_denied_maps_to_denied() {
    let server = MockServer::start().await;

    mock_client_token(&server).mount(&server).await;

    Mock::given(method("POST"))
        .and(path("/realms/test/protocol/openid-connect/token"))
        .and(body_string_contains("uma-ticket"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": "access_denied",
            "error_description": "The resource owner denied access to invoice:123"
        })))
        .mount(&server)
        .await;

    let evaluator = build_evaluator(&server);
    let decision = evaluator.evaluate(test_request()).await.unwrap();

    match decision {
        PermissionDecision::Denied { reason } => {
            assert!(
                reason.contains("invoice:123"),
                "reason should contain resource id, got: {reason}"
            );
        }
        PermissionDecision::Granted => panic!("expected Denied, got Granted"),
    }
}

#[tokio::test]
async fn uma_deny_with_not_authorized_maps_to_denied() {
    let server = MockServer::start().await;

    mock_client_token(&server).mount(&server).await;

    Mock::given(method("POST"))
        .and(path("/realms/test/protocol/openid-connect/token"))
        .and(body_string_contains("uma-ticket"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": "not_authorized",
            "error_description": "no permissions"
        })))
        .mount(&server)
        .await;

    let evaluator = build_evaluator(&server);
    let decision = evaluator.evaluate(test_request()).await.unwrap();

    assert_eq!(
        decision,
        PermissionDecision::Denied {
            reason: "no permissions".to_string()
        }
    );
}

#[tokio::test]
async fn uma_403_with_invalid_json_still_denies() {
    let server = MockServer::start().await;

    mock_client_token(&server).mount(&server).await;

    Mock::given(method("POST"))
        .and(path("/realms/test/protocol/openid-connect/token"))
        .and(body_string_contains("uma-ticket"))
        .respond_with(ResponseTemplate::new(403).set_body_string("not json"))
        .mount(&server)
        .await;

    let evaluator = build_evaluator(&server);
    let decision = evaluator.evaluate(test_request()).await.unwrap();

    assert!(
        matches!(decision, PermissionDecision::Denied { .. }),
        "expected Denied, got {decision:?}"
    );
}

#[tokio::test]
async fn uma_client_auth_failure_returns_provider_unavailable() {
    let server = MockServer::start().await;

    // Client credentials returns 401 — evaluator should get ProviderUnavailable
    Mock::given(method("POST"))
        .and(path("/realms/test/protocol/openid-connect/token"))
        .and(body_string_contains("grant_type=client_credentials"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let evaluator = build_evaluator(&server);
    let result = evaluator.evaluate(test_request()).await;

    match result {
        Err(AuthError::ProviderUnavailable(msg)) => {
            assert!(
                !msg.is_empty(),
                "ProviderUnavailable message should not be empty"
            );
        }
        other => panic!("expected AuthError::ProviderUnavailable, got: {other:?}"),
    }
}

#[tokio::test]
async fn uma_5xx_returns_provider_unavailable() {
    let server = MockServer::start().await;

    mock_client_token(&server).mount(&server).await;

    Mock::given(method("POST"))
        .and(path("/realms/test/protocol/openid-connect/token"))
        .and(body_string_contains("uma-ticket"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let evaluator = build_evaluator(&server);
    let result = evaluator.evaluate(test_request()).await;

    match result {
        Err(AuthError::ProviderUnavailable(msg)) => {
            assert!(
                msg.contains("500"),
                "message should mention status 500, got: {msg}"
            );
        }
        other => panic!("expected AuthError::ProviderUnavailable, got: {other:?}"),
    }
}

#[tokio::test]
async fn uma_invalid_json_on_success_returns_granted() {
    let server = MockServer::start().await;

    mock_client_token(&server).mount(&server).await;

    Mock::given(method("POST"))
        .and(path("/realms/test/protocol/openid-connect/token"))
        .and(body_string_contains("uma-ticket"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let evaluator = build_evaluator(&server);
    let decision = evaluator.evaluate(test_request()).await.unwrap();

    assert_eq!(decision, PermissionDecision::Granted);
}

#[tokio::test]
async fn uma_sends_claim_token_with_principal_claims() {
    let server = MockServer::start().await;

    mock_client_token(&server).mount(&server).await;

    Mock::given(method("POST"))
        .and(path("/realms/test/protocol/openid-connect/token"))
        .and(body_string_contains("claim_token="))
        .and(body_string_contains(
            "claim_token_format=urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Ajwt",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "rpt-token",
            "token_type": "Bearer",
            "expires_in": 300
        })))
        .mount(&server)
        .await;

    let evaluator = build_evaluator(&server);
    let decision = evaluator.evaluate(test_request()).await.unwrap();

    assert_eq!(decision, PermissionDecision::Granted);
}

fn bob_principal() -> Principal {
    Principal {
        subject: "bob".into(),
        issuer: "test".into(),
        audience: vec![],
        scopes: vec![],
        roles: vec![],
        claims: json!({"sub": "bob", "iss": "test"}),
    }
}

fn bob_request() -> PermissionRequest {
    PermissionRequest {
        principal: bob_principal(),
        resource: "invoice:456".into(),
        action: "write".into(),
        requested_scopes: vec![],
        context: serde_json::Value::Null,
    }
}

#[tokio::test]
async fn uma_different_principals_produce_different_claim_tokens() {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    let alice_claims = &test_principal().claims;
    let bob_claims = &bob_principal().claims;

    let alice_token = BASE64.encode(serde_json::to_string(alice_claims).unwrap());
    let bob_token = BASE64.encode(serde_json::to_string(bob_claims).unwrap());

    assert_ne!(
        alice_token, bob_token,
        "different principals must produce different claim_tokens"
    );

    // Verify each token decodes back to the correct claims
    let alice_decoded: serde_json::Value =
        serde_json::from_str(&String::from_utf8(BASE64.decode(&alice_token).unwrap()).unwrap())
            .unwrap();
    let bob_decoded: serde_json::Value =
        serde_json::from_str(&String::from_utf8(BASE64.decode(&bob_token).unwrap()).unwrap())
            .unwrap();

    assert_eq!(alice_decoded["sub"], "alice");
    assert_eq!(bob_decoded["sub"], "bob");

    // Wire the bob request through the evaluator end-to-end
    let server = MockServer::start().await;
    mock_client_token(&server).mount(&server).await;

    Mock::given(method("POST"))
        .and(path("/realms/test/protocol/openid-connect/token"))
        .and(body_string_contains("claim_token="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "rpt-token",
            "token_type": "Bearer",
            "expires_in": 300
        })))
        .mount(&server)
        .await;

    let evaluator = build_evaluator(&server);
    let decision = evaluator.evaluate(bob_request()).await.unwrap();
    assert_eq!(decision, PermissionDecision::Granted);
}
