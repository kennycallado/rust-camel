//! Tests for the embedded virtual-store configuration seam
//! (`CamelConfig::from_toml_value_with_env`, openspec change `multidoc`
//! Task 2.2).
//!
//! The seam builds a `CamelConfig` from an already-merged TOML tree —
//! the shape `camel_dsl::discover_virtual_store` produces — resolving
//! `${env:}` placeholders through an injected lookup ONLY: no file
//! read, no ambient `CAMEL_PROFILE` selection, no allowlisted
//! `CAMEL_*` override merge, no include expansion.

use super::*;

/// Parse TOML text into the merged-tree shape the seam consumes.
fn merged(toml_text: &str) -> toml::Value {
    toml::from_str(toml_text).expect("test TOML must parse")
}

/// The deployment lookup the artifact runtime injects (ambient process
/// environment), for the ambient-interaction tests.
fn ambient(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[test]
fn virtual_store_env_resolves_through_injected_lookup() {
    let _guard = super::env_lock();
    unset_env("VS_BEARER");
    // The injected lookup is the ONLY source: the name is unset in the
    // ambient environment, so resolution proves lookup exclusivity.
    let lookup = |name: &str| match name {
        "VS_BEARER" => Some("injected-secret".to_string()),
        _ => None,
    };
    let config = CamelConfig::from_toml_value_with_env(
        merged(
            r#"
[security.native]
subject = "svc"
bearer_token = "${env:VS_BEARER}"
"#,
        ),
        &lookup,
    )
    .expect("injected lookup must resolve the placeholder");
    assert_eq!(
        config
            .security
            .native
            .as_ref()
            .and_then(|n| n.bearer_token.as_deref()),
        Some("injected-secret")
    );
}

#[test]
fn virtual_store_unset_env_fails_closed() {
    let _guard = super::env_lock();
    unset_env("VS_BEARER");
    let err = CamelConfig::from_toml_value_with_env(
        merged(
            r#"
[security.native]
subject = "svc"
bearer_token = "${env:VS_BEARER}"
"#,
        ),
        &ambient,
    )
    .expect_err("unset referenced var must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("VS_BEARER"),
        "message should name the var: {msg}"
    );
    assert!(
        msg.contains("bearer_token"),
        "message should name the field: {msg}"
    );
}

#[test]
fn virtual_store_does_not_merge_ambient_camel_overrides() {
    let _guard = super::env_lock();
    set_env("CAMEL_LOG_LEVEL", "warn");
    set_env("CAMEL_PROFILE", "prod");
    let config = CamelConfig::from_toml_value_with_env(
        // Final merged shape: a flat tree, profile selection already
        // performed at compile time.
        merged("log_level = \"debug\"\nwatch = false\n"),
        &ambient,
    )
    .expect("flat merged tree must load");
    assert_eq!(
        config.log_level, "debug",
        "ambient CAMEL_LOG_LEVEL must never override the embedded value"
    );
    assert!(!config.watch, "embedded watch flag is authoritative");
    unset_env("CAMEL_LOG_LEVEL");
    unset_env("CAMEL_PROFILE");
}

#[test]
fn virtual_store_empty_tree_yields_serde_defaults() {
    let config =
        CamelConfig::from_toml_value_with_env(toml::Value::Table(toml::Table::new()), &ambient)
            .expect("empty merged tree must deserialize to defaults");
    assert!(
        config.routes.is_empty(),
        "no routes entries without embedded configuration"
    );
    assert!(!config.watch, "watch defaults to off");
}

#[test]
fn virtual_store_typed_mismatch_fails_closed() {
    let err =
        CamelConfig::from_toml_value_with_env(merged("timeout_ms = \"not-a-number\"\n"), &ambient)
            .expect_err("a quoted numeric must stay rejected");
    assert!(
        err.to_string().contains("timeout_ms"),
        "message should name the field: {err}"
    );
}
