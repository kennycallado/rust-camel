use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use camel_api::security_policy::{
    AuthorizationDecision, Principal, SecurityPolicy, principal_from_exchange,
    store_principal_properties,
};
use camel_api::{Body, Exchange, Message};
use camel_component_wasm::WasmConfig;
use camel_component_wasm::WasmSecurityPolicy;
use camel_core::Registry;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("security-policy-test.wasm")
}

fn make_registry() -> Arc<std::sync::Mutex<Registry>> {
    Arc::new(std::sync::Mutex::new(Registry::new()))
}

#[tokio::test]
async fn wasm_security_policy_grants_when_admin_role_present() {
    let path = fixture_path();
    let registry = make_registry();
    let policy = WasmSecurityPolicy::new(&path, WasmConfig::default(), registry, HashMap::new())
        .await
        .expect("failed to create WasmSecurityPolicy");

    let mut exchange = Exchange::new(Message::new(Body::Empty));
    exchange.set_property(
        "camel.auth.roles",
        serde_json::to_string(&vec!["admin"]).unwrap(),
    );
    let principal = Principal {
        subject: "user-1".to_string(),
        issuer: "test".to_string(),
        audience: vec![],
        scopes: vec![],
        roles: vec!["admin".to_string()],
        claims: serde_json::Value::Null,
    };
    store_principal_properties(&mut exchange, &principal);

    let decision = policy.evaluate(&mut exchange).await;
    assert!(
        matches!(decision, Ok(AuthorizationDecision::Granted { .. })),
        "expected Granted, got: {decision:?}"
    );
}

#[tokio::test]
async fn wasm_security_policy_denies_when_admin_role_missing() {
    let path = fixture_path();
    let registry = make_registry();
    let policy = WasmSecurityPolicy::new(&path, WasmConfig::default(), registry, HashMap::new())
        .await
        .expect("failed to create WasmSecurityPolicy");

    let mut exchange = Exchange::new(Message::new(Body::Empty));
    exchange.set_property(
        "camel.auth.roles",
        serde_json::to_string(&vec!["viewer"]).unwrap(),
    );
    let principal = Principal {
        subject: "user-2".to_string(),
        issuer: "test".to_string(),
        audience: vec![],
        scopes: vec![],
        roles: vec!["viewer".to_string()],
        claims: serde_json::Value::Null,
    };
    store_principal_properties(&mut exchange, &principal);

    let decision = policy.evaluate(&mut exchange).await;
    assert!(
        matches!(decision, Ok(AuthorizationDecision::Denied { .. })),
        "expected Denied, got: {decision:?}"
    );
}

#[tokio::test]
async fn wasm_security_policy_granted_preserves_principal() {
    let path = fixture_path();
    let registry = make_registry();
    let policy = WasmSecurityPolicy::new(&path, WasmConfig::default(), registry, HashMap::new())
        .await
        .expect("failed to create WasmSecurityPolicy");

    let mut exchange = Exchange::new(Message::new(Body::Empty));
    exchange.set_property(
        "camel.auth.roles",
        serde_json::to_string(&vec!["admin"]).unwrap(),
    );
    let principal = Principal {
        subject: "admin-user".to_string(),
        issuer: "keycloak".to_string(),
        audience: vec!["my-app".to_string()],
        scopes: vec!["read".to_string(), "write".to_string()],
        roles: vec!["admin".to_string()],
        claims: serde_json::json!({"department": "engineering"}),
    };
    store_principal_properties(&mut exchange, &principal);

    let decision = policy.evaluate(&mut exchange).await;
    match decision {
        Ok(AuthorizationDecision::Granted { principal: p }) => {
            assert_eq!(p.subject, "admin-user");
            assert_eq!(p.issuer, "keycloak");
            let recovered = principal_from_exchange(&exchange)
                .expect("principal should be recoverable from exchange");
            assert_eq!(recovered.subject, "admin-user");
            assert_eq!(recovered.issuer, "keycloak");
        }
        other => panic!("expected Granted, got: {other:?}"),
    }
}
