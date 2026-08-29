use super::*;

#[test]
fn parse_security_config() {
    let toml_str = r#"
[security.keycloak]
server_url = "http://localhost:8080"
realm = "test-realm"
client_id = "my-client"
client_secret = "my-secret"

[security.keycloak.validation]
method = "local"
audience = ["my-api"]
clock_skew_secs = 30

[security.keycloak.jwks]
cache_ttl_secs = 3600
refresh_skew_secs = 60
"#;
    let config: CamelConfig = toml::from_str(toml_str).unwrap();
    let kc = config.security.keycloak.unwrap();
    assert_eq!(kc.server_url, "http://localhost:8080");
    assert_eq!(kc.realm, "test-realm");
    assert_eq!(kc.client_id, "my-client");
    assert_eq!(kc.validation.method, "local");
    assert_eq!(kc.validation.audience, vec!["my-api"]);
}

#[test]
fn security_config_defaults_when_absent() {
    let config: CamelConfig = toml::from_str("").unwrap();
    assert!(config.security.oidc.is_none());
    assert!(config.security.native.is_none());
    assert!(config.security.keycloak.is_none());
    assert!(config.security.permissions.is_none());
    assert!(config.security.policies.is_none());
}

#[test]
fn parse_security_oidc_and_native_config() {
    let toml_str = r#"
[security.oidc]
issuer = "https://issuer.example.com/realms/test"
jwks_uri = "https://issuer.example.com/realms/test/protocol/openid-connect/certs"
audience = ["api", "backend"]
client_id = "svc-client"
client_secret = "svc-secret"
token_endpoint = "https://issuer.example.com/realms/test/protocol/openid-connect/token"

[security.native]
subject = "native-user"
issuer = "native"
bearer_token = "token-123"
api_key = "key-123"
roles = ["admin"]
scopes = ["read", "write"]
"#;
    let config: CamelConfig = toml::from_str(toml_str).unwrap();

    let oidc = config.security.oidc.unwrap();
    assert_eq!(oidc.issuer, "https://issuer.example.com/realms/test");
    assert_eq!(oidc.audience, vec!["api", "backend"]);
    assert_eq!(oidc.client_id.as_deref(), Some("svc-client"));
    assert_eq!(oidc.client_secret.as_deref(), Some("svc-secret"));

    let native = config.security.native.unwrap();
    assert_eq!(native.subject, "native-user");
    assert_eq!(native.issuer.as_deref(), Some("native"));
    assert_eq!(native.roles, vec!["admin"]);
    assert_eq!(native.scopes, vec!["read", "write"]);
}

#[test]
fn native_auth_debug_redacts_secrets() {
    let native = NativeAuthConfig {
        subject: "native-user".into(),
        issuer: Some("native".into()),
        bearer_token: Some("super-secret-token".into()),
        api_key: Some("super-secret-key".into()),
        roles: vec!["admin".into()],
        scopes: vec!["read".into()],
        credentials: Vec::new(),
    };

    let debug = format!("{native:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("super-secret-token"));
    assert!(!debug.contains("super-secret-key"));
}

#[test]
fn keycloak_validation_defaults() {
    let defaults = KeycloakValidationConfig::default();
    assert_eq!(defaults.method, "local");
    assert!(defaults.audience.is_empty());
    assert_eq!(defaults.clock_skew_secs, 30);
}

#[test]
fn keycloak_jwks_defaults() {
    let defaults = KeycloakJwksConfig::default();
    assert_eq!(defaults.cache_ttl_secs, 3600);
    assert_eq!(defaults.refresh_skew_secs, 60);
}

#[test]
fn keycloak_introspection_defaults() {
    let defaults = KeycloakIntrospectionConfig::default();
    assert_eq!(defaults.max_entries, 10_000);
    assert_eq!(defaults.default_ttl_secs, 60);
    assert_eq!(defaults.negative_ttl_secs, 5);
}

#[test]
fn keycloak_introspection_config_parses_from_toml() {
    let toml = r#"
            server_url = "https://kc.example.com"
            realm = "test"
            client_id = "my-client"
            client_secret = "secret"

            [introspection]
            max_entries = 5000
            default_ttl_secs = 120
            negative_ttl_secs = 10
        "#;
    let config: KeycloakSecurityConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.introspection.max_entries, 5000);
    assert_eq!(config.introspection.default_ttl_secs, 120);
    assert_eq!(config.introspection.negative_ttl_secs, 10);
}

