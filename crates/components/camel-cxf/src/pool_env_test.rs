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
use std::sync::{Arc, Mutex};

use camel_bridge::process::{BridgeProcessConfig, CxfProfileEnvVars, Redacted};

use crate::pool::env_var_keys;

/// Realistic profile set as built by `start_bridge_inner`: one profile whose
/// `to_env_vars` flattening Deref-unwraps `Redacted` passwords into plain
/// strings (see `CxfProfileEnvVars::to_env_vars`).
fn secret_profiles() -> Vec<CxfProfileEnvVars> {
    vec![CxfProfileEnvVars {
        name: "baleares".to_string(),
        wsdl_path: "/etc/cxf/baleares.wsdl".to_string(),
        service_name: "Svc".to_string(),
        port_name: "Port".to_string(),
        address: Some("http://admin:s3cret@soap.internal:9443/cxf?tok=abc".to_string()),
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
    }]
}

/// Realistic `env_vars` payload as built by `start_bridge_inner`: profile
/// vars from `cxf_profiles` (passwords Deref-unwrapped to plain strings by
/// `to_env_vars`) plus the pool-level `CXF_ADDRESS` bind URL.
fn bridge_env_vars_with_secrets() -> Vec<(String, String)> {
    let mut config = BridgeProcessConfig::cxf_profiles(
        PathBuf::from("/tmp/cxf-bridge"),
        &secret_profiles(),
        15_000,
    );
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
    assert!(!rendered.contains("soap.internal"), "leak: {rendered}");
    assert!(!rendered.contains("tok=abc"), "leak: {rendered}");
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

// --- Tracing capture helper for the bridge-spawn trace site ---

/// `MakeWriter` that appends formatted events to a shared `Vec<u8>` sink.
/// Used by the site-guard test to assert that the bridge-spawn `trace!`
/// record renders env var KEY names and none of the values (bd rc-2eckt).
/// The sink collects the ANSI-stripped fmt layer output.
#[derive(Clone)]
struct CapturingWriter {
    sink: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for CapturingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sink.lock().unwrap().extend_from_slice(buf); // allow-unwrap: test-only
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingWriter {
    type Writer = CapturingWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Install a bare registry as the process-global tracing default, once
/// per test binary. Guards the `capture_sink` tests against
/// callsite-interest poisoning: `tracing` caches each callsite's
/// `Interest` process-wide from its FIRST macro execution, evaluated
/// against the executing thread's dispatcher. Subscriber-less sibling
/// tests in this binary hit the shared bridge-spawn `trace!` callsite
/// (`crate::pool::bridge_config_with_env_trace`) first and cache
/// `Interest::never`, so a later thread-local `set_default` capture
/// silently drops events. The global registry heals prior poison and
/// floors future rebuilds at `sometimes`
/// (fix pattern: c3853198; bd rc-img5; bd rc-6jarb; convention:
/// docs/testing/tracing-capture-guards.md).
fn ensure_global_tracing_default() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INIT.set(()).is_ok() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    }
}

fn capture_sink() -> (Arc<Mutex<Vec<u8>>>, impl tracing::Subscriber) {
    ensure_global_tracing_default();
    let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let writer = CapturingWriter {
        sink: Arc::clone(&sink),
    };
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    (sink, subscriber)
}

/// The bridge-spawn trace site itself must render env var KEY names only.
///
/// `env_var_keys` is covered by the tests above, but the log SITE was not:
/// reverting the trace to `env_vars = ?config.env_vars` (a value dump) would
/// pass all of them. This test drives `bridge_config_with_env_trace` — the
/// extracted config-build + trace block from `start_bridge_inner` — through a
/// capture subscriber, so the site itself is guarded (bd rc-2eckt).
#[test]
fn bridge_env_trace_site_renders_keys_not_values() {
    let (sink, subscriber) = capture_sink();
    let _guard = tracing::subscriber::set_default(subscriber);

    let config = crate::pool::bridge_config_with_env_trace(
        "slot-1",
        PathBuf::from("/tmp/cxf-bridge"),
        &secret_profiles(),
        Some("http://admin:s3cret@soap.internal:9443/cxf?tok=abc"),
        15_000,
    );
    drop(_guard);

    let captured = String::from_utf8(sink.lock().unwrap().clone()).unwrap(); // allow-unwrap: test-only

    // KEYS survive — these double as proof the event was actually captured:
    // a silently-empty capture fails here, never false-passes.
    assert!(
        captured.contains("CXF_PROFILES"),
        "missing key in {captured}"
    );
    assert!(
        captured.contains("CXF_ADDRESS"),
        "missing key in {captured}"
    );
    assert!(
        captured.contains("CXF_PROFILE_BALEARES_SIG_USERNAME"),
        "missing key in {captured}"
    );
    assert!(
        captured.contains("CXF_PROFILE_BALEARES_ENC_USERNAME"),
        "missing key in {captured}"
    );

    // VALUES absent — a revert to `env_vars = ?config.env_vars` dumps all of
    // these (address credentials, usernames, Deref-unwrapped passwords).
    assert!(!captured.contains("s3cret"), "leak: {captured}");
    assert!(!captured.contains("soap.internal"), "leak: {captured}");
    assert!(!captured.contains("tok=abc"), "leak: {captured}");
    assert!(!captured.contains("sig-user-9"), "leak: {captured}");
    assert!(!captured.contains("enc-user-7"), "leak: {captured}");
    assert!(!captured.contains("kspass-1"), "leak: {captured}");
    assert!(!captured.contains("tspass-2"), "leak: {captured}");
    assert!(!captured.contains("sigpass-3"), "leak: {captured}");

    // Config still built correctly: the pushed CXF_ADDRESS pair is last.
    let (last_key, last_val) = config.env_vars.last().expect("env_vars non-empty");
    assert_eq!(last_key, "CXF_ADDRESS");
    assert_eq!(
        last_val,
        "http://admin:s3cret@soap.internal:9443/cxf?tok=abc"
    );
}
