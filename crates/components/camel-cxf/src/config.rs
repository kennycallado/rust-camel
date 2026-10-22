use std::fmt;
use std::path::PathBuf;

use camel_bridge::process::{CxfTlsEnvVars, Redacted};
use camel_component_api::NetworkRetryPolicy;

use crate::BRIDGE_VERSION;

fn default_max_bridges() -> usize {
    4
}

fn default_bridge_start_timeout_ms() -> u64 {
    30_000
}

fn default_health_check_interval_ms() -> u64 {
    5_000
}

/// Per-component reconnect default: unlimited retries (max_attempts=0),
/// preserving the previous infinite-reconnect behavior. Operators can opt
/// into bounded retry via TOML `[reconnect]`.
fn cxf_reconnect_default() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 0, // unlimited
        ..NetworkRetryPolicy::default()
    }
}

/// Validates that a profile name contains only `[a-z0-9_]+`.
pub fn validate_profile_name(name: &str) -> Result<(), camel_component_api::CamelError> {
    if name.is_empty() {
        return Err(camel_component_api::CamelError::ProcessorError(
            "profile name must not be empty".to_string(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(camel_component_api::CamelError::ProcessorError(format!(
            "profile name '{}' must contain only lowercase letters, digits, and underscores",
            name
        )));
    }
    Ok(())
}

/// Bind-address scheme (lowercased) or `None` when no scheme is present.
pub(crate) fn bind_address_scheme(address: &str) -> Option<String> {
    address.split_once(':').map(|(s, _)| s.to_ascii_lowercase())
}

/// Validates the consumer-side bind address forwarded to the bridge as the
/// `CXF_ADDRESS` env var. Only `http` and `https` are accepted as schemes; the
/// TLS material interplay (https needs material, material needs https) is
/// validated by [`CxfPoolConfig::validate`].
pub(crate) fn validate_consumer_address(
    address: &str,
) -> Result<(), camel_component_api::CamelError> {
    match bind_address_scheme(address).as_deref() {
        Some("http") | Some("https") => Ok(()),
        other => Err(camel_component_api::CamelError::Config(format!(
            "cxf.bind_address scheme '{}' is not supported: use http:// or https://",
            other.unwrap_or("<missing scheme>")
        ))),
    }
}

/// Producer transport key/trust store type. TOML accepts `PKCS12` (default) or
/// `JKS` only — any other spelling is rejected at parse time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[non_exhaustive]
pub enum KeyStoreType {
    /// PKCS#12 store (the default).
    #[default]
    #[serde(rename = "PKCS12")]
    Pkcs12,
    /// Java KeyStore.
    #[serde(rename = "JKS")]
    Jks,
}

impl KeyStoreType {
    /// Canonical serialized name emitted to the bridge env vars.
    pub fn as_str(self) -> &'static str {
        match self {
            KeyStoreType::Pkcs12 => "PKCS12",
            KeyStoreType::Jks => "JKS",
        }
    }
}

/// Nested `[profiles.security.tls]` producer transport TLS material.
///
/// Credentials are redacted from `Debug` (ADR-0051). Pair completeness is
/// validated by `CxfTlsConfig::validate`; the block is serialized as
/// `CXF_PROFILE_<NAME>_TLS_*` env vars by [`CxfTlsConfig::to_env`].
#[derive(Clone, Default, serde::Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct CxfTlsConfig {
    pub keystore_path: Option<String>,
    pub keystore_password: Option<String>,
    pub keystore_type: KeyStoreType,
    pub truststore_path: Option<String>,
    pub truststore_password: Option<String>,
    pub truststore_type: KeyStoreType,
}

impl fmt::Debug for CxfTlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CxfTlsConfig")
            .field("keystore_path", &self.keystore_path)
            .field(
                "keystore_password",
                &self.keystore_password.as_ref().map(|_| "<redacted>"),
            )
            .field("keystore_type", &self.keystore_type)
            .field("truststore_path", &self.truststore_path)
            .field(
                "truststore_password",
                &self.truststore_password.as_ref().map(|_| "<redacted>"),
            )
            .field("truststore_type", &self.truststore_type)
            .finish()
    }
}

impl CxfTlsConfig {
    /// Bidirectional pair validation: `*_path` and `*_password` are supplied
    /// together for both the keystore and the truststore.
    fn validate(&self, profile: &str) -> Result<(), camel_component_api::CamelError> {
        validate_tls_pair(
            profile,
            "keystore",
            self.keystore_path.as_deref(),
            self.keystore_password.as_deref(),
        )?;
        validate_tls_pair(
            profile,
            "truststore",
            self.truststore_path.as_deref(),
            self.truststore_password.as_deref(),
        )?;
        Ok(())
    }

    /// Map the parsed TLS config onto the bridge env-var carrier.
    pub fn to_env(&self) -> CxfTlsEnvVars {
        CxfTlsEnvVars {
            keystore_path: self.keystore_path.clone(),
            keystore_password: self.keystore_password.clone().map(Redacted::new),
            keystore_type: self
                .keystore_path
                .as_ref()
                .map(|_| self.keystore_type.as_str().to_string()),
            truststore_path: self.truststore_path.clone(),
            truststore_password: self.truststore_password.clone().map(Redacted::new),
            truststore_type: self
                .truststore_path
                .as_ref()
                .map(|_| self.truststore_type.as_str().to_string()),
        }
    }
}

/// Fail-closed blank TLS-value check (N4 parity): a supplied value that is
/// empty or Unicode `White_Space`-only is rejected by BOTH layers instead of
/// being silently treated as unset. `char::is_whitespace` implements the
/// Unicode `White_Space` property — the exact set the Java bridge's
/// `BlankTlsValue` predicate encodes (NBSP U+00A0, NEL U+0085, and U+202F
/// included, unlike Java's `String.isBlank`). Nonblank values are never
/// trimmed; only a genuinely omitted (`None`) value counts as unset.
fn is_blank_tls_value(value: &str) -> bool {
    value.chars().all(char::is_whitespace)
}

fn validate_tls_pair(
    profile: &str,
    kind: &str,
    path: Option<&str>,
    password: Option<&str>,
) -> Result<(), camel_component_api::CamelError> {
    // N4 fail-closed: an explicitly supplied empty/whitespace-only value is
    // invalid — never silently unset (a both-blank pair previously passed and
    // silently selected JVM store defaults on the Java side).
    if path.is_some_and(is_blank_tls_value) {
        return Err(camel_component_api::CamelError::Config(format!(
            "cxf profile '{profile}': tls.{kind}_path is empty or whitespace-only — supply a \
             nonblank value or omit the key (blank TLS values are rejected, not treated as \
             unset)"
        )));
    }
    if password.is_some_and(is_blank_tls_value) {
        return Err(camel_component_api::CamelError::Config(format!(
            "cxf profile '{profile}': tls.{kind}_password is empty or whitespace-only — supply \
             a nonblank value or omit the key (blank TLS values are rejected, not treated as \
             unset)"
        )));
    }
    match (path, password) {
        (Some(_), None) => Err(camel_component_api::CamelError::Config(format!(
            "cxf profile '{profile}': tls.{kind}_path is set but tls.{kind}_password is missing"
        ))),
        (None, Some(_)) => Err(camel_component_api::CamelError::Config(format!(
            "cxf profile '{profile}': tls.{kind}_password is set but tls.{kind}_path is missing"
        ))),
        _ => Ok(()),
    }
}

#[derive(Clone, Default, serde::Deserialize)]
pub struct CxfSecurityFields {
    pub keystore_path: Option<String>,
    pub keystore_password: Option<String>,
    pub truststore_path: Option<String>,
    pub truststore_password: Option<String>,
    pub sig_username: Option<String>,
    pub sig_password: Option<String>,
    pub enc_username: Option<String>,
    pub security_actions_out: Option<String>,
    pub security_actions_in: Option<String>,
    pub signature_algorithm: Option<String>,
    pub signature_digest_algorithm: Option<String>,
    pub signature_c14n_algorithm: Option<String>,
    pub signature_parts: Option<String>,
    /// Comma-separated inbound signature algorithm allow-list (absolute URIs).
    /// Replaces the bridge's secure default when set; exempt from the signing
    /// preconditions because it is an inbound verification knob.
    pub allowed_signature_algorithms: Option<String>,
    /// Comma-separated inbound digest algorithm allow-list (absolute URIs).
    /// Replaces the bridge's secure default when set; exempt from the signing
    /// preconditions because it is an inbound verification knob.
    pub allowed_digest_algorithms: Option<String>,
    /// Per-profile opt-in accepting an expired inbound certificate only when
    /// its full DER is pinned in the profile's keystore or truststore.
    /// Defaults to `false`; the Java bridge remains the authoritative
    /// fail-closed validator.
    #[serde(default)]
    pub allow_expired_pinned_certificate: bool,
    /// Consumer inbound Timestamp TTL in seconds (`Created` may be at most this old).
    /// `None` keeps the bridge default (300). Accepted range 1..=2147483647 (the
    /// WSS4J `int` limit); larger values are rejected at startup.
    #[serde(default)]
    pub timestamp_ttl_seconds: Option<u64>,
    /// Consumer inbound Timestamp future tolerance in seconds (`Created` may be at
    /// most this far ahead). `None` keeps the bridge default (60). Accepted range
    /// 1..=2147483647 (the WSS4J `int` limit); larger values are rejected at startup.
    #[serde(default)]
    pub timestamp_future_tolerance_seconds: Option<u64>,
    /// Optional per-profile HTTP status override applied to **security (WSS)
    /// faults only**, on both SOAP versions. Validated to `400..=599` at
    /// startup; out-of-range values fail the pool config before the bridge
    /// starts.
    #[serde(default)]
    pub fault_status: Option<u16>,
    /// Nested `[profiles.security.tls]` producer transport TLS material.
    #[serde(default)]
    pub tls: Option<CxfTlsConfig>,
}