#[test]
fn keycloak_introspection_config_uses_defaults_when_omitted() {
    let toml = r#"
            server_url = "https://kc.example.com"
            realm = "test"
            client_id = "my-client"
            client_secret = "secret"
        "#;
    let config: KeycloakSecurityConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.introspection.max_entries, 10_000);
    assert_eq!(config.introspection.default_ttl_secs, 60);
    assert_eq!(config.introspection.negative_ttl_secs, 5);
}

#[test]
fn parse_security_permission_wasm_full_config() {
    let toml = r#"
[security.permissions.invoice-policy]
provider = "wasm"
path = "./policies/invoice-policy.wasm"

[security.permissions.invoice-policy.config]
tenant_header = "CamelTenantId"
mode = "enforce"

[security.permissions.invoice-policy.cache]
positive_ttl_secs = 60
negative_ttl_secs = 10
max_entries = 5000
"#;

    let config: CamelConfig = toml::from_str(toml).unwrap();
    let permissions = config.security.permissions.unwrap();
    let policy = permissions.get("invoice-policy").unwrap();

    assert_eq!(policy.provider, "wasm");
    assert_eq!(
        policy.path.as_deref(),
        Some("./policies/invoice-policy.wasm")
    );
    let cfg = policy.config.as_ref().unwrap();
    assert_eq!(cfg.get("tenant_header").unwrap(), "CamelTenantId");
    assert_eq!(cfg.get("mode").unwrap(), "enforce");
    assert_eq!(policy.cache.positive_ttl_secs, 60);
    assert_eq!(policy.cache.negative_ttl_secs, 10);
    assert_eq!(policy.cache.max_entries, 5000);
}

#[test]
fn parse_security_permission_minimal_provider_uses_cache_defaults() {
    let toml = r#"
[security.permissions.invoice-policy]
provider = "wasm"
"#;

    let config: CamelConfig = toml::from_str(toml).unwrap();
    let permissions = config.security.permissions.unwrap();
    let policy = permissions.get("invoice-policy").unwrap();

    assert_eq!(policy.provider, "wasm");
    assert_eq!(policy.path, None);
    assert_eq!(policy.config, None);
    assert_eq!(policy.cache.positive_ttl_secs, 30);
    assert_eq!(policy.cache.negative_ttl_secs, 5);
    assert_eq!(policy.cache.max_entries, 10_000);
}

#[test]
fn security_permissions_absent_by_default() {
    let config = SecurityConfig::default();
    assert!(config.permissions.is_none());
}

#[test]
fn parse_security_policies_wasm_full_config() {
    let toml = r#"
[security.policies.wasm.corp-auth]
path = "plugins/authz.wasm"

[security.policies.wasm.corp-auth.limits]
timeout-secs = 30
max-memory = 52428800

[security.policies.wasm.corp-auth.config]
ldap_url = "ldap://corp"
retry_count = "3"
"#;
    let config: CamelConfig = toml::from_str(toml).unwrap();
    let policies = config.security.policies.unwrap();
    let policy = policies.wasm.get("corp-auth").unwrap();
    assert_eq!(policy.path, "plugins/authz.wasm");
    assert_eq!(policy.limits.timeout_secs, Some(30));
    assert_eq!(policy.limits.max_memory, Some(52_428_800));
    assert_eq!(policy.config.get("ldap_url").unwrap(), "ldap://corp");
    assert_eq!(policy.config.get("retry_count").unwrap(), "3");
}

#[test]
fn parse_security_policies_wasm_minimal_config() {
    let toml = r#"
[security.policies.wasm.corp-auth]
path = "plugins/authz.wasm"
"#;
    let config: CamelConfig = toml::from_str(toml).unwrap();
    let policies = config.security.policies.unwrap();
    let policy = policies.wasm.get("corp-auth").unwrap();
    assert_eq!(policy.path, "plugins/authz.wasm");
    assert_eq!(
        policy.limits,
        crate::wasm_limits::WasmLimitsConfig::default()
    );
    assert!(policy.config.is_empty());
}

#[test]
fn parse_security_policies_wasm_deny_unknown_fields() {
    let toml = r#"
[security.policies.wasm.corp-auth]
path = "plugins/authz.wasm"
unknown_key = "rejected"
"#;
    let result: Result<CamelConfig, _> = toml::from_str(toml);
    assert!(
        result.is_err(),
        "deny_unknown_fields must reject unknown keys"
    );
}
