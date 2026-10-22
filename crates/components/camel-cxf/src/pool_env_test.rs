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

use camel_bridge::process::{BridgeProcessConfig, CxfProfileEnvVars, CxfTlsEnvVars, Redacted};

use crate::config::{CxfListenerTls, CxfPoolConfig, CxfProfileConfig, CxfSecurityFields};
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
        allowed_signature_algorithms: Some(
            "http://www.w3.org/2000/09/xmldsig#rsa-sha1".to_string(),
        ),
        allowed_digest_algorithms: Some("http://www.w3.org/2000/09/xmldsig#sha1".to_string()),
        allow_expired_pinned_certificate: false,
        timestamp_ttl_seconds: None,
        timestamp_future_tolerance_seconds: None,
        fault_status: None,
        exposed_soap_headers: vec!["{http://partner.example/ns}header-SENTINEL".to_string()],
        tls: Some(CxfTlsEnvVars {
            keystore_path: Some("/etc/cxf/tls-keystore.p12".to_string()),
            keystore_password: Some(Redacted::new("tlspass-ks".to_string())),
            keystore_type: Some("PKCS12".to_string()),
            truststore_path: Some("/etc/cxf/tls-truststore.p12".to_string()),
            truststore_password: Some(Redacted::new("tlspass-ts".to_string())),
            truststore_type: Some("PKCS12".to_string()),
        }),
    }]
}