impl fmt::Debug for CxfSecurityFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CxfSecurityFields")
            .field("keystore_path", &self.keystore_path)
            .field(
                "keystore_password",
                &self.keystore_password.as_ref().map(|_| "<redacted>"),
            )
            .field("truststore_path", &self.truststore_path)
            .field(
                "truststore_password",
                &self.truststore_password.as_ref().map(|_| "<redacted>"),
            )
            .field("sig_username", &self.sig_username)
            .field(
                "sig_password",
                &self.sig_password.as_ref().map(|_| "<redacted>"),
            )
            .field("enc_username", &self.enc_username)
            .field("security_actions_out", &self.security_actions_out)
            .field("security_actions_in", &self.security_actions_in)
            .field("signature_algorithm", &self.signature_algorithm)
            .field(
                "signature_digest_algorithm",
                &self.signature_digest_algorithm,
            )
            .field("signature_c14n_algorithm", &self.signature_c14n_algorithm)
            .field("signature_parts", &self.signature_parts)
            .field(
                "allowed_signature_algorithms",
                &self.allowed_signature_algorithms,
            )
            .field("allowed_digest_algorithms", &self.allowed_digest_algorithms)
            .field(
                "allow_expired_pinned_certificate",
                &self.allow_expired_pinned_certificate,
            )
            .field("timestamp_ttl_seconds", &self.timestamp_ttl_seconds)
            .field(
                "timestamp_future_tolerance_seconds",
                &self.timestamp_future_tolerance_seconds,
            )
            .field("fault_status", &self.fault_status)
            .field("tls", &self.tls)
            .finish()
    }
}

impl CxfSecurityFields {
    /// Lightweight mirror of the Java `ALLOWED_*` construction checks. The
    /// lists are inbound verification knobs, so they are exempt from the
    /// signing preconditions (no keystore or outbound action is required), but
    /// they are inert without verification material. Java remains the
    /// authoritative validator for URI absoluteness; here only `://` presence
    /// is checked.
    fn validate_allowed_algorithms(
        &self,
        profile: &str,
    ) -> Result<(), camel_component_api::CamelError> {
        let has_verification_material =
            self.keystore_path.is_some() || self.truststore_path.is_some();
        for (key, raw) in [
            (
                "allowed_signature_algorithms",
                self.allowed_signature_algorithms.as_deref(),
            ),
            (
                "allowed_digest_algorithms",
                self.allowed_digest_algorithms.as_deref(),
            ),
        ] {
            let Some(raw) = raw else { continue };
            let mut parsed = 0usize;
            let mut blank_segment = false;
            for segment in raw.split(',') {
                let segment = segment.trim();
                if segment.is_empty() {
                    blank_segment = true;
                    continue;
                }
                if !segment.contains("://") {
                    return Err(camel_component_api::CamelError::Config(format!(
                        "cxf profile '{profile}': security.{key} segment '{segment}' is not an \
                         absolute URI; algorithm URIs must be absolute (for example \
                         'http://www.w3.org/2001/04/xmldsig-more#rsa-sha256')"
                    )));
                }
                parsed += 1;
            }
            if parsed == 0 {
                return Err(camel_component_api::CamelError::Config(format!(
                    "cxf profile '{profile}': security.{key} is set but lists no algorithm URIs; \
                     an empty allow-list would allow every algorithm, including SHA-1 — remove \
                     the option to use the secure defaults or list at least one absolute \
                     algorithm URI"
                )));
            }
            if blank_segment {
                return Err(camel_component_api::CamelError::Config(format!(
                    "cxf profile '{profile}': security.{key} contains a blank algorithm URI \
                     segment; remove the empty entry and list only absolute algorithm URIs"
                )));
            }
            if !has_verification_material {
                return Err(camel_component_api::CamelError::Config(format!(
                    "cxf profile '{profile}': security.{key} is set but the profile has no \
                     inbound verification material (neither security.keystore_path nor \
                     security.truststore_path); the allow-list would be silently inert — \
                     configure a truststore or keystore, or remove the option"
                )));
            }
        }
        Ok(())
    }

