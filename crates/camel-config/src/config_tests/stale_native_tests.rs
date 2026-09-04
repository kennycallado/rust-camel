use super::*;

/// Loads a `CamelConfig` from TOML text through the real config-building
/// path (profile handling, deserialization, placeholder resolution, validation).
fn load_config(toml_text: &str) -> Result<CamelConfig, ConfigError> {
    let value: toml::Value = toml::from_str(toml_text).expect("test TOML must parse");
    super::build_from_toml_value_inner(value, None, false, Vec::new(), &super::ambient_lookup())
}

#[test]
fn stale_token_issuer_rejected_loudly() {
    let err = load_config(
        r#"
[security.native]
subject = "svc"

[security.native.token_issuer]
issuer = "https://orders.local"
signing_key_env = "KEY"
"#,
    )
    .expect_err("stale `token_issuer` key must be rejected (deny_unknown_fields)");
    let msg = err.to_string();
    assert!(
        msg.contains("token_issuer"),
        "message should name the stale key: {msg}"
    );
}

#[test]
fn stale_clients_rejected_loudly() {
    let err = load_config(
        r#"
[security.native]
subject = "svc"

[[security.native.clients]]
client_id = "worker"
client_secret_env = "SECRET"
"#,
    )
    .expect_err("stale `clients` key must be rejected (deny_unknown_fields)");
    let msg = err.to_string();
    assert!(
        msg.contains("clients"),
        "message should name the stale key: {msg}"
    );
}
