use super::*;

/// Loads a `CamelConfig` from TOML text through the real config-building
/// path (profile handling, deserialization, placeholder resolution, validation).
fn load_config(toml_text: &str) -> Result<CamelConfig, ConfigError> {
    let value: toml::Value = toml::from_str(toml_text).expect("test TOML must parse");
    super::build_from_toml_value_inner(value, None, false, Vec::new())
}

#[test]
fn credentials_array_parses() {
    let config = load_config(
        r#"
[security.native]
subject = "svc"

[[security.native.credentials]]
subject = "svc-a"
secret_env = "SVC_A_SECRET"
roles = ["admin", "reader"]
scopes = ["read", "write"]

[[security.native.credentials]]
subject = "svc-b"
secret = "plain-text-secret"
"#,
    )
    .expect("config should load");

    let native = config.security.native.expect("native config present");
    let creds = &native.credentials;
    assert_eq!(creds.len(), 2);
    assert_eq!(creds[0].subject, "svc-a");
    assert_eq!(creds[0].secret_env.as_deref(), Some("SVC_A_SECRET"));
    assert_eq!(creds[0].secret, None);
    assert_eq!(
        creds[0].roles,
        vec!["admin".to_string(), "reader".to_string()]
    );
    assert_eq!(
        creds[0].scopes,
        vec!["read".to_string(), "write".to_string()]
    );
    assert_eq!(creds[1].subject, "svc-b");
    assert_eq!(creds[1].secret.as_deref(), Some("plain-text-secret"));
    assert_eq!(creds[1].secret_env, None);
    assert!(creds[1].roles.is_empty());
    assert!(creds[1].scopes.is_empty());
}

#[test]
fn entry_with_both_secrets_rejected() {
    let err = load_config(
        r#"
[security.native]
subject = "svc"

[[security.native.credentials]]
subject = "svc-a"
secret_env = "SVC_A_SECRET"
secret = "also-a-secret"
"#,
    )
    .expect_err("both secret_env and secret must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("credentials[0]"), "must name index: {msg}");
    assert!(
        msg.contains("exactly one"),
        "must explain the constraint: {msg}"
    );
}

#[test]
fn entry_with_no_secret_rejected() {
    let err = load_config(
        r#"
[security.native]
subject = "svc"

[[security.native.credentials]]
subject = "svc-a"
"#,
    )
    .expect_err("neither secret_env nor secret must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("credentials[0]"), "must name index: {msg}");
}

#[test]
fn entry_with_empty_subject_rejected() {
    let err = load_config(
        r#"
[security.native]
subject = "svc"

[[security.native.credentials]]
subject = ""
secret = "x"
"#,
    )
    .expect_err("empty subject must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("credentials[0]"), "must name index: {msg}");
}

#[test]
fn credentials_debug_redacts() {
    let config = load_config(
        r#"
[security.native]
subject = "svc"

[[security.native.credentials]]
subject = "svc-a"
secret = "hunter2-debug-secret"
"#,
    )
    .expect("config should load");
    let debug = format!("{:?}", config.security.native);
    assert!(
        !debug.contains("hunter2-debug-secret"),
        "debug must not leak the secret: {debug}"
    );
    let creds = &config.security.native.expect("native").credentials;
    let entry_debug = format!("{:?}", creds[0]);
    assert!(
        entry_debug.contains("[REDACTED]"),
        "entry debug must redact the secret: {entry_debug}"
    );
    assert!(
        !entry_debug.contains("hunter2-debug-secret"),
        "entry debug must not leak the secret: {entry_debug}"
    );
}

#[test]
fn credential_secret_placeholder_resolves() {
    let _guard = super::env_lock();
    set_env("CRED_ONE", "resolved-env-secret");
    let config = load_config(
        r#"
[security.native]
subject = "svc"

[[security.native.credentials]]
subject = "svc-a"
secret = "${env:CRED_ONE}"
"#,
    )
    .expect("env-backed secret should resolve");
    let creds = &config.security.native.expect("native").credentials;
    assert_eq!(creds[0].secret.as_deref(), Some("resolved-env-secret"));
    unset_env("CRED_ONE");
}

#[test]
fn credential_secret_unset_fails_closed() {
    let _guard = super::env_lock();
    unset_env("CRED_ONE");
    let err = load_config(
        r#"
[security.native]
subject = "svc"

[[security.native.credentials]]
subject = "svc-a"
secret = "${env:CRED_ONE}"
"#,
    )
    .expect_err("unset credential secret must fail closed");
    let msg = err.to_string();
    assert!(msg.contains("CRED_ONE"), "must name the var: {msg}");
    assert!(
        msg.contains("credentials[0].secret") || msg.contains("credentials[0]"),
        "must name the field path: {msg}"
    );
}
