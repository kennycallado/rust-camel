use super::*;

/// Loads a `CamelConfig` from TOML text through the real config-building
/// path (profile handling, deserialization, placeholder resolution, validation).
fn load_config(toml_text: &str) -> Result<CamelConfig, ConfigError> {
    let value: toml::Value = toml::from_str(toml_text).expect("test TOML must parse");
    super::build_from_toml_value_inner(value, None, false, Vec::new())
}

#[test]
fn security_bearer_token_env_resolves() {
    let _guard = super::env_lock();
    set_env("AUTH_TOKEN", "real-secret");
    let config = load_config(
        r#"
[security.native]
subject = "svc"
bearer_token = "${env:AUTH_TOKEN}"
"#,
    )
    .expect("config should load");
    assert_eq!(
        config
            .security
            .native
            .as_ref()
            .and_then(|n| n.bearer_token.as_deref()),
        Some("real-secret")
    );
    unset_env("AUTH_TOKEN");
}

#[test]
fn security_unset_env_fails_closed() {
    let _guard = super::env_lock();
    unset_env("AUTH_TOKEN");
    let err = load_config(
        r#"
[security.native]
subject = "svc"
bearer_token = "${env:AUTH_TOKEN}"
"#,
    )
    .expect_err("unset credential env var must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("AUTH_TOKEN"),
        "message should name the var: {msg}"
    );
    assert!(
        msg.contains("bearer_token"),
        "message should name the field: {msg}"
    );
}

#[test]
fn security_explicit_default_resolves() {
    let _guard = super::env_lock();
    unset_env("AUTH_TOKEN");
    let config = load_config(
        r#"
[security.native]
subject = "svc"
bearer_token = "${env:AUTH_TOKEN:-fallback-secret}"
"#,
    )
    .expect("explicit default should resolve without error");
    assert_eq!(
        config
            .security
            .native
            .as_ref()
            .and_then(|n| n.bearer_token.as_deref()),
        Some("fallback-secret")
    );
}

// The old dash-default rejection tests (`dash_default_rejected_*`) were
// deleted: `:-` is the NATIVE default separator under the `${env:}`
// syntax (the rc-0wvi dash trap died with the legacy single-colon
// default form).

#[test]
fn noncredential_security_leaf_resolves() {
    let _guard = super::env_lock();
    unset_env("KC_REALM");
    let config = load_config(
        r#"
[security.keycloak]
server_url = "http://localhost:8080"
realm = "${env:KC_REALM:-main}"
client_id = "client"
client_secret = "secret"
"#,
    )
    .expect("non-credential security leaf should resolve");
    assert_eq!(config.security.keycloak.unwrap().realm, "main");
}

#[test]
fn datasource_leaves_resolve() {
    let _guard = super::env_lock();
    set_env("DB_URL", "postgresql://localhost:5432/db");
    set_env("SURREAL_PASS", "s3cret");
    let config = load_config(
        r#"
[datasources.main]
db_url = "${env:DB_URL}"

[datasources.main.extra]
password = "${env:SURREAL_PASS}"
"#,
    )
    .expect("datasource leaves should resolve");
    let ds = config.datasources.get("main").expect("main datasource");
    assert_eq!(ds.db_url, "postgresql://localhost:5432/db");
    assert_eq!(
        ds.extra.get("password").and_then(|v| v.as_str()),
        Some("s3cret")
    );
    unset_env("DB_URL");
    unset_env("SURREAL_PASS");
}

