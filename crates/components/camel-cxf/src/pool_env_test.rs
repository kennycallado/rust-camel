//! Redaction coverage for the bridge spawn env-var trace (bd rc-m1qw7).
//!
//! `pool::start_bridge_inner` logs the env vars handed to the CXF bridge
//! subprocess as KEY names only (see `pool::env_var_keys`). The flattened
//! `env_vars` payload carries raw credentials — `CXF_ADDRESS` is a URL-shaped
//! consumer bind address, `*_SIG_USERNAME`/`*_ENC_USERNAME` are usernames, and
//! `CxfProfileEnvVars::to_env_vars` Deref-unwraps `Redacted` passwords into
//! plain strings — so the Debug render of the logged field must contain the
//! key names and none of the values.

use std::path::PathBuf;

use camel_bridge::process::{BridgeProcessConfig, CxfProfileEnvVars, Redacted};

use crate::pool::env_var_keys;

/// Realistic `env_vars` payload as built by `start_bridge_inner`: profile
/// vars from `cxf_profiles` (passwords Deref-unwrapped to plain strings by
/// `to_env_vars`) plus the pool-level `CXF_ADDRESS` bind URL.
fn bridge_env_vars_with_secrets() -> Vec<(String, String)> {
    let profiles = vec![CxfProfileEnvVars {
        name: "baleares".to_string(),
        wsdl_path: "/etc/cxf/baleares.wsdl".to_string(),
        service_name: "Svc".to_string(),
        port_name: "Port".to_string(),
        address: Some(
            "http://admin:s3cret@soap.example.com:9000/OrderService?user=bob".to_string(),
        ),
        keystore_path: Some("/etc/cxf/keystore.p12".to_string()),
        keystore_password: Some(Redacted::new("kspass-1".to_string())),
        truststore_path: Some("/etc/cxf/truststore.p12".to_string()),
        truststore_password: Some(Redacted::new("tspass-2".to_string())),
        sig_username: Some("sig-user-9".to_string()),
        sig_password: Some(Redacted::new("sigpass-3".to_string())),
        enc_username: Some("enc-user-7".to_string()),
        security_actions_out: None,
        security_actions_in: None,
        signature_algorithm: None,
        signature_digest_algorithm: None,
        signature_c14n_algorithm: None,
        signature_parts: None,
    }];
    let mut config =
        BridgeProcessConfig::cxf_profiles(PathBuf::from("/tmp/cxf-bridge"), &profiles, 15_000);
    config.env_vars.push((
        "CXF_ADDRESS".to_string(),
        "http://admin:s3cret@soap.internal:9443/cxf?tok=abc".to_string(),
    ));
    config.env_vars
}

#[test]
fn env_var_keys_logs_key_names_for_diagnostics() {
    let rendered = format!("{:?}", env_var_keys(&bridge_env_vars_with_secrets()));
    assert!(rendered.contains("CXF_PROFILES"), "got: {rendered}");
    assert!(rendered.contains("CXF_ADDRESS"), "got: {rendered}");
    assert!(
        rendered.contains("CXF_PROFILE_BALEARES_SIG_USERNAME"),
        "got: {rendered}"
    );
    assert!(
        rendered.contains("CXF_PROFILE_BALEARES_ENC_USERNAME"),
        "got: {rendered}"
    );
}

#[test]
fn env_var_keys_render_never_contains_address_values() {
    let rendered = format!("{:?}", env_var_keys(&bridge_env_vars_with_secrets()));
    assert!(!rendered.contains("s3cret"), "leak: {rendered}");
    assert!(!rendered.contains("soap.example.com"), "leak: {rendered}");
    assert!(!rendered.contains("soap.internal"), "leak: {rendered}");
    assert!(!rendered.contains("user=bob"), "leak: {rendered}");
}

#[test]
fn env_var_keys_render_never_contains_username_values() {
    let rendered = format!("{:?}", env_var_keys(&bridge_env_vars_with_secrets()));
    assert!(!rendered.contains("sig-user-9"), "leak: {rendered}");
    assert!(!rendered.contains("enc-user-7"), "leak: {rendered}");
}

#[test]
fn env_var_keys_render_never_contains_password_values() {
    // `to_env_vars` Deref-unwraps `Redacted` passwords into plain strings,
    // so the flattened `env_vars` carry raw credentials: a value dump at
    // any log level would leak them (beyond the address/username scope
    // noted in bd rc-m1qw7). The keys-only dump closes this too.
    let rendered = format!("{:?}", env_var_keys(&bridge_env_vars_with_secrets()));
    assert!(!rendered.contains("kspass-1"), "leak: {rendered}");
    assert!(!rendered.contains("tspass-2"), "leak: {rendered}");
    assert!(!rendered.contains("sigpass-3"), "leak: {rendered}");
}

#[test]
fn env_var_keys_preserves_entry_count_and_order() {
    let vars = bridge_env_vars_with_secrets();
    let keys = env_var_keys(&vars);
    assert_eq!(keys.len(), vars.len());
    assert_eq!(keys.first().copied(), Some("CXF_PROFILES"));
    assert_eq!(keys.last().copied(), Some("CXF_ADDRESS"));
}
