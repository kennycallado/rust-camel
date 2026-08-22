use std::{fs, path::PathBuf, sync::Arc};

#[test]
fn guide_yaml_route_uses_the_real_parser() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/config-basic/routes/hello.yaml");
    let yaml = fs::read_to_string(path).expect("guide YAML example must be readable");

    let routes = camel_dsl::parse_yaml(&yaml).expect("guide YAML example must parse");

    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "hello-timer");
}

/// Doc gate for the anchored rest-block `security_policy` example
/// (`unify-transport-auth`, Task 2.10): the YAML included from
/// `docs/src/yaml-dsl/route-structure.md` must lower and compile through
/// the real parser, with every lowered route carrying the block policy
/// and the declared provider.
#[test]
fn rest_security_example_lowers_and_compiles() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/rest-crud/routes/secured.yaml");
    let yaml = fs::read_to_string(path).expect("rest security example must be readable");

    let ctx = camel_dsl::SecurityCompileContext::default()
        .with_named_authenticator("native-demo", stub_authenticator());
    let defs = camel_dsl::parse_yaml_with_threshold_and_security(&yaml, 1024, ctx)
        .expect("rest security example must lower + compile");

    assert_eq!(defs.len(), 4, "all four CRUD operations must lower");
    for def in &defs {
        assert!(
            def.security_policy_config().is_some(),
            "route '{}' must carry the block policy",
            def.route_id()
        );
        assert_eq!(
            def.security_provider(),
            Some("native-demo"),
            "route '{}' must carry the declared provider",
            def.route_id()
        );
    }
}

fn stub_authenticator() -> Arc<dyn camel_auth::TokenAuthenticator> {
    Arc::new(StubAuth)
}

struct StubAuth;

#[async_trait::async_trait]
impl camel_auth::TokenAuthenticator for StubAuth {
    async fn authenticate_bearer(
        &self,
        _token: &str,
    ) -> Result<camel_api::security_policy::Principal, camel_api::CamelError> {
        Ok(camel_api::security_policy::Principal {
            subject: "doc-example".into(),
            issuer: "test".into(),
            audience: vec![],
            scopes: vec![],
            roles: vec!["user".into()],
            claims: serde_json::Value::Null,
        })
    }
}