    /// Fail-closed consumer inbound Timestamp freshness window. A zero value is
    /// rejected; a window is inert without verification material, so setting it
    /// without a keystore or truststore is rejected too.
    fn validate_timestamp_window(
        &self,
        profile: &str,
    ) -> Result<(), camel_component_api::CamelError> {
        let has_verification_material = self
            .keystore_path
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty())
            || self
                .truststore_path
                .as_deref()
                .is_some_and(|p| !p.trim().is_empty());
        for (key, value) in [
            ("timestamp_ttl_seconds", self.timestamp_ttl_seconds),
            (
                "timestamp_future_tolerance_seconds",
                self.timestamp_future_tolerance_seconds,
            ),
        ] {
            let Some(v) = value else { continue };
            if v == 0 {
                return Err(camel_component_api::CamelError::Config(format!(
                    "cxf profile '{profile}': security.{key} must be a positive number of \
                     seconds; 0 is not allowed"
                )));
            }
            if v > i32::MAX as u64 {
                return Err(camel_component_api::CamelError::Config(format!(
                    "cxf profile {profile}: security.{key} must be at most 2147483647 seconds \
                     (WSS4J int limit), got {v}"
                )));
            }
            if !has_verification_material {
                return Err(camel_component_api::CamelError::Config(format!(
                    "cxf profile '{profile}': security.{key} is set but the profile has no \
                     inbound verification material (neither security.keystore_path nor \
                     security.truststore_path); the timestamp window would be silently inert — \
                     configure a truststore or keystore, or remove the option"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct CxfProfileConfig {
    pub name: String,
    pub address: Option<String>,
    pub wsdl_path: String,
    pub service_name: String,
    pub port_name: String,
    #[serde(default)]
    pub security: CxfSecurityFields,
}

impl CxfProfileConfig {
    pub fn env_prefix(&self) -> String {
        format!("CXF_PROFILE_{}_", self.name.to_uppercase())
    }
}

/// Pool-level listener transport TLS material, configured beside `bind_address`
/// through the five flat pool keys (`soap_tls_cert_path`, `soap_tls_key_path`,
/// `soap_tls_client_ca_path`, `soap_tls_pkcs12_path`, `soap_tls_pkcs12_password`)
/// and assembled by [`TryFrom<CxfPoolConfigRaw>`].
///
/// Exactly one key-material form is valid: `cert_path` + `key_path` (PEM) or
/// `pkcs12_path` + `pkcs12_password` (PKCS#12); `client_ca_path` is always
/// required (mutual TLS). Shape rules are enforced by
/// [`CxfPoolConfig::validate`] at config load. The record is forwarded as the
/// five `CXF_SOAP_TLS_*` env vars on the non-profile path (see
/// `pool::bridge_config_with_env_trace`) and is independent of the WSS
/// message-layer stores. Credentials are redacted from `Debug` (ADR-0051).
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct CxfListenerTls {
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    pub client_ca_path: Option<String>,
    pub pkcs12_path: Option<String>,
    pub pkcs12_password: Option<String>,
}

impl fmt::Debug for CxfListenerTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CxfListenerTls")
            .field("cert_path", &self.cert_path)
            .field("key_path", &self.key_path)
            .field("client_ca_path", &self.client_ca_path)
            .field("pkcs12_path", &self.pkcs12_path)
            .field(
                "pkcs12_password",
                &self.pkcs12_password.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(try_from = "CxfPoolConfigRaw")]
pub struct CxfPoolConfig {
    #[serde(default)]
    pub profiles: Vec<CxfProfileConfig>,
    #[serde(default = "default_max_bridges")]
    pub max_bridges: usize,
    #[serde(default = "default_bridge_start_timeout_ms")]
    pub bridge_start_timeout_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
    pub bridge_cache_dir: Option<PathBuf>,
    #[serde(default = "default_bridge_version")]
    pub version: String,
    /// Optional HTTP bind address for the consumer-side SOAP endpoint published by
    /// the bridge process. Forwarded to the bridge as the `CXF_ADDRESS` env var.
    /// When unset the bridge falls back to its built-in default (`http://0.0.0.0:9000/cxf`).
    /// Format: `http://<host>:<port>/<base-path>` or `https://<host>:<port>/<base-path>`
    /// (https requires [`CxfListenerTls`] material) — profiles are routed under
    /// `<base-path>/<profile_name>` by the bridge.
    #[serde(default)]
    pub bind_address: Option<String>,
    /// Pool-level listener TLS material, assembled from the five `soap_tls_*`
    /// pool keys. Single source of truth carried into the bridge slot and
    /// re-emitted on restart.
    #[serde(default)]
    pub listener_tls: Option<CxfListenerTls>,
    /// Reconnection policy for transient bridge connection failures.
    /// Controls retry behavior when bridge channel connection fails during
    /// startup, health-monitor restarts, or channel refreshes.
    /// Default: unlimited (max_attempts=0), preserving previous infinite-retry behavior.
    #[serde(default = "cxf_reconnect_default")]
    pub reconnect: NetworkRetryPolicy,
}

#[derive(Clone, serde::Deserialize)]
struct CxfPoolConfigRaw {
    #[serde(default)]
    profiles: Vec<CxfProfileConfig>,
    #[serde(default = "default_max_bridges")]
    max_bridges: usize,
    #[serde(default = "default_bridge_start_timeout_ms")]
    bridge_start_timeout_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    health_check_interval_ms: u64,
    bridge_cache_dir: Option<PathBuf>,
    #[serde(default = "default_bridge_version")]
    version: String,
    #[serde(default)]
    bind_address: Option<String>,
    #[serde(default)]
    soap_tls_cert_path: Option<String>,
    #[serde(default)]
    soap_tls_key_path: Option<String>,
    #[serde(default)]
    soap_tls_client_ca_path: Option<String>,
    #[serde(default)]
    soap_tls_pkcs12_path: Option<String>,
    #[serde(default)]
    soap_tls_pkcs12_password: Option<String>,
    #[serde(default = "cxf_reconnect_default")]
    reconnect: NetworkRetryPolicy,
}

impl fmt::Debug for CxfPoolConfigRaw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CxfPoolConfigRaw")
            .field("profiles", &self.profiles)
            .field("max_bridges", &self.max_bridges)
            .field("bridge_start_timeout_ms", &self.bridge_start_timeout_ms)
            .field("health_check_interval_ms", &self.health_check_interval_ms)
            .field("bridge_cache_dir", &self.bridge_cache_dir)
            .field("version", &self.version)
            .field("bind_address", &self.bind_address)
            .field("soap_tls_cert_path", &self.soap_tls_cert_path)
            .field("soap_tls_key_path", &self.soap_tls_key_path)
            .field("soap_tls_client_ca_path", &self.soap_tls_client_ca_path)
            .field("soap_tls_pkcs12_path", &self.soap_tls_pkcs12_path)
            .field(
                "soap_tls_pkcs12_password",
                &self.soap_tls_pkcs12_password.as_ref().map(|_| "<redacted>"),
            )
            .field("reconnect", &self.reconnect)
            .finish()
    }
}

fn default_bridge_version() -> String {
    BRIDGE_VERSION.to_string()
}

impl Default for CxfPoolConfig {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            max_bridges: default_max_bridges(),
            bridge_start_timeout_ms: default_bridge_start_timeout_ms(),
            health_check_interval_ms: default_health_check_interval_ms(),
            bridge_cache_dir: None,
            version: default_bridge_version(),
            bind_address: None,
            listener_tls: None,
            reconnect: cxf_reconnect_default(),
        }
    }
}

impl CxfPoolConfig {
    pub fn validate(&self) -> Result<(), camel_component_api::CamelError> {
        if self.max_bridges == 0 {
            return Err(camel_component_api::CamelError::Config(
                "cxf.max_bridges must be greater than 0".into(),
            ));
        }
        if self.bridge_start_timeout_ms == 0 {
            return Err(camel_component_api::CamelError::Config(
                "cxf.bridge_start_timeout_ms must be greater than 0".into(),
            ));
        }
        if self.health_check_interval_ms == 0 {
            return Err(camel_component_api::CamelError::Config(
                "cxf.health_check_interval_ms must be greater than 0".into(),
            ));
        }
        if let Some(address) = self.bind_address.as_deref() {
            validate_consumer_address(address)?;
        }
        self.validate_listener_tls()?;
        for profile in &self.profiles {
            if let Some(fault_status) = profile.security.fault_status
                && !(400..=599).contains(&fault_status)
            {
                return Err(camel_component_api::CamelError::Config(format!(
                    "cxf profile '{}': security.fault_status {} is out of range; expected \
                     400-599 (security.fault_status overrides WSS fault statuses only and \
                     must be an HTTP error status)",
                    profile.name, fault_status
                )));
            }
            profile
                .security
                .validate_allowed_algorithms(&profile.name)?;
            profile.security.validate_timestamp_window(&profile.name)?;
            if let Some(tls) = &profile.security.tls {
                tls.validate(&profile.name)?;
            }
        }
        Ok(())
    }

    /// Fail-closed listener TLS shape (cxf-listener-tls): explicitly supplied
    /// empty/whitespace-only values are rejected (N4, same policy as the Java
    /// bridge), exactly one key-material
    /// form (`cert_path`+`key_path` PEM XOR `pkcs12_path`+`pkcs12_password`), no
    /// partial pairs, `client_ca_path` always required, TLS material only with an
    /// `https` bind, `https` only with material, and material without a bind is
    /// not applicable. Producer transport TLS (`[profiles.security.tls]`) is a
    /// distinct, per-profile concern and is never consulted here.
    fn validate_listener_tls(&self) -> Result<(), camel_component_api::CamelError> {
        let Some(tls) = self.listener_tls.as_ref() else {
            // No `soap_tls_*` key: an https bind without material fails closed.
            if let Some(address) = self.bind_address.as_deref()
                && bind_address_scheme(address).as_deref() == Some("https")
            {
                return Err(camel_component_api::CamelError::Config(
                    "cxf listener TLS: https bind_address requires key material — set \
                     soap_tls_cert_path+soap_tls_key_path (PEM) or \
                     soap_tls_pkcs12_path+soap_tls_pkcs12_password (PKCS#12)"
                        .to_string(),
                ));
            }
            return Ok(());
        };

        // N4 fail-closed: an explicitly supplied empty/whitespace-only value
        // is rejected at Rust config load — never silently unset, never
        // deferred to bridge startup.
        for (key, value) in [
            ("soap_tls_cert_path", &tls.cert_path),
            ("soap_tls_key_path", &tls.key_path),
            ("soap_tls_client_ca_path", &tls.client_ca_path),
            ("soap_tls_pkcs12_path", &tls.pkcs12_path),
            ("soap_tls_pkcs12_password", &tls.pkcs12_password),
        ] {
            if value.as_deref().is_some_and(is_blank_tls_value) {
                return Err(camel_component_api::CamelError::Config(format!(
                    "cxf listener TLS: {key} is empty or whitespace-only — supply a nonblank \
                     value or omit the key (blank TLS values are rejected, not treated as unset)"
                )));
            }
        }

        // Partial key-material pairs fail in both directions.
        match (tls.cert_path.as_deref(), tls.key_path.as_deref()) {
            (Some(_), None) => {
                return Err(camel_component_api::CamelError::Config(
                    "cxf listener TLS: soap_tls_cert_path is set but soap_tls_key_path \
                     is missing"
                        .to_string(),
                ));
            }
            (None, Some(_)) => {
                return Err(camel_component_api::CamelError::Config(
                    "cxf listener TLS: soap_tls_key_path is set but soap_tls_cert_path \
                     is missing"
                        .to_string(),
                ));
            }
            _ => {}
        }
        match (tls.pkcs12_path.as_deref(), tls.pkcs12_password.as_deref()) {
            (Some(_), None) => {
                return Err(camel_component_api::CamelError::Config(
                    "cxf listener TLS: soap_tls_pkcs12_path is set but \
                     soap_tls_pkcs12_password is missing"
                        .to_string(),
                ));
            }
            (None, Some(_)) => {
                return Err(camel_component_api::CamelError::Config(
                    "cxf listener TLS: soap_tls_pkcs12_password is set but \
                     soap_tls_pkcs12_path is missing"
                        .to_string(),
                ));
            }
            _ => {}
        }

        let pem_form = tls.cert_path.is_some() && tls.key_path.is_some();
        let pkcs12_form = tls.pkcs12_path.is_some() && tls.pkcs12_password.is_some();
        if pem_form && pkcs12_form {
            return Err(camel_component_api::CamelError::Config(
                "cxf listener TLS: supply exactly one key-material form — \
                 soap_tls_cert_path+soap_tls_key_path (PEM) or \
                 soap_tls_pkcs12_path+soap_tls_pkcs12_password (PKCS#12) — never both"
                    .to_string(),
            ));
        }
        if !pem_form && !pkcs12_form {
            return Err(camel_component_api::CamelError::Config(
                "cxf listener TLS: supply exactly one key-material form — \
                 soap_tls_cert_path+soap_tls_key_path (PEM) or \
                 soap_tls_pkcs12_path+soap_tls_pkcs12_password (PKCS#12) — none is \
                 configured"
                    .to_string(),
            ));
        }
        if tls.client_ca_path.is_none() {
            return Err(camel_component_api::CamelError::Config(
                "cxf listener TLS: soap_tls_client_ca_path is always required \
                 (mutual TLS client authentication)"
                    .to_string(),
            ));
        }
        match self.bind_address.as_deref() {
            None => {
                return Err(camel_component_api::CamelError::Config(
                    "cxf listener TLS: soap_tls_* material is configured but \
                     bind_address is absent; the listener TLS configuration is not \
                     applicable without a bind"
                        .to_string(),
                ));
            }
            Some(address) => {
                if bind_address_scheme(address).as_deref() != Some("https") {
                    return Err(camel_component_api::CamelError::Config(
                        "cxf listener TLS: bind_address must use https when \
                         soap_tls_* material is configured"
                            .to_string(),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl TryFrom<CxfPoolConfigRaw> for CxfPoolConfig {
    type Error = camel_component_api::CamelError;

    fn try_from(raw: CxfPoolConfigRaw) -> Result<Self, Self::Error> {
        // The five flat `soap_tls_*` keys are the only TOML surface; any of them
        // present assembles the record so validation sees partial shapes too.
        let listener_tls = {
            let tls = CxfListenerTls {
                cert_path: raw.soap_tls_cert_path,
                key_path: raw.soap_tls_key_path,
                client_ca_path: raw.soap_tls_client_ca_path,
                pkcs12_path: raw.soap_tls_pkcs12_path,
                pkcs12_password: raw.soap_tls_pkcs12_password,
            };
            let any_key = tls.cert_path.is_some()
                || tls.key_path.is_some()
                || tls.client_ca_path.is_some()
                || tls.pkcs12_path.is_some()
                || tls.pkcs12_password.is_some();
            any_key.then_some(tls)
        };
        let cfg = CxfPoolConfig {
            profiles: raw.profiles,
            max_bridges: raw.max_bridges,
            bridge_start_timeout_ms: raw.bridge_start_timeout_ms,
            health_check_interval_ms: raw.health_check_interval_ms,
            bridge_cache_dir: raw.bridge_cache_dir,
            version: raw.version,
            bind_address: raw.bind_address,
            listener_tls,
            reconnect: raw.reconnect,
        };
        cfg.validate()?;
        Ok(cfg)
    }
}

#[derive(Debug, Clone)]
pub struct CxfEndpointConfig {
    pub address: String,
    pub wsdl_path: String,
    pub service_name: String,
    pub port_name: String,
    pub operation: Option<String>,
    pub profile: Option<String>,
    pub timeout_ms: Option<u64>,
    /// When `true`, the producer sets `Content-Type: multipart/related` and
    /// `SOAPAction` headers on outgoing requests.
    ///
    /// /// TODO(CXF-014): full MTOM multipart encoding not yet implemented
    pub mtom_enabled: bool,
    /// Optional override for the attachment Content-Type used when `mtom_enabled`
    /// is `true`.
    pub attachment_content_type: Option<String>,
}

impl CxfEndpointConfig {
    pub fn validate(&self) -> Result<(), camel_component_api::CamelError> {
        if self.address.trim().is_empty() {
            return Err(camel_component_api::CamelError::Config(
                "cxf URI field 'address' must not be empty".into(),
            ));
        }
        if self.wsdl_path.trim().is_empty() {
            return Err(camel_component_api::CamelError::Config(
                "cxf URI field 'wsdl' must not be empty".into(),
            ));
        }
        if self.service_name.trim().is_empty() {
            return Err(camel_component_api::CamelError::Config(
                "cxf URI field 'service' must not be empty".into(),
            ));
        }
        if self.port_name.trim().is_empty() {
            return Err(camel_component_api::CamelError::Config(
                "cxf URI field 'port' must not be empty".into(),
            ));
        }
        if matches!(self.timeout_ms, Some(0)) {
            return Err(camel_component_api::CamelError::Config(
                "cxf URI field 'timeout_ms' must be greater than 0".into(),
            ));
        }
        Ok(())
    }

    pub fn from_uri(uri: &str) -> Result<Self, camel_component_api::CamelError> {
        let rest = uri.strip_prefix("cxf://").ok_or_else(|| {
            camel_component_api::CamelError::InvalidUri("expected scheme 'cxf://'".to_string())
        })?;

        let (path, query) = match rest.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (rest, None),
        };

        let address = if path.is_empty() {
            return Err(camel_component_api::CamelError::InvalidUri(
                "cxf URI must include an address after 'cxf://'".into(),
            ));
        } else {
            path.to_string()
        };

        let mut wsdl_path = None;
        let mut service_name = None;
        let mut port_name = None;
        let mut operation = None;
        let mut profile = None;
        let mut timeout_ms = None;
        let mut mtom_enabled = None;
        let mut attachment_content_type = None;
        let mut unknown_params = Vec::new();

        if let Some(q) = query {
            for kv in q.split('&') {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "wsdl" => wsdl_path = Some(v.to_string()),
                        "service" => service_name = Some(v.to_string()),
                        "port" => port_name = Some(v.to_string()),
                        "operation" => operation = Some(v.to_string()),
                        "profile" => profile = Some(v.to_string()),
                        "timeout_ms" => {
                            timeout_ms = Some(v.parse::<u64>().map_err(|_| {
                                camel_component_api::CamelError::InvalidUri(
                                    "cxf URI field 'timeout_ms' must be an unsigned integer"
                                        .to_string(),
                                )
                            })?)
                        }
                        "mtom_enabled" => {
                            mtom_enabled = Some(v.parse::<bool>().map_err(|_| {
                                camel_component_api::CamelError::InvalidUri(
                                    "cxf URI field 'mtom_enabled' must be 'true' or 'false'"
                                        .to_string(),
                                )
                            })?)
                        }
                        "attachment_content_type" => {
                            attachment_content_type = Some(v.to_string());
                        }
                        _ => unknown_params.push(k.to_string()),
                    }
                }
            }
        }

        if !unknown_params.is_empty() {
            return Err(camel_component_api::CamelError::InvalidUri(format!(
                "unknown cxf URI query parameter(s): {}",
                unknown_params.join(", ")
            )));
        }

        let wsdl_path = wsdl_path.ok_or_else(|| {
            camel_component_api::CamelError::InvalidUri(
                "cxf URI requires 'wsdl' query parameter".to_string(),
            )
        })?;
        let service_name = service_name.ok_or_else(|| {
            camel_component_api::CamelError::InvalidUri(
                "cxf URI requires 'service' query parameter".to_string(),
            )
        })?;
        let port_name = port_name.ok_or_else(|| {
            camel_component_api::CamelError::InvalidUri(
                "cxf URI requires 'port' query parameter".to_string(),
            )
        })?;

        let cfg = CxfEndpointConfig {
            address,
            wsdl_path,
            service_name,
            port_name,
            operation,
            profile,
            timeout_ms,
            mtom_enabled: mtom_enabled.unwrap_or(false),
            attachment_content_type,
        };
        cfg.validate()?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cxf_minimal() {
        let err = CxfEndpointConfig::from_uri("cxf://http://localhost:8080/service").unwrap_err();
        assert!(err.to_string().contains("requires 'wsdl'"));
    }

    #[test]
    fn parse_cxf_with_params() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://localhost:8080/service?wsdl=service.wsdl&service=MyService&port=MyPort&operation=doSomething",
        )
        .unwrap();
        assert_eq!(cfg.address, "http://localhost:8080/service");
        assert_eq!(cfg.wsdl_path, "service.wsdl".to_string());
        assert_eq!(cfg.service_name, "MyService".to_string());
        assert_eq!(cfg.port_name, "MyPort".to_string());
        assert_eq!(cfg.operation, Some("doSomething".to_string()));
        assert_eq!(cfg.timeout_ms, None);
    }

    #[test]
    fn parse_cxf_missing_service_param() {
        let err = CxfEndpointConfig::from_uri(
            "cxf://http://localhost:8080/service?wsdl=service.wsdl&port=MyPort",
        )
        .unwrap_err();
        assert!(err.to_string().contains("requires 'service'"));
    }

    #[test]
    fn parse_cxf_missing_port_param() {
        let err = CxfEndpointConfig::from_uri(
            "cxf://http://localhost:8080/service?wsdl=service.wsdl&service=MyService",
        )
        .unwrap_err();
        assert!(err.to_string().contains("requires 'port'"));
    }

    #[test]
    fn parse_cxf_wrong_scheme() {
        let err = CxfEndpointConfig::from_uri("http://localhost:8080/service").unwrap_err();
        assert!(err.to_string().contains("cxf://"));
    }

    #[test]
    fn parse_cxf_empty_address() {
        let err = CxfEndpointConfig::from_uri("cxf://").unwrap_err();
        assert!(err.to_string().contains("address"));
    }

    #[test]
    fn default_pool_config() {
        let cfg = CxfPoolConfig::default();
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.max_bridges, 4);
        assert!(cfg.profiles.is_empty());
        assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
        assert_eq!(cfg.health_check_interval_ms, 5_000);
        assert_eq!(cfg.version, BRIDGE_VERSION);
    }

    #[test]
    fn parse_pool_config_from_toml() {
        let toml_str = r#"
            max_bridges = 8
            bridge_start_timeout_ms = 60_000
            health_check_interval_ms = 10_000

            [[profiles]]
            name = "myservice"
            address = "http://localhost:9090/ws"
            wsdl_path = "service.wsdl"
            service_name = "MyService"
            port_name = "MyPort"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.max_bridges, 8);
        assert_eq!(cfg.bridge_start_timeout_ms, 60_000);
        assert_eq!(cfg.health_check_interval_ms, 10_000);
        assert_eq!(cfg.profiles.len(), 1);
        assert_eq!(cfg.profiles[0].name, "myservice");
        assert_eq!(
            cfg.profiles[0].address,
            Some("http://localhost:9090/ws".to_string())
        );
    }

    #[test]
    fn parse_pool_config_defaults() {
        let cfg: CxfPoolConfig = toml::from_str("").expect("empty toml");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.max_bridges, 4);
        assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
        assert_eq!(cfg.health_check_interval_ms, 5_000);
        assert!(cfg.profiles.is_empty());
        assert!(cfg.bridge_cache_dir.is_none());
    }

    #[test]
    fn parse_profile_config() {
        let toml_str = r#"
            [[profiles]]
            name = "baleares"
            address = "http://localhost:8080/ws"
            wsdl_path = "/etc/112/baleares/service.wsdl"
            service_name = "{urn:112}EmergencyService"
            port_name = "{urn:112}EmergencyPort"

            [profiles.security]
            keystore_path = "/etc/112/baleares/keystore.jks"
            sig_username = "baleares_cert"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        let p = &cfg.profiles[0];
        assert_eq!(p.name, "baleares");
        assert_eq!(p.wsdl_path, "/etc/112/baleares/service.wsdl");
        assert_eq!(
            p.security.keystore_path,
            Some("/etc/112/baleares/keystore.jks".to_string())
        );
        assert_eq!(p.security.sig_username, Some("baleares_cert".to_string()));
    }

    #[test]
    fn parse_multiple_profiles() {
        let toml_str = r#"
            [[profiles]]
            name = "baleares"
            wsdl_path = "/a.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            keystore_path = "/a.jks"
            keystore_password = "pass"

            [[profiles]]
            name = "extremadura"
            wsdl_path = "/b.wsdl"
            service_name = "Svc2"
            port_name = "Port2"

            [profiles.security]
            keystore_path = "/b.jks"
            truststore_path = "/b.ts"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        assert_eq!(cfg.profiles.len(), 2);
        assert_eq!(cfg.profiles[0].name, "baleares");
        assert_eq!(cfg.profiles[1].name, "extremadura");
        assert_eq!(
            cfg.profiles[1].security.truststore_path,
            Some("/b.ts".to_string())
        );
    }

    #[test]
    fn parse_pool_config_with_profiles() {
        let toml_str = r#"
            max_bridges = 2

            [[profiles]]
            name = "test"
            wsdl_path = "/wsdl/hello.wsdl"
            service_name = "HelloService"
            port_name = "HelloPort"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.max_bridges, 2);
        assert_eq!(cfg.profiles.len(), 1);
        assert_eq!(cfg.profiles[0].name, "test");
    }

    #[test]
    fn profile_name_validation_rejects_hyphens() {
        assert!(validate_profile_name("my-profile").is_err());
    }

    #[test]
    fn profile_name_validation_rejects_uppercase() {
        assert!(validate_profile_name("MyProfile").is_err());
    }

    #[test]
    fn profile_name_validation_rejects_spaces() {
        assert!(validate_profile_name("my profile").is_err());
    }

    #[test]
    fn profile_name_validation_rejects_empty() {
        assert!(validate_profile_name("").is_err());
    }

    #[test]
    fn profile_name_validation_accepts_valid() {
        assert!(validate_profile_name("baleares").is_ok());
        assert!(validate_profile_name("my_profile").is_ok());
        assert!(validate_profile_name("profile123").is_ok());
    }

    #[test]
    fn endpoint_config_parses_profile_param() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://host:8080/service?wsdl=file.wsdl&service=Svc&port=Port&profile=baleares",
        )
        .unwrap();
        assert_eq!(cfg.profile, Some("baleares".to_string()));
    }

    #[test]
    fn endpoint_config_profile_default_none() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://host:8080/service?wsdl=file.wsdl&service=Svc&port=Port",
        )
        .unwrap();
        assert!(cfg.profile.is_none());
    }

    #[test]
    fn endpoint_config_from_uri_full() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://example.com/ws?wsdl=file.wsdl&service=MySvc&port=MyPort&operation=doWork",
        )
        .unwrap();
        assert_eq!(cfg.address, "http://example.com/ws");
        assert_eq!(cfg.wsdl_path, "file.wsdl".to_string());
        assert_eq!(cfg.service_name, "MySvc".to_string());
        assert_eq!(cfg.port_name, "MyPort".to_string());
        assert_eq!(cfg.operation, Some("doWork".to_string()));
        assert_eq!(cfg.timeout_ms, None);
    }

    #[test]
    fn endpoint_config_from_uri_minimal() {
        let err = CxfEndpointConfig::from_uri("cxf://http://example.com/ws").unwrap_err();
        assert!(err.to_string().contains("requires 'wsdl'"));
    }

    #[test]
    fn test_rejects_zero_timeout() {
        let mut cfg = CxfEndpointConfig {
            address: "http://example.com/ws".to_string(),
            wsdl_path: "file.wsdl".to_string(),
            service_name: "svc".to_string(),
            port_name: "port".to_string(),
            operation: None,
            profile: None,
            timeout_ms: None,
            mtom_enabled: false,
            attachment_content_type: None,
        };
        cfg.timeout_ms = Some(0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_json_negative_timeout_fails_deserialization() {
        #[derive(serde::Deserialize)]
        #[expect(dead_code)]
        struct TimeoutWrapper {
            timeout_ms: u64,
        }

        let result = serde_json::from_str::<TimeoutWrapper>(r#"{"timeout_ms": -1}"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_empty_required_field() {
        let mut cfg = CxfEndpointConfig {
            address: "http://example.com/ws".to_string(),
            wsdl_path: "file.wsdl".to_string(),
            service_name: "svc".to_string(),
            port_name: "port".to_string(),
            operation: None,
            profile: None,
            timeout_ms: None,
            mtom_enabled: false,
            attachment_content_type: None,
        };
        cfg.address = "".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn endpoint_config_rejects_unknown_query_param() {
        let err = CxfEndpointConfig::from_uri(
            "cxf://http://example.com/ws?wsdl=file.wsdl&service=Svc&port=Port&bogus=1",
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown cxf URI query parameter"));
    }

    #[test]
    fn endpoint_config_parses_timeout_ms() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://example.com/ws?wsdl=file.wsdl&service=Svc&port=Port&timeout_ms=5000",
        )
        .unwrap();
        assert_eq!(cfg.timeout_ms, Some(5000));
    }

    #[test]
    fn profile_config_env_prefix() {
        let profile = CxfProfileConfig {
            name: "baleares".to_string(),
            address: None,
            wsdl_path: "/wsdl/test.wsdl".to_string(),
            service_name: "Svc".to_string(),
            port_name: "Port".to_string(),
            security: Default::default(),
        };
        assert_eq!(profile.env_prefix(), "CXF_PROFILE_BALEARES_");
    }

    #[test]
    fn pool_config_validate_rejects_zero_max_bridges() {
        let err = toml::from_str::<CxfPoolConfig>("max_bridges = 0").unwrap_err();
        assert!(err.to_string().contains("max_bridges"));
    }

    #[test]
    fn pool_config_validate_rejects_zero_bridge_start_timeout() {
        let err = toml::from_str::<CxfPoolConfig>("bridge_start_timeout_ms = 0").unwrap_err();
        assert!(err.to_string().contains("bridge_start_timeout_ms"));
    }

    #[test]
    fn pool_config_validate_rejects_zero_health_check_interval() {
        let err = toml::from_str::<CxfPoolConfig>("health_check_interval_ms = 0").unwrap_err();
        assert!(err.to_string().contains("health_check_interval_ms"));
    }

    // ── Consumer bind address scheme gate (bridge-lockstep-hardening 1.2) ──

    #[test]
    fn consumer_address_https_rejected() {
        let cfg = CxfPoolConfig {
            bind_address: Some("https://0.0.0.0:9000/soap".to_string()),
            listener_tls: None,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("https bind_address requires key material"),
            "got: {err}"
        );
    }

    #[test]
    fn consumer_address_http_accepted() {
        let cfg = CxfPoolConfig {
            bind_address: Some("http://localhost:9000/soap".to_string()),
            listener_tls: None,
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn consumer_address_missing_scheme_rejected() {
        assert!(validate_consumer_address("0.0.0.0:9000/soap").is_err());
    }

    #[test]
    fn consumer_address_uppercase_http_accepted() {
        assert!(validate_consumer_address("HTTP://localhost:9000/soap").is_ok());
    }

    #[test]
    fn pool_config_validate_rejects_https_bind_address() {
        let toml_str = r#"bind_address = "https://0.0.0.0:9000/soap""#;
        let err = toml::from_str::<CxfPoolConfig>(toml_str).unwrap_err();
        assert!(
            err.to_string()
                .contains("https bind_address requires key material"),
            "got: {err}"
        );
    }

    #[test]
    fn endpoint_config_mtom_disabled_by_default() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://example.com/ws?wsdl=file.wsdl&service=Svc&port=Port",
        )
        .unwrap();
        assert!(!cfg.mtom_enabled);
        assert!(cfg.attachment_content_type.is_none());
    }

    #[test]
    fn endpoint_config_parses_mtom_enabled() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://example.com/ws?wsdl=file.wsdl&service=Svc&port=Port&mtom_enabled=true",
        )
        .unwrap();
        assert!(cfg.mtom_enabled);
    }

    // ── Reconnect policy (rc-eyn) ──────────────────────────────────────────

    #[test]
    fn cxf_pool_config_default_reconnect_is_unlimited() {
        let cfg = CxfPoolConfig::default();
        assert!(cfg.reconnect.enabled);
        assert_eq!(
            cfg.reconnect.max_attempts, 0,
            "default must be unlimited (max_attempts=0) to preserve previous infinite-retry behavior"
        );
    }

    #[test]
    fn cxf_pool_config_has_reconnect_policy() {
        let toml_str = r#"
            version = "0.1.0"
            max_bridges = 5
            bridge_start_timeout_ms = 30000
            health_check_interval_ms = 5000
            [[profiles]]
            name = "test"
            wsdl_path = "test.wsdl"
            service_name = "Svc"
            port_name = "Port"
            [profiles.security]
            [reconnect]
            max_attempts = 5
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.reconnect.max_attempts, 5);
    }

    #[test]
    fn cxf_pool_config_toml_empty_reconnect_defaults_to_unlimited() {
        let cfg: CxfPoolConfig = toml::from_str("").expect("empty toml");
        assert!(cfg.reconnect.enabled);
        assert_eq!(cfg.reconnect.max_attempts, 0);
    }

    // ── Credential redaction (rc-ryl0) ──────────────────────────────────────

    #[test]
    fn cxf_security_fields_debug_redacts_passwords() {
        let fields = CxfSecurityFields {
            keystore_password: Some("SENTINEL-KEYSTORE-PASS".into()),
            truststore_password: Some("SENTINEL-TRUSTSTORE-PASS".into()),
            sig_password: Some("SENTINEL-SIG-PASS".into()),
            tls: Some(CxfTlsConfig {
                keystore_password: Some("SENTINEL-TLS-KS-PASS".into()),
                truststore_password: Some("SENTINEL-TLS-TS-PASS".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let debug = format!("{:?}", fields);
        assert!(!debug.contains("SENTINEL-KEYSTORE-PASS"));
        assert!(!debug.contains("SENTINEL-TRUSTSTORE-PASS"));
        assert!(!debug.contains("SENTINEL-SIG-PASS"));
        assert!(!debug.contains("SENTINEL-TLS-KS-PASS"));
        assert!(!debug.contains("SENTINEL-TLS-TS-PASS"));
    }

    // ── Producer TLS config plumbing (Task 1.5) ─────────────────────────────

    fn profile_with_tls(name: &str, tls: CxfTlsConfig) -> CxfProfileConfig {
        CxfProfileConfig {
            name: name.to_string(),
            address: None,
            wsdl_path: "/wsdl/test.wsdl".to_string(),
            service_name: "Svc".to_string(),
            port_name: "Port".to_string(),
            security: CxfSecurityFields {
                tls: Some(tls),
                ..Default::default()
            },
        }
    }

    #[test]
    fn tls_path_without_password_fails_validation() {
        let cfg = CxfPoolConfig {
            profiles: vec![profile_with_tls(
                "baleares",
                CxfTlsConfig {
                    keystore_path: Some("/etc/cxf/ks.p12".to_string()),
                    ..Default::default()
                },
            )],
            listener_tls: None,
            ..Default::default()
        };
        match cfg.validate().unwrap_err() {
            camel_component_api::CamelError::Config(msg) => {
                assert!(msg.contains("keystore_password"), "got: {msg}");
            }
            other => panic!("expected CamelError::Config, got {other:?}"),
        }
    }

    #[test]
    fn tls_password_without_path_fails_validation() {
        let cfg = CxfPoolConfig {
            profiles: vec![profile_with_tls(
                "baleares",
                CxfTlsConfig {
                    truststore_password: Some("pw".to_string()),
                    ..Default::default()
                },
            )],
            listener_tls: None,
            ..Default::default()
        };
        match cfg.validate().unwrap_err() {
            camel_component_api::CamelError::Config(msg) => {
                assert!(msg.contains("truststore_path"), "got: {msg}");
            }
            other => panic!("expected CamelError::Config, got {other:?}"),
        }
    }

    #[test]
    fn keystore_type_defaults_to_pkcs12() {
        let env = CxfTlsConfig {
            keystore_path: Some("/etc/cxf/ks.p12".to_string()),
            keystore_password: Some("pw".to_string()),
            ..Default::default()
        }
        .to_env()
        .to_env_vars("CXF_PROFILE_TEST_");
        let entry = env
            .iter()
            .find(|(k, _)| k.as_str() == "CXF_PROFILE_TEST_TLS_KEYSTORE_TYPE")
            .expect("TLS_KEYSTORE_TYPE emitted");
        assert_eq!(entry.1, "PKCS12");
    }

    #[test]
    fn keystore_type_rejects_unknown_spelling() {
        let toml_str = r#"
            [[profiles]]
            name = "baleares"
            wsdl_path = "/wsdl/test.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security.tls]
            keystore_path = "/etc/cxf/ks.jks"
            keystore_password = "pw"
            keystore_type = "jks"
        "#;
        assert!(toml::from_str::<CxfPoolConfig>(toml_str).is_err());
    }

    // ── Blank TLS-value rejection (N4 fail-closed parity) ──────────────────

    /// Explicitly supplied values both layers must reject: empty, ASCII
    /// whitespace, and Unicode `White_Space` members where Java's
    /// `String.isBlank` (`Character.isWhitespace`) disagrees with Rust's
    /// `char::is_whitespace` (NBSP U+00A0, NEL U+0085, mixed ASCII + U+202F).
    const BLANK_TLS_VALUES: [&str; 7] =
        ["", " ", "\t", " \t\r\n", "\u{00A0}", "\u{85}", " \u{202F}"];

    /// Blanks whose echo would leak distinctive bytes; plain ASCII whitespace
    /// occurs in any message text, so the non-echo check targets these only.
    const ECHO_SENSITIVE_BLANKS: [&str; 3] = ["\u{00A0}", "\u{85}", " \u{202F}"];

    #[test]
    fn producer_tls_rejects_blank_store_values() {
        // Blank keystore path beside an otherwise valid paired password.
        for blank in BLANK_TLS_VALUES {
            let cfg = CxfPoolConfig {
                profiles: vec![profile_with_tls(
                    "baleares",
                    CxfTlsConfig {
                        keystore_path: Some(blank.to_string()),
                        keystore_password: Some("pw".to_string()),
                        ..Default::default()
                    },
                )],
                listener_tls: None,
                ..Default::default()
            };
            match cfg.validate().unwrap_err() {
                camel_component_api::CamelError::Config(msg) => {
                    assert!(msg.contains("keystore_path"), "got: {msg}");
                    assert!(msg.contains("baleares"), "got: {msg}");
                    if ECHO_SENSITIVE_BLANKS.contains(&blank) {
                        assert!(!msg.contains(blank), "error echoed the value: {msg}");
                    }
                }
                other => panic!("expected CamelError::Config, got {other:?}"),
            }
        }

        // Blank truststore password beside an otherwise valid paired path.
        for blank in BLANK_TLS_VALUES {
            let cfg = CxfPoolConfig {
                profiles: vec![profile_with_tls(
                    "baleares",
                    CxfTlsConfig {
                        truststore_path: Some("/etc/cxf/ts.p12".to_string()),
                        truststore_password: Some(blank.to_string()),
                        ..Default::default()
                    },
                )],
                listener_tls: None,
                ..Default::default()
            };
            match cfg.validate().unwrap_err() {
                camel_component_api::CamelError::Config(msg) => {
                    assert!(msg.contains("truststore_password"), "got: {msg}");
                    assert!(msg.contains("baleares"), "got: {msg}");
                    if ECHO_SENSITIVE_BLANKS.contains(&blank) {
                        assert!(!msg.contains(blank), "error echoed the value: {msg}");
                    }
                }
                other => panic!("expected CamelError::Config, got {other:?}"),
            }
        }

        // A both-blank pair previously slipped through as a "complete" pair
        // and silently selected JVM store defaults on the Java side.
        for blank in BLANK_TLS_VALUES {
            let cfg = CxfPoolConfig {
                profiles: vec![profile_with_tls(
                    "baleares",
                    CxfTlsConfig {
                        keystore_path: Some(blank.to_string()),
                        keystore_password: Some(blank.to_string()),
                        ..Default::default()
                    },
                )],
                listener_tls: None,
                ..Default::default()
            };
            match cfg.validate().unwrap_err() {
                camel_component_api::CamelError::Config(msg) => {
                    assert!(msg.contains("keystore_path"), "got: {msg}");
                }
                other => panic!("expected CamelError::Config, got {other:?}"),
            }
        }

        // Genuinely omitted pairs stay valid; nonblank passwords are never
        // trimmed (meaningful surrounding spaces survive to the env var).
        let cfg = CxfPoolConfig {
            profiles: vec![profile_with_tls("baleares", CxfTlsConfig::default())],
            listener_tls: None,
            ..Default::default()
        };
        assert!(cfg.validate().is_ok(), "omitted TLS pairs must stay valid");
        let tls = CxfTlsConfig {
            keystore_path: Some("/etc/cxf/ks.p12".to_string()),
            keystore_password: Some(" pass word ".to_string()),
            ..Default::default()
        };
        assert!(
            CxfPoolConfig {
                profiles: vec![profile_with_tls("baleares", tls.clone())],
                listener_tls: None,
                ..Default::default()
            }
            .validate()
            .is_ok(),
            "nonblank password with meaningful spaces must stay valid"
        );
        assert_eq!(
            tls.to_env().keystore_password.map(Redacted::into_inner),
            Some(" pass word ".to_string()),
            "nonblank password bytes must be forwarded untouched"
        );
    }

    // ── Fault-status override (Task 2.2) ────────────────────────────────────

    fn profile_with_fault_status(name: &str, fault_status: u16) -> CxfProfileConfig {
        CxfProfileConfig {
            name: name.to_string(),
            address: None,
            wsdl_path: "/wsdl/test.wsdl".to_string(),
            service_name: "Svc".to_string(),
            port_name: "Port".to_string(),
            security: CxfSecurityFields {
                fault_status: Some(fault_status),
                ..Default::default()
            },
        }
    }

    #[test]
    fn fault_status_out_of_range_rejected() {
        for out_of_range in [101u16, 204, 399, 600] {
            let cfg = CxfPoolConfig {
                profiles: vec![profile_with_fault_status("esap", out_of_range)],
                listener_tls: None,
                ..Default::default()
            };
            let err = cfg.validate().unwrap_err();
            match err {
                camel_component_api::CamelError::Config(msg) => {
                    assert!(
                        msg.contains("fault_status"),
                        "error must name fault_status, got: {msg}"
                    );
                }
                other => panic!("expected CamelError::Config, got {other:?}"),
            }
        }
    }

    #[test]
    fn fault_status_in_range_accepted() {
        for in_range in [400u16, 403, 500, 599] {
            let cfg = CxfPoolConfig {
                profiles: vec![profile_with_fault_status("esap", in_range)],
                listener_tls: None,
                ..Default::default()
            };
            assert!(
                cfg.validate().is_ok(),
                "fault_status {in_range} must be accepted"
            );
        }
    }

    // ── ALLOWED_* inbound algorithm allow-lists (Task 2.1) ─────────────────

    const LEGACY_SIG_URI: &str = "http://www.w3.org/2000/09/xmldsig#rsa-sha1";
    const MODERN_SIG_URI: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";

    fn profile_with_security(name: &str, security: CxfSecurityFields) -> CxfProfileConfig {
        CxfProfileConfig {
            name: name.to_string(),
            address: None,
            wsdl_path: "/wsdl/test.wsdl".to_string(),
            service_name: "Svc".to_string(),
            port_name: "Port".to_string(),
            security,
        }
    }

    #[test]
    fn allowed_signature_algorithms_parses_and_emits() {
        let toml_str = format!(
            r#"
            [[profiles]]
            name = "legacy"
            wsdl_path = "/wsdl/test.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            truststore_path = "/etc/cxf/truststore.jks"
            allowed_signature_algorithms = "{LEGACY_SIG_URI},{MODERN_SIG_URI}"
        "#
        );
        let cfg: CxfPoolConfig = toml::from_str(&toml_str).expect("valid toml");
        assert_eq!(
            cfg.profiles[0]
                .security
                .allowed_signature_algorithms
                .as_deref(),
            Some(format!("{LEGACY_SIG_URI},{MODERN_SIG_URI}").as_str())
        );
        let debug = format!("{:?}", cfg.profiles[0].security);
        assert!(
            debug.contains(LEGACY_SIG_URI),
            "Debug must not redact algorithm URIs: {debug}"
        );
        assert!(
            debug.contains(MODERN_SIG_URI),
            "Debug must not redact algorithm URIs: {debug}"
        );
    }

    #[test]
    fn allowed_algorithms_exempt_from_signing_precondition() {
        let security = CxfSecurityFields {
            truststore_path: Some("/etc/cxf/truststore.jks".to_string()),
            allowed_signature_algorithms: Some(format!("{LEGACY_SIG_URI},{MODERN_SIG_URI}")),
            allowed_digest_algorithms: Some("http://www.w3.org/2001/04/xmlenc#sha256".to_string()),
            ..Default::default()
        };
        assert!(
            security.keystore_path.is_none(),
            "verify-only profile must not need a signing keystore"
        );
        assert!(
            security.security_actions_out.is_none(),
            "verify-only profile must not need outbound actions"
        );
        let cfg = CxfPoolConfig {
            profiles: vec![profile_with_security("legacy", security)],
            ..Default::default()
        };
        assert!(
            cfg.validate().is_ok(),
            "ALLOWED_* options are exempt from signing preconditions"
        );
    }

    #[test]
    fn allowed_algorithms_without_verification_material_rejected() {
        for (key, security) in [
            (
                "allowed_signature_algorithms",
                CxfSecurityFields {
                    allowed_signature_algorithms: Some(LEGACY_SIG_URI.to_string()),
                    ..Default::default()
                },
            ),
            (
                "allowed_digest_algorithms",
                CxfSecurityFields {
                    allowed_digest_algorithms: Some(
                        "http://www.w3.org/2000/09/xmldsig#sha1".to_string(),
                    ),
                    ..Default::default()
                },
            ),
        ] {
            let cfg = CxfPoolConfig {
                profiles: vec![profile_with_security("legacy", security)],
                ..Default::default()
            };
            match cfg.validate().unwrap_err() {
                camel_component_api::CamelError::Config(msg) => {
                    assert!(msg.contains(key), "error must name {key}, got: {msg}");
                    assert!(msg.contains("verification material"), "got: {msg}");
                }
                other => panic!("expected CamelError::Config, got {other:?}"),
            }
        }

        // Producer transport TLS stores are NOT inbound verification material.
        let cfg = CxfPoolConfig {
            profiles: vec![profile_with_security(
                "legacy",
                CxfSecurityFields {
                    allowed_signature_algorithms: Some(LEGACY_SIG_URI.to_string()),
                    tls: Some(CxfTlsConfig {
                        keystore_path: Some("/etc/cxf/ks.p12".to_string()),
                        keystore_password: Some("pw".to_string()),
                        truststore_path: Some("/etc/cxf/ts.p12".to_string()),
                        truststore_password: Some("pw".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )],
            ..Default::default()
        };
        match cfg.validate().unwrap_err() {
            camel_component_api::CamelError::Config(msg) => {
                assert!(
                    msg.contains("allowed_signature_algorithms"),
                    "error must name the option, got: {msg}"
                );
            }
            other => panic!("expected CamelError::Config, got {other:?}"),
        }
    }

    #[test]
    fn allowed_algorithms_empty_rejected() {
        let toml_str = r#"
            [[profiles]]
            name = "legacy"
            wsdl_path = "/wsdl/test.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            truststore_path = "/etc/cxf/truststore.jks"
            allowed_signature_algorithms = ""
        "#;
        let err = toml::from_str::<CxfPoolConfig>(toml_str).unwrap_err();
        assert!(
            err.to_string().contains("allowed_signature_algorithms"),
            "got: {err}"
        );
    }

    #[test]
    fn allowed_algorithms_blank_segment_rejected() {
        let toml_str = r#"
            [[profiles]]
            name = "legacy"
            wsdl_path = "/wsdl/test.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            keystore_path = "/etc/cxf/keystore.jks"
            allowed_signature_algorithms = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256,,http://www.w3.org/2001/04/xmldsig-more#rsa-sha512"
        "#;
        let err = toml::from_str::<CxfPoolConfig>(toml_str).unwrap_err();
        assert!(
            err.to_string().contains("allowed_signature_algorithms"),
            "got: {err}"
        );
        assert!(err.to_string().contains("blank"), "got: {err}");
    }

    #[test]
    fn allowed_algorithms_non_uri_rejected() {
        let toml_str = r#"
            [[profiles]]
            name = "legacy"
            wsdl_path = "/wsdl/test.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            truststore_path = "/etc/cxf/truststore.jks"
            allowed_signature_algorithms = "rsa-sha1"
        "#;
        let err = toml::from_str::<CxfPoolConfig>(toml_str).unwrap_err();
        assert!(
            err.to_string().contains("allowed_signature_algorithms"),
            "got: {err}"
        );
        assert!(err.to_string().contains("absolute URI"), "got: {err}");
    }

    // ── Expired pinned-certificate opt-in (Task 4) ─────────────────────────

    #[test]
    fn allow_expired_pinned_certificate_defaults_false() {
        let toml_str = r#"
            [[profiles]]
            name = "partner"
            wsdl_path = "/wsdl/partner.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            truststore_path = "/etc/cxf/truststore.jks"
            truststore_password = "changeit"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        assert!(
            !cfg.profiles[0].security.allow_expired_pinned_certificate,
            "the opt-in must default to false"
        );
    }

    #[test]
    fn allow_expired_pinned_certificate_parses_true() {
        let toml_str = r#"
            [[profiles]]
            name = "partner"
            wsdl_path = "/wsdl/partner.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            truststore_path = "/etc/cxf/truststore.jks"
            truststore_password = "changeit"
            allow_expired_pinned_certificate = true
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        let security = &cfg.profiles[0].security;
        assert!(security.allow_expired_pinned_certificate);
        let debug = format!("{security:?}");
        assert!(
            debug.contains("allow_expired_pinned_certificate: true"),
            "boolean opt-in must not be redacted in Debug: {debug}"
        );
    }

    // ── Timestamp freshness window ──

    #[test]
    fn timestamp_window_defaults_unset() {
        let toml_str = r#"
            [[profiles]]
            name = "partner"
            wsdl_path = "/wsdl/partner.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            truststore_path = "/etc/cxf/truststore.jks"
            truststore_password = "changeit"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        let security = &cfg.profiles[0].security;
        assert_eq!(security.timestamp_ttl_seconds, None);
        assert_eq!(security.timestamp_future_tolerance_seconds, None);
        cfg.validate().expect("defaults must validate");
    }

    #[test]
    fn timestamp_window_parses_from_toml() {
        let toml_str = r#"
            [[profiles]]
            name = "partner"
            wsdl_path = "/wsdl/partner.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            truststore_path = "/etc/cxf/truststore.jks"
            truststore_password = "changeit"
            timestamp_ttl_seconds = 900
            timestamp_future_tolerance_seconds = 120
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        let security = &cfg.profiles[0].security;
        assert_eq!(security.timestamp_ttl_seconds, Some(900));
        assert_eq!(security.timestamp_future_tolerance_seconds, Some(120));
        cfg.validate().expect("positive windows validate");
        let debug = format!("{security:?}");
        assert!(
            debug.contains("timestamp_ttl_seconds: Some(900)"),
            "timestamp window must not be redacted in Debug: {debug}"
        );
    }

    #[test]
    fn timestamp_window_zero_rejected() {
        // Pool deserialization validates, so an invalid TOML value never reaches
        // `validate()`; build the config directly to assert the check itself.
        for key in [
            "timestamp_ttl_seconds",
            "timestamp_future_tolerance_seconds",
        ] {
            let mut security = CxfSecurityFields {
                truststore_path: Some("/etc/cxf/truststore.jks".to_string()),
                truststore_password: Some("changeit".to_string()),
                ..Default::default()
            };
            match key {
                "timestamp_ttl_seconds" => security.timestamp_ttl_seconds = Some(0),
                _ => security.timestamp_future_tolerance_seconds = Some(0),
            }
            let cfg = CxfPoolConfig {
                profiles: vec![profile_with_security("partner", security)],
                ..Default::default()
            };
            match cfg.validate().unwrap_err() {
                camel_component_api::CamelError::Config(msg) => {
                    assert!(msg.contains("'partner'"), "got: {msg}");
                    assert!(msg.contains(&format!("security.{key}")), "got: {msg}");
                    assert!(msg.contains("positive"), "got: {msg}");
                }
                other => panic!("expected CamelError::Config, got {other:?}"),
            }
        }
    }

    #[test]
    fn timestamp_window_zero_rejected_at_toml_load() {
        for key in [
            "timestamp_ttl_seconds",
            "timestamp_future_tolerance_seconds",
        ] {
            let toml_str = format!(
                r#"
                [[profiles]]
                name = "partner"
                wsdl_path = "/wsdl/partner.wsdl"
                service_name = "Svc"
                port_name = "Port"

                [profiles.security]
                truststore_path = "/etc/cxf/truststore.jks"
                truststore_password = "changeit"
                {key} = 0
                "#
            );
            let err = toml::from_str::<CxfPoolConfig>(&toml_str).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains(&format!("security.{key}")), "got: {msg}");
            assert!(msg.contains("positive"), "got: {msg}");
        }
    }

    #[test]
    fn timestamp_window_int_max_accepted() {
        let toml_str = r#"
            [[profiles]]
            name = "partner"
            wsdl_path = "/wsdl/partner.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            truststore_path = "/etc/cxf/truststore.jks"
            truststore_password = "changeit"
            timestamp_ttl_seconds = 2147483647
            timestamp_future_tolerance_seconds = 2147483647
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        let security = &cfg.profiles[0].security;
        assert_eq!(security.timestamp_ttl_seconds, Some(2147483647));
        assert_eq!(
            security.timestamp_future_tolerance_seconds,
            Some(2147483647)
        );
        cfg.validate().expect("int max windows validate");
    }

    #[test]
    fn timestamp_window_above_int_max_rejected() {
        // Values above the WSS4J `int` limit are rejected, never clamped
        // (owner ruling 2026-10-21).
        for key in [
            "timestamp_ttl_seconds",
            "timestamp_future_tolerance_seconds",
        ] {
            for value in [2147483648_u64, u64::MAX] {
                let mut security = CxfSecurityFields {
                    truststore_path: Some("/etc/cxf/truststore.jks".to_string()),
                    truststore_password: Some("changeit".to_string()),
                    ..Default::default()
                };
                match key {
                    "timestamp_ttl_seconds" => security.timestamp_ttl_seconds = Some(value),
                    _ => security.timestamp_future_tolerance_seconds = Some(value),
                }
                let cfg = CxfPoolConfig {
                    profiles: vec![profile_with_security("partner", security)],
                    ..Default::default()
                };
                match cfg.validate().unwrap_err() {
                    camel_component_api::CamelError::Config(msg) => {
                        assert!(msg.contains(&format!("security.{key}")), "got: {msg}");
                        assert!(msg.contains("2147483647"), "got: {msg}");
                    }
                    other => panic!("expected CamelError::Config, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn timestamp_window_without_material_rejected() {
        // Pool deserialization validates, so an invalid TOML value never reaches
        // `validate()`; build the config directly to assert the check itself.
        for key in [
            "timestamp_ttl_seconds",
            "timestamp_future_tolerance_seconds",
        ] {
            let mut security = CxfSecurityFields::default();
            match key {
                "timestamp_ttl_seconds" => security.timestamp_ttl_seconds = Some(900),
                _ => security.timestamp_future_tolerance_seconds = Some(900),
            }
            let cfg = CxfPoolConfig {
                profiles: vec![profile_with_security("partner", security)],
                ..Default::default()
            };
            match cfg.validate().unwrap_err() {
                camel_component_api::CamelError::Config(msg) => {
                    assert!(
                        msg.contains("no inbound verification material"),
                        "got: {msg}"
                    );
                }
                other => panic!("expected CamelError::Config, got {other:?}"),
            }
        }
    }

    #[test]
    fn timestamp_window_blank_material_rejected() {
        // An empty or whitespace-only store path is not verification material:
        // the Java bridge uses `hasText`, so a blank path must reject the window.
        for key in [
            "timestamp_ttl_seconds",
            "timestamp_future_tolerance_seconds",
        ] {
            for blank in ["", "   "] {
                let mut security = CxfSecurityFields {
                    truststore_path: Some(blank.to_string()),
                    truststore_password: Some("changeit".to_string()),
                    ..Default::default()
                };
                match key {
                    "timestamp_ttl_seconds" => security.timestamp_ttl_seconds = Some(900),
                    _ => security.timestamp_future_tolerance_seconds = Some(900),
                }
                let cfg = CxfPoolConfig {
                    profiles: vec![profile_with_security("partner", security)],
                    ..Default::default()
                };
                match cfg.validate().unwrap_err() {
                    camel_component_api::CamelError::Config(msg) => {
                        assert!(
                            msg.contains("no inbound verification material"),
                            "key {key}, blank {blank:?}: got: {msg}"
                        );
                    }
                    other => panic!("expected CamelError::Config, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn timestamp_window_without_material_rejected_at_toml_load() {
        for key in [
            "timestamp_ttl_seconds",
            "timestamp_future_tolerance_seconds",
        ] {
            let toml_str = format!(
                r#"
                [[profiles]]
                name = "partner"
                wsdl_path = "/wsdl/partner.wsdl"
                service_name = "Svc"
                port_name = "Port"

                [profiles.security]
                {key} = 900
                "#
            );
            let err = toml::from_str::<CxfPoolConfig>(&toml_str).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("no inbound verification material"),
                "got: {msg}"
            );
        }
    }

    // ── Listener TLS pool shape (Task 2.6) ─────────────────────────────────

    /// Assert a config-validation rejection is a `CamelError::Config` whose
    /// message names the failing key material.
    fn assert_config_err(cfg: &CxfPoolConfig, needle: &str) {
        match cfg.validate().unwrap_err() {
            camel_component_api::CamelError::Config(msg) => {
                assert!(msg.contains(needle), "expected {needle:?} in: {msg}");
            }
            other => panic!("expected CamelError::Config, got {other:?}"),
        }
    }

    fn listener_tls(
        cert_path: Option<&str>,
        key_path: Option<&str>,
        client_ca_path: Option<&str>,
        pkcs12_path: Option<&str>,
        pkcs12_password: Option<&str>,
    ) -> CxfListenerTls {
        CxfListenerTls {
            cert_path: cert_path.map(str::to_string),
            key_path: key_path.map(str::to_string),
            client_ca_path: client_ca_path.map(str::to_string),
            pkcs12_path: pkcs12_path.map(str::to_string),
            pkcs12_password: pkcs12_password.map(str::to_string),
        }
    }

    #[test]
    fn listener_tls_both_forms_rejected() {
        let cfg = CxfPoolConfig {
            bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
            listener_tls: Some(listener_tls(
                Some("/etc/cxf/server.pem"),
                Some("/etc/cxf/server.key"),
                Some("/etc/cxf/ca.pem"),
                Some("/etc/cxf/server.p12"),
                Some("s3cret"),
            )),
            ..Default::default()
        };
        assert_config_err(&cfg, "one key-material form");
    }

    #[test]
    fn listener_tls_neither_form_rejected_for_https() {
        let cfg = CxfPoolConfig {
            bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
            listener_tls: None,
            ..Default::default()
        };
        assert_config_err(&cfg, "soap_tls_");
    }

    #[test]
    fn listener_tls_missing_client_ca_rejected() {
        let pem_without_ca = CxfPoolConfig {
            bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
            listener_tls: Some(listener_tls(
                Some("/etc/cxf/server.pem"),
                Some("/etc/cxf/server.key"),
                None,
                None,
                None,
            )),
            ..Default::default()
        };
        assert_config_err(&pem_without_ca, "soap_tls_client_ca_path");

        let pkcs12_without_ca = CxfPoolConfig {
            bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
            listener_tls: Some(listener_tls(
                None,
                None,
                None,
                Some("/etc/cxf/server.p12"),
                Some("s3cret"),
            )),
            ..Default::default()
        };
        assert_config_err(&pkcs12_without_ca, "soap_tls_client_ca_path");
    }

    #[test]
    fn listener_tls_http_bind_rejected() {
        let cfg = CxfPoolConfig {
            bind_address: Some("http://0.0.0.0:9443/soap".to_string()),
            listener_tls: Some(listener_tls(
                Some("/etc/cxf/server.pem"),
                Some("/etc/cxf/server.key"),
                Some("/etc/cxf/ca.pem"),
                None,
                None,
            )),
            ..Default::default()
        };
        assert_config_err(&cfg, "https");
    }

    #[test]
    fn listener_tls_pkcs12_form_accepted() {
        // Fresh benign sentinel: the RAW config's own manual Debug must redact the
        // PKCS12 password (ADR-0051 covers config raw too). The raw assertion is
        // independent of the final record's formatter — neutralizing only the raw
        // redaction fails this test; the record formatter cannot mask it.
        const RAW_DEBUG_SENTINEL: &str = "raw-dbg-SENTINEL-7f3a";
        let toml_str = format!(
            r#"
            bind_address = "https://0.0.0.0:9443/soap"
            soap_tls_pkcs12_path = "/etc/cxf/server.p12"
            soap_tls_pkcs12_password = "{RAW_DEBUG_SENTINEL}"
            soap_tls_client_ca_path = "/etc/cxf/ca.pem"
        "#
        );

        // RAW, formatted BEFORE conversion.
        let raw: CxfPoolConfigRaw =
            toml::from_str(&toml_str).expect("valid raw listener TLS config");
        let raw_debug = format!("{raw:?}");
        assert!(
            !raw_debug.contains(RAW_DEBUG_SENTINEL),
            "raw Debug leaked the pkcs12 password: {raw_debug}"
        );
        assert!(
            raw_debug.contains("<redacted>"),
            "raw Debug must show <redacted>: {raw_debug}"
        );

        // FINAL record: PKCS12 correct fields accepted; record Debug formatted
        // separately also redacts (covered again in pool_env_test).
        let cfg = CxfPoolConfig::try_from(raw).expect("valid listener TLS config");
        assert!(
            cfg.validate().is_ok(),
            "PKCS12 form with client CA must validate"
        );
        let record_debug = format!("{:?}", cfg.listener_tls);
        assert!(
            !record_debug.contains(RAW_DEBUG_SENTINEL),
            "record Debug leaked the pkcs12 password: {record_debug}"
        );
    }

    #[test]
    fn listener_tls_cert_key_form_accepted() {
        let toml_str = r#"
            bind_address = "https://0.0.0.0:9443/soap"
            soap_tls_cert_path = "/etc/cxf/server.pem"
            soap_tls_key_path = "/etc/cxf/server.key"
            soap_tls_client_ca_path = "/etc/cxf/ca.pem"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid listener TLS config");
        assert!(
            cfg.validate().is_ok(),
            "cert/key form with client CA must validate"
        );
    }

    #[test]
    fn listener_tls_rejects_blank_material_values() {
        // Reuses the module-level parity table so producer and listener coverage
        // cannot drift apart.
        for blank in BLANK_TLS_VALUES {
            let cfg = CxfPoolConfig {
                bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
                listener_tls: Some(listener_tls(
                    Some(blank),
                    Some("/etc/cxf/server.key"),
                    Some("/etc/cxf/ca.pem"),
                    None,
                    None,
                )),
                ..Default::default()
            };
            assert_config_err(&cfg, "soap_tls_cert_path");
        }
        for blank in BLANK_TLS_VALUES {
            let cfg = CxfPoolConfig {
                bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
                listener_tls: Some(listener_tls(
                    Some("/etc/cxf/server.pem"),
                    Some(blank),
                    Some("/etc/cxf/ca.pem"),
                    None,
                    None,
                )),
                ..Default::default()
            };
            assert_config_err(&cfg, "soap_tls_key_path");
        }
        for blank in BLANK_TLS_VALUES {
            let cfg = CxfPoolConfig {
                bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
                listener_tls: Some(listener_tls(
                    Some("/etc/cxf/server.pem"),
                    Some("/etc/cxf/server.key"),
                    Some(blank),
                    None,
                    None,
                )),
                ..Default::default()
            };
            assert_config_err(&cfg, "soap_tls_client_ca_path");
        }

        // Each blank material key inside an otherwise-valid PKCS#12 shape.
        for blank in BLANK_TLS_VALUES {
            let cfg = CxfPoolConfig {
                bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
                listener_tls: Some(listener_tls(
                    None,
                    None,
                    Some("/etc/cxf/ca.pem"),
                    Some(blank),
                    Some("s3cret"),
                )),
                ..Default::default()
            };
            assert_config_err(&cfg, "soap_tls_pkcs12_path");
        }
        for blank in BLANK_TLS_VALUES {
            let cfg = CxfPoolConfig {
                bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
                listener_tls: Some(listener_tls(
                    None,
                    None,
                    Some("/etc/cxf/ca.pem"),
                    Some("/etc/cxf/server.p12"),
                    Some(blank),
                )),
                ..Default::default()
            };
            // Rejection must name the key and never echo the blank password.
            match cfg.validate().unwrap_err() {
                camel_component_api::CamelError::Config(msg) => {
                    assert!(msg.contains("soap_tls_pkcs12_password"), "got: {msg}");
                    if ECHO_SENSITIVE_BLANKS.contains(&blank) {
                        assert!(!msg.contains(blank), "error echoed the value: {msg}");
                    }
                }
                other => panic!("expected CamelError::Config, got {other:?}"),
            }
        }

        // Explicitly blank-only TLS material with no bind is rejected as a
        // blank value, not as a confusing "none is configured" shape error.
        for blank in BLANK_TLS_VALUES {
            let cfg = CxfPoolConfig {
                bind_address: None,
                listener_tls: Some(listener_tls(None, None, Some(blank), None, None)),
                ..Default::default()
            };
            assert_config_err(&cfg, "soap_tls_client_ca_path");
        }

        // A blank-complete PEM form over an https bind previously passed Rust
        // config load and only failed later at bridge startup.
        let cfg = CxfPoolConfig {
            bind_address: Some("https://0.0.0.0:9443/soap".to_string()),
            listener_tls: Some(listener_tls(Some(""), Some(""), Some(""), None, None)),
            ..Default::default()
        };
        assert_config_err(&cfg, "soap_tls_cert_path");

        // Omitted no-TLS/no-bind (producer-only pool) stays valid.
        let cfg = CxfPoolConfig::default();
        assert!(
            cfg.validate().is_ok(),
            "omitted no-TLS no-bind must be valid"
        );
    }
}