#[test]
fn tls_env_vars_are_prefixed_and_redacted() {
    let profiles = secret_profiles();
    let config =
        BridgeProcessConfig::cxf_profiles(PathBuf::from("/tmp/cxf-bridge"), &profiles, 15_000);
    let keys = env_var_keys(&config.env_vars);
    for suffix in [
        "TLS_KEYSTORE_PATH",
        "TLS_KEYSTORE_PASSWORD",
        "TLS_KEYSTORE_TYPE",
        "TLS_TRUSTSTORE_PATH",
        "TLS_TRUSTSTORE_PASSWORD",
        "TLS_TRUSTSTORE_TYPE",
    ] {
        let key = format!("CXF_PROFILE_BALEARES_{suffix}");
        assert!(keys.contains(&key.as_str()), "missing {key}: {keys:?}");
        assert!(!key.contains("tlspass"), "password leaked into key {key}");
    }

    // `CxfTlsConfig`'s manual Debug redacts both passwords.
    let tls_config = crate::config::CxfTlsConfig {
        keystore_password: Some("tlspass-ks".to_string()),
        truststore_password: Some("tlspass-ts".to_string()),
        ..Default::default()
    };
    let debug = format!("{tls_config:?}");
    assert!(!debug.contains("tlspass-ks"), "leak: {debug}");
    assert!(!debug.contains("tlspass-ts"), "leak: {debug}");

    // The bridge config Debug redacts the flattened env var values too.
    let debug = format!("{config:?}");
    assert!(!debug.contains("tlspass-ks"), "leak: {debug}");
    assert!(!debug.contains("tlspass-ts"), "leak: {debug}");
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
    assert!(
        rendered.contains("CXF_PROFILE_BALEARES_EXPOSED_SOAP_HEADERS"),
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
fn env_var_keys_render_never_contains_exposed_header_values() {
    let rendered = format!("{:?}", env_var_keys(&bridge_env_vars_with_secrets()));
    assert!(!rendered.contains("header-SENTINEL"), "leak: {rendered}");
    assert!(!rendered.contains("partner.example"), "leak: {rendered}");
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
        None,
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
    assert!(
        captured.contains("CXF_PROFILE_BALEARES_ALLOWED_SIGNATURE_ALGORITHMS"),
        "missing key in {captured}"
    );
    assert!(
        captured.contains("CXF_PROFILE_BALEARES_ALLOWED_DIGEST_ALGORITHMS"),
        "missing key in {captured}"
    );
    assert!(
        captured.contains("CXF_PROFILE_BALEARES_EXPOSED_SOAP_HEADERS"),
        "missing key in {captured}"
    );

    // VALUES absent — a revert to `env_vars = ?config.env_vars` dumps all of
    // these (address credentials, usernames, Deref-unwrapped passwords).
    assert!(!captured.contains("s3cret"), "leak: {captured}");
    assert!(!captured.contains("soap.internal"), "leak: {captured}");
    assert!(!captured.contains("tok=abc"), "leak: {captured}");
    assert!(!captured.contains("sig-user-9"), "leak: {captured}");
    assert!(!captured.contains("enc-user-7"), "leak: {captured}");
    assert!(!captured.contains("header-SENTINEL"), "leak: {captured}");
    assert!(!captured.contains("partner.example"), "leak: {captured}");
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

// --- Inbound algorithm allow-lists are opt-in env vars (Task 2.2) ---

/// Profile with the inbound verification allow-lists set (or not) exactly as
/// parsed from `[profiles.security]`.
fn allowed_algorithms_profile(
    name: &str,
    allowed_signature_algorithms: Option<&str>,
    allowed_digest_algorithms: Option<&str>,
) -> CxfProfileConfig {
    CxfProfileConfig {
        name: name.to_string(),
        address: None,
        wsdl_path: "/etc/cxf/baleares.wsdl".to_string(),
        service_name: "Svc".to_string(),
        port_name: "Port".to_string(),
        security: CxfSecurityFields {
            allowed_signature_algorithms: allowed_signature_algorithms.map(str::to_string),
            allowed_digest_algorithms: allowed_digest_algorithms.map(str::to_string),
            ..Default::default()
        },
    }
}

fn env_map(config: &BridgeProcessConfig) -> std::collections::HashMap<&str, &str> {
    config
        .env_vars
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

#[test]
fn pool_env_emits_allowed_algorithms_only_when_set() {
    const SIG: &str = "http://www.w3.org/2000/09/xmldsig#rsa-sha1,http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
    const DIGEST: &str = "http://www.w3.org/2000/09/xmldsig#sha1";

    let opted_in = crate::pool::profile_env_vars_for(&[allowed_algorithms_profile(
        "legacy_opt_in",
        Some(SIG),
        Some(DIGEST),
    )]);
    let config = crate::pool::bridge_config_with_env_trace(
        "slot-1",
        PathBuf::from("/tmp/cxf-bridge"),
        &opted_in,
        None,
        15_000,
        None,
    );
    let env = env_map(&config);
    assert_eq!(
        env.get("CXF_PROFILE_LEGACY_OPT_IN_ALLOWED_SIGNATURE_ALGORITHMS")
            .copied(),
        Some(SIG)
    );
    assert_eq!(
        env.get("CXF_PROFILE_LEGACY_OPT_IN_ALLOWED_DIGEST_ALGORITHMS")
            .copied(),
        Some(DIGEST)
    );

    let secure_default = crate::pool::profile_env_vars_for(&[allowed_algorithms_profile(
        "secure_default",
        None,
        None,
    )]);
    let config = crate::pool::bridge_config_with_env_trace(
        "slot-1",
        PathBuf::from("/tmp/cxf-bridge"),
        &secure_default,
        None,
        15_000,
        None,
    );
    let env = env_map(&config);
    assert!(
        !env.contains_key("CXF_PROFILE_SECURE_DEFAULT_ALLOWED_SIGNATURE_ALGORITHMS"),
        "unset allow-list must not be emitted: {env:?}"
    );
    assert!(
        !env.contains_key("CXF_PROFILE_SECURE_DEFAULT_ALLOWED_DIGEST_ALGORITHMS"),
        "unset allow-list must not be emitted: {env:?}"
    );
}

// --- Expired pinned-certificate opt-in (Task 4) ---

/// Profile carrying the expired pinned-certificate opt-in (or not) as parsed
/// from `[profiles.security]`.
fn expired_pinned_profile(name: &str, allow_expired_pinned_certificate: bool) -> CxfProfileConfig {
    CxfProfileConfig {
        name: name.to_string(),
        address: None,
        wsdl_path: "/etc/cxf/baleares.wsdl".to_string(),
        service_name: "Svc".to_string(),
        port_name: "Port".to_string(),
        security: CxfSecurityFields {
            truststore_path: Some("/etc/cxf/truststore.jks".to_string()),
            truststore_password: Some("changeit".to_string()),
            allow_expired_pinned_certificate,
            ..Default::default()
        },
    }
}

#[test]
fn pool_env_emits_allow_expired_pinned_certificate_only_when_true() {
    let opted_in = crate::pool::profile_env_vars_for(&[expired_pinned_profile("opted", true)]);
    let config = crate::pool::bridge_config_with_env_trace(
        "slot-true",
        PathBuf::from("/tmp/cxf-bridge"),
        &opted_in,
        None,
        15_000,
        None,
    );
    let env = env_map(&config);
    assert_eq!(
        env.get("CXF_PROFILE_OPTED_ALLOW_EXPIRED_PINNED_CERTIFICATE")
            .copied(),
        Some("true")
    );

    let default_off =
        crate::pool::profile_env_vars_for(&[expired_pinned_profile("default", false)]);
    let config = crate::pool::bridge_config_with_env_trace(
        "slot-false",
        PathBuf::from("/tmp/cxf-bridge"),
        &default_off,
        None,
        15_000,
        None,
    );
    let env = env_map(&config);
    assert!(
        !env.keys()
            .any(|key| key.ends_with("ALLOW_EXPIRED_PINNED_CERTIFICATE")),
        "default-off profile must not emit the opt-in: {env:?}"
    );
}

// --- Per-profile timestamp freshness window (Task 4) ---

/// Profile carrying the timestamp window values as parsed from
/// `[profiles.security]`; the truststore pair keeps the config valid.
fn timestamp_window_profile(
    name: &str,
    ttl: Option<u64>,
    future_tolerance: Option<u64>,
) -> CxfProfileConfig {
    CxfProfileConfig {
        name: name.to_string(),
        address: None,
        wsdl_path: "/etc/cxf/baleares.wsdl".to_string(),
        service_name: "Svc".to_string(),
        port_name: "Port".to_string(),
        security: CxfSecurityFields {
            truststore_path: Some("/etc/cxf/truststore.jks".to_string()),
            truststore_password: Some("changeit".to_string()),
            timestamp_ttl_seconds: ttl,
            timestamp_future_tolerance_seconds: future_tolerance,
            ..Default::default()
        },
    }
}

#[test]
fn pool_env_carries_timestamp_window_from_profile() {
    let profiles = crate::pool::profile_env_vars_for(&[timestamp_window_profile(
        "window",
        Some(900),
        Some(120),
    )]);
    let config = crate::pool::bridge_config_with_env_trace(
        "slot-ts",
        PathBuf::from("/tmp/cxf-bridge"),
        &profiles,
        None,
        15_000,
        None,
    );
    let env = env_map(&config);
    assert_eq!(
        env.get("CXF_PROFILE_WINDOW_TIMESTAMP_TTL_SECONDS").copied(),
        Some("900")
    );
    assert_eq!(
        env.get("CXF_PROFILE_WINDOW_TIMESTAMP_FUTURE_TOLERANCE_SECONDS")
            .copied(),
        Some("120")
    );

    let default =
        crate::pool::profile_env_vars_for(&[timestamp_window_profile("default", None, None)]);
    let config = crate::pool::bridge_config_with_env_trace(
        "slot-ts-default",
        PathBuf::from("/tmp/cxf-bridge"),
        &default,
        None,
        15_000,
        None,
    );
    let env = env_map(&config);
    assert!(
        !env.contains_key("CXF_PROFILE_DEFAULT_TIMESTAMP_TTL_SECONDS"),
        "unset ttl must not be emitted: {env:?}"
    );
    assert!(
        !env.contains_key("CXF_PROFILE_DEFAULT_TIMESTAMP_FUTURE_TOLERANCE_SECONDS"),
        "unset future tolerance must not be emitted: {env:?}"
    );
}

#[test]
fn pool_env_carries_int_max_timestamp_window_from_toml() {
    let toml_str = r#"
        [[profiles]]
        name = "huge"
        wsdl_path = "/etc/cxf/huge.wsdl"
        service_name = "Svc"
        port_name = "Port"

        [profiles.security]
        truststore_path = "/etc/cxf/truststore.jks"
        truststore_password = "changeit"
        timestamp_ttl_seconds = 2147483647
        timestamp_future_tolerance_seconds = 2147483647
    "#;
    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
    cfg.validate().expect("int max window validates");

    let profiles = crate::pool::profile_env_vars_for(&cfg.profiles);
    let config = crate::pool::bridge_config_with_env_trace(
        "slot-huge",
        PathBuf::from("/tmp/cxf-bridge"),
        &profiles,
        None,
        15_000,
        None,
    );
    let env = env_map(&config);
    assert_eq!(
        env.get("CXF_PROFILE_HUGE_TIMESTAMP_TTL_SECONDS").copied(),
        Some("2147483647")
    );
    assert_eq!(
        env.get("CXF_PROFILE_HUGE_TIMESTAMP_FUTURE_TOLERANCE_SECONDS")
            .copied(),
        Some("2147483647")
    );
}

// --- Listener TLS travels as pool env vars, never per-profile (Task 2.6) ---

/// Unchecked fixture with ALL five record fields set: the emission mapping for
/// every `CXF_SOAP_TLS_*` key is asserted in one config. The shape rule (exactly
/// one key-material form) is validated at config load; this fixture bypasses it
/// deliberately.
fn full_listener_tls() -> CxfListenerTls {
    CxfListenerTls {
        cert_path: Some("/etc/cxf/server.pem".to_string()),
        key_path: Some("/etc/cxf/server.key".to_string()),
        client_ca_path: Some("/etc/cxf/ca.pem".to_string()),
        pkcs12_path: Some("/etc/cxf/server.p12".to_string()),
        pkcs12_password: Some("pkcs12-SENTINEL".to_string()),
    }
}

/// Known profile with NO producer TLS (`[profiles.security.tls]` unset): any
/// `CXF_PROFILE_*_TLS_*` key in the emitted env can only come from listener
/// material, which must never travel the per-profile path.
fn no_tls_profiles() -> Vec<CxfProfileConfig> {
    vec![CxfProfileConfig {
        name: "baleares".to_string(),
        address: None,
        wsdl_path: "/etc/cxf/baleares.wsdl".to_string(),
        service_name: "Svc".to_string(),
        port_name: "Port".to_string(),
        security: Default::default(),
    }]
}

fn tls_env_keys(config: &BridgeProcessConfig) -> Vec<String> {
    config
        .env_vars
        .iter()
        .map(|(k, _)| k.clone())
        .filter(|k| k.starts_with("CXF_SOAP_TLS_"))
        .collect()
}

#[test]
fn listener_tls_uses_pool_env_vars_only() {
    let profiles = crate::pool::profile_env_vars_for(&no_tls_profiles());

    // All five keys asserted from the full-field fixture.
    let config = crate::pool::bridge_config_with_env_trace(
        "slot-1",
        PathBuf::from("/tmp/cxf-bridge"),
        &profiles,
        Some("https://0.0.0.0:9443/soap"),
        15_000,
        Some(&full_listener_tls()),
    );
    let keys = env_var_keys(&config.env_vars);
    for key in [
        "CXF_SOAP_TLS_CERT_PATH",
        "CXF_SOAP_TLS_KEY_PATH",
        "CXF_SOAP_TLS_CLIENT_CA_PATH",
        "CXF_SOAP_TLS_PKCS12_PATH",
        "CXF_SOAP_TLS_PKCS12_PASSWORD",
    ] {
        assert!(keys.contains(&key), "missing {key}: {keys:?}");
    }
    assert!(
        !keys
            .iter()
            .any(|k| k.starts_with("CXF_PROFILE_") && k.contains("TLS_")),
        "listener TLS must not travel as per-profile vars: {keys:?}"
    );
    // A SOAP bind opts in to the soap-ready expectation.
    assert!(config.expect_soap_ready);

    // Valid PEM record: only the configured keys are emitted, never empty vars.
    let pem = CxfListenerTls {
        cert_path: Some("/etc/cxf/server.pem".to_string()),
        key_path: Some("/etc/cxf/server.key".to_string()),
        client_ca_path: Some("/etc/cxf/ca.pem".to_string()),
        ..Default::default()
    };
    let config = crate::pool::bridge_config_with_env_trace(
        "slot-1",
        PathBuf::from("/tmp/cxf-bridge"),
        &profiles,
        Some("https://0.0.0.0:9443/soap"),
        15_000,
        Some(&pem),
    );
    assert_eq!(
        tls_env_keys(&config),
        vec![
            "CXF_SOAP_TLS_CERT_PATH",
            "CXF_SOAP_TLS_KEY_PATH",
            "CXF_SOAP_TLS_CLIENT_CA_PATH",
        ],
        "a valid record emits exactly its configured keys"
    );
    assert!(
        config
            .env_vars
            .iter()
            .filter(|(k, _)| k.starts_with("CXF_SOAP_TLS_"))
            .all(|(_, v)| !v.is_empty()),
        "no empty env values"
    );

    // Secret hygiene: the PKCS12 password value lives only in the env VALUE
    // (the bridge needs it), never in a key name or in Debug.
    let pkcs12 = CxfListenerTls {
        pkcs12_path: Some("/etc/cxf/server.p12".to_string()),
        pkcs12_password: Some("pkcs12-SENTINEL".to_string()),
        client_ca_path: Some("/etc/cxf/ca.pem".to_string()),
        ..Default::default()
    };
    let config = crate::pool::bridge_config_with_env_trace(
        "slot-1",
        PathBuf::from("/tmp/cxf-bridge"),
        &profiles,
        Some("https://0.0.0.0:9443/soap"),
        15_000,
        Some(&pkcs12),
    );
    let rendered_keys = format!("{:?}", env_var_keys(&config.env_vars));
    assert!(
        !rendered_keys.contains("pkcs12-SENTINEL"),
        "leak: {rendered_keys}"
    );
    let rendered_config = format!("{config:?}");
    assert!(
        !rendered_config.contains("pkcs12-SENTINEL"),
        "leak: {rendered_config}"
    );
    let rendered_tls = format!("{pkcs12:?}");
    assert!(
        !rendered_tls.contains("pkcs12-SENTINEL"),
        "leak: {rendered_tls}"
    );

    // No bind → no soap-ready expectation (opt-in is bind-gated).
    let no_bind = crate::pool::bridge_config_with_env_trace(
        "slot-1",
        PathBuf::from("/tmp/cxf-bridge"),
        &profiles,
        None,
        15_000,
        Some(&pem),
    );
    assert!(!no_bind.expect_soap_ready);
}

/// Health-monitor restart re-emits the slot's persisted listener TLS through
/// the same builder as startup. Both configs come from
/// [`crate::pool::bridge_config_for_slot`] — the exact call `start_bridge_inner`
/// and the health-monitor restart path make — with the slot as the ONLY value
/// source (the startup record is dropped before the restart call), so the
/// assertion is on slot-persisted values, not on twice-fed test arguments.
#[test]
fn listener_tls_survives_restart() {
    let pkcs12_password = "pkcs12-SENTINEL";
    for (label, tls) in [
        (
            "pem",
            CxfListenerTls {
                cert_path: Some("/etc/cxf/server.pem".to_string()),
                key_path: Some("/etc/cxf/server.key".to_string()),
                client_ca_path: Some("/etc/cxf/ca.pem".to_string()),
                ..Default::default()
            },
        ),
        (
            "pkcs12",
            CxfListenerTls {
                pkcs12_path: Some("/etc/cxf/server.p12".to_string()),
                pkcs12_password: Some(pkcs12_password.to_string()),
                client_ca_path: Some("/etc/cxf/ca.pem".to_string()),
                ..Default::default()
            },
        ),
    ] {
        // Persisted slot record: the only value source after startup.
        let mut tls_source = Some(tls);
        let (state_tx, state_rx) = tokio::sync::watch::channel(crate::pool::BridgeState::Starting);
        let slot = crate::pool::BridgeSlot {
            key: crate::pool::CxfBridgePool::slot_key(),
            configured_profiles: no_tls_profiles(),
            bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
            listener_tls: tls_source.clone(),
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
        };

        let startup =
            crate::pool::bridge_config_for_slot(&slot, PathBuf::from("/tmp/cxf-bridge"), 15_000);

        // Startup's record is gone; the health-monitor restart sees only the
        // slot's persisted copy as its value source.
        drop(tls_source.take());
        let restart =
            crate::pool::bridge_config_for_slot(&slot, PathBuf::from("/tmp/cxf-bridge"), 15_000);

        assert!(slot.listener_tls.is_some(), "{label}: record not persisted");
        let mut startup_keys = env_var_keys(&startup.env_vars).to_vec();
        let mut restart_keys = env_var_keys(&restart.env_vars).to_vec();
        startup_keys.sort_unstable();
        restart_keys.sort_unstable();
        assert_eq!(
            startup_keys, restart_keys,
            "{label}: restart dropped env keys"
        );
        for key in [
            "CXF_SOAP_TLS_CERT_PATH",
            "CXF_SOAP_TLS_KEY_PATH",
            "CXF_SOAP_TLS_CLIENT_CA_PATH",
            "CXF_SOAP_TLS_PKCS12_PATH",
            "CXF_SOAP_TLS_PKCS12_PASSWORD",
        ] {
            let expected = match key {
                "CXF_SOAP_TLS_CERT_PATH" => label == "pem",
                "CXF_SOAP_TLS_KEY_PATH" => label == "pem",
                "CXF_SOAP_TLS_PKCS12_PATH" => label == "pkcs12",
                "CXF_SOAP_TLS_PKCS12_PASSWORD" => label == "pkcs12",
                _ => true,
            };
            assert_eq!(
                restart.env_vars.iter().any(|(k, _)| k == key),
                expected,
                "{label}: {key} presence mismatch"
            );
        }
        let mut startup_pairs = startup.env_vars.clone();
        let mut restart_pairs = restart.env_vars.clone();
        startup_pairs.sort();
        restart_pairs.sort();
        assert_eq!(
            startup_pairs, restart_pairs,
            "{label}: restart changed values"
        );

        // Secret Debug redaction holds on both configs and on the record.
        for rendered in [
            format!("{startup:?}"),
            format!("{restart:?}"),
            format!("{:?}", slot.listener_tls),
        ] {
            assert!(
                !rendered.contains(pkcs12_password),
                "{label}: leak: {rendered}"
            );
        }
        assert_eq!(
            startup.expect_soap_ready, restart.expect_soap_ready,
            "{label}: expect_soap_ready must survive restart"
        );
        assert!(startup.expect_soap_ready, "{label}: bind present opts in");
    }
}