#[test]
fn datasource_ssl_leaves_resolve() {
    let _guard = super::env_lock();
    set_env("SSL_MODE", "require");
    set_env("SSL_ROOT_CERT", "/etc/certs/ca.pem");
    set_env("SSL_CERT", "/etc/certs/client.pem");
    set_env("SSL_KEY", "/etc/certs/client-key.pem");
    let config = load_config(
        r#"
[datasources.main]
db_url = "postgres://localhost/orders"
ssl_mode = "${env:SSL_MODE}"
ssl_root_cert = "${env:SSL_ROOT_CERT}"
ssl_cert = "${env:SSL_CERT}"
ssl_key = "${env:SSL_KEY}"
"#,
    )
    .expect("datasource ssl_* leaves should resolve");
    let ds = config.datasources.get("main").expect("main datasource");
    assert_eq!(ds.ssl_mode.as_deref(), Some("require"));
    assert_eq!(ds.ssl_root_cert.as_deref(), Some("/etc/certs/ca.pem"));
    assert_eq!(ds.ssl_cert.as_deref(), Some("/etc/certs/client.pem"));
    assert_eq!(ds.ssl_key.as_deref(), Some("/etc/certs/client-key.pem"));
    unset_env("SSL_MODE");
    unset_env("SSL_ROOT_CERT");
    unset_env("SSL_CERT");
    unset_env("SSL_KEY");
}

#[test]
fn datasource_ssl_leaf_unset_env_fails_closed() {
    let _guard = super::env_lock();
    let marker = "${env:SSL_VAR}";
    for field in ["ssl_mode", "ssl_root_cert", "ssl_cert", "ssl_key"] {
        unset_env("SSL_VAR");
        let err = load_config(&format!(
            r#"
[datasources.main]
db_url = "postgres://localhost/orders"
{field} = "{marker}"
"#
        ))
        .expect_err("unset env var on a datasource ssl_* leaf must fail closed");
        let msg = err.to_string();
        assert!(
            msg.contains("SSL_VAR"),
            "message should name the var: {msg}"
        );
        assert!(
            msg.contains(field),
            "message should name the field {field}: {msg}"
        );
    }
}

#[test]
fn datasource_provider_leaf_resolves() {
    let _guard = super::env_lock();
    set_env("DS_PROVIDER", "postgres");
    let config = load_config(
        r#"
[datasources.main]
db_url = "postgres://localhost/orders"
provider = "${env:DS_PROVIDER}"
"#,
    )
    .expect("datasource provider leaf should resolve");
    let ds = config.datasources.get("main").expect("main datasource");
    assert_eq!(ds.provider.as_deref(), Some("postgres"));
    unset_env("DS_PROVIDER");
}

#[test]
fn datasource_provider_leaf_unset_env_fails_closed() {
    let _guard = super::env_lock();
    unset_env("DS_PROVIDER");
    let err = load_config(
        r#"
[datasources.main]
db_url = "postgres://localhost/orders"
provider = "${env:DS_PROVIDER}"
"#,
    )
    .expect_err("unset env var on the datasource provider leaf must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("DS_PROVIDER"),
        "message should name the var: {msg}"
    );
    assert!(
        msg.contains("provider"),
        "message should name the field provider: {msg}"
    );
}

#[test]
fn surviving_marker_rejected() {
    let _guard = super::env_lock();
    let err = load_config(
        r#"
[security.native]
subject = "svc"
bearer_token = "${env:"
"#,
    )
    .expect_err("surviving placeholder marker must fail closed");
    assert!(
        err.to_string().contains("marker"),
        "message should mention the marker: {err}"
    );
}

#[test]
fn wasm_limits_allow_call_schemes_resolves() {
    let _guard = super::env_lock();
    unset_env("WASM_SCHEMES");
    let config = load_config(
        r#"
[security.policies.wasm.corp-auth]
path = "plugins/authz.wasm"

[security.policies.wasm.corp-auth.limits]
allow-call-schemes = "${env:WASM_SCHEMES:-file,https}"
"#,
    )
    .expect("wasm limits allow_call_schemes should resolve");
    let policy = config
        .security
        .policies
        .as_ref()
        .and_then(|p| p.wasm.get("corp-auth"))
        .expect("corp-auth policy");
    assert_eq!(
        policy.limits.allow_call_schemes.as_deref(),
        Some("file,https")
    );
}

#[test]
fn strict_prefixes_content_is_deliberate() {
    // Tripwire: the strict-class set is the contract for the path-prefix
    // dispatch in `resolve_tree_placeholders`. Changing it without updating
    // this literal means a credential surface silently lost (or gained)
    // strict resolution.
    assert_eq!(
        STRICT_PREFIXES,
        &["security", "datasources", "idempotent_repo", "cache_repo"]
    );
}
