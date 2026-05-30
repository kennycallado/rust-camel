use async_trait::async_trait;
use camel_api::security_policy::{AuthorizationDecision, Principal, SecurityPolicy};
use camel_api::{CamelError, Exchange};
use camel_auth::{
    AuthError, PermissionDecision, PermissionEvaluator, PermissionEvaluatorRegistry,
    PermissionRequest, SecurityPolicyRegistry,
};
use camel_dsl::{
    SecurityCompileContext, compile_declarative_route_with_stream_cache_threshold,
    parse_yaml_to_declarative, parse_yaml_with_threshold_and_security,
};
use std::sync::Arc;

struct TestAuthenticator;

#[async_trait]
impl camel_auth::TokenAuthenticator for TestAuthenticator {
    async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
        Ok(Principal {
            subject: "test-user".into(),
            issuer: "test-issuer".into(),
            audience: vec![],
            scopes: vec!["read:api".into()],
            roles: vec!["admin".into()],
            claims: serde_json::Value::Null,
        })
    }
}

struct TestPolicy;

#[async_trait]
impl SecurityPolicy for TestPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
    ) -> Result<AuthorizationDecision, CamelError> {
        Ok(AuthorizationDecision::Granted {
            principal: Principal {
                subject: "test".into(),
                issuer: "test".into(),
                audience: vec![],
                scopes: vec![],
                roles: vec![],
                claims: serde_json::Value::Null,
            },
        })
    }
}

#[test]
fn e2e_yaml_roles_compile_to_route_definition() {
    let yaml = r#"
routes:
  - id: admin-api
    from: direct:start
    security_policy:
      roles: ["admin"]
    steps:
      - to: log:info
"#;
    let routes = parse_yaml_to_declarative(yaml).unwrap();
    assert_eq!(routes.len(), 1);
    assert!(routes[0].security_policy.is_some());

    let auth = Arc::new(TestAuthenticator) as Arc<dyn camel_auth::TokenAuthenticator>;
    let ctx = SecurityCompileContext::new(Some(auth), None);
    let def = compile_declarative_route_with_stream_cache_threshold(
        routes.into_iter().next().unwrap(),
        1024,
        ctx,
    )
    .unwrap();

    assert!(def.security_policy_config().is_some());
    assert!(def.security_authenticator().is_some());
}

#[test]
fn e2e_yaml_scopes_compile_to_route_definition() {
    let yaml = r#"
routes:
  - id: read-api
    from: direct:start
    security_policy:
      scopes: ["read:api"]
      all_required: false
    steps:
      - to: log:info
"#;
    let routes = parse_yaml_to_declarative(yaml).unwrap();
    let auth = Arc::new(TestAuthenticator) as Arc<dyn camel_auth::TokenAuthenticator>;
    let ctx = SecurityCompileContext::new(Some(auth), None);
    let def = compile_declarative_route_with_stream_cache_threshold(
        routes.into_iter().next().unwrap(),
        1024,
        ctx,
    )
    .unwrap();

    assert!(def.security_policy_config().is_some());
    assert!(def.security_authenticator().is_some());
}

#[test]
fn e2e_yaml_ref_compiles_from_registry() {
    let yaml = r#"
routes:
  - id: custom-api
    from: direct:start
    security_policy:
      ref: "my-custom-policy"
    steps:
      - to: log:info
"#;
    let routes = parse_yaml_to_declarative(yaml).unwrap();
    let registry = Arc::new(SecurityPolicyRegistry::new());
    registry.register("my-custom-policy", Arc::new(TestPolicy));
    let ctx = SecurityCompileContext::new(None, Some(registry));
    let def = compile_declarative_route_with_stream_cache_threshold(
        routes.into_iter().next().unwrap(),
        1024,
        ctx,
    )
    .unwrap();

    assert!(def.security_policy_config().is_some());
    assert!(def.security_authenticator().is_none());
}

#[test]
fn e2e_yaml_roles_without_authenticator_fails() {
    let yaml = r#"
routes:
  - id: admin-api
    from: direct:start
    security_policy:
      roles: ["admin"]
    steps:
      - to: log:info
"#;
    let routes = parse_yaml_to_declarative(yaml).unwrap();
    let ctx = SecurityCompileContext::default();
    let result = compile_declarative_route_with_stream_cache_threshold(
        routes.into_iter().next().unwrap(),
        1024,
        ctx,
    );
    assert!(result.is_err());
}

#[test]
fn e2e_yaml_wasm_not_yet_supported() {
    let yaml = r#"
routes:
  - id: wasm-api
    from: direct:start
    security_policy:
      wasm: "plugins/my-policy.wasm"
    steps:
      - to: log:info
"#;
    let routes = parse_yaml_to_declarative(yaml).unwrap();
    let ctx = SecurityCompileContext::default();
    let result = compile_declarative_route_with_stream_cache_threshold(
        routes.into_iter().next().unwrap(),
        1024,
        ctx,
    );
    assert!(result.is_err());
}

#[test]
fn e2e_parse_yaml_with_threshold_and_security() {
    let yaml = r#"
routes:
  - id: sec-route
    from: direct:start
    security_policy:
      roles: ["admin"]
    steps:
      - to: log:info
"#;
    let auth = Arc::new(TestAuthenticator) as Arc<dyn camel_auth::TokenAuthenticator>;
    let ctx = SecurityCompileContext::new(Some(auth), None);
    let defs = parse_yaml_with_threshold_and_security(yaml, 1024, ctx).unwrap();
    assert_eq!(defs.len(), 1);
    assert!(defs[0].security_policy_config().is_some());
    assert!(defs[0].security_authenticator().is_some());
}

struct TestPermissionEvaluator;

#[async_trait]
impl PermissionEvaluator for TestPermissionEvaluator {
    async fn evaluate(&self, _request: PermissionRequest) -> Result<PermissionDecision, AuthError> {
        Ok(PermissionDecision::Granted)
    }
}

#[test]
fn e2e_yaml_permission_compiles_with_evaluator_registry() {
    let yaml = r#"
routes:
  - id: permission-route
    from: direct:start
    security_policy:
      permission:
        policy: "test-eval"
    steps:
      - to: log:info
"#;

    let eval_registry = Arc::new(PermissionEvaluatorRegistry::new());
    eval_registry.register("test-eval", Arc::new(TestPermissionEvaluator));

    let ctx = SecurityCompileContext::new(None, None).with_evaluator_registry(eval_registry);

    let defs = parse_yaml_with_threshold_and_security(yaml, 1024, ctx).unwrap();
    assert_eq!(defs.len(), 1);
    assert!(defs[0].security_policy_config().is_some());
}
