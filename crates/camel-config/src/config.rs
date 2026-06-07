use crate::PropertiesResolver;
use camel_api::CamelError;
use camel_core::TracerConfig;
use config::{Config, ConfigError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct CamelConfig {
    #[serde(default)]
    pub routes: Vec<String>,

    /// Enable file-watcher hot-reload. Defaults to false.
    /// Can be overridden per profile in Camel.toml or via `--watch` / `--no-watch` CLI flags.
    // TODO(CONFIG-004): hot-reload watch plumbing not fully implemented yet.
    #[serde(default)]
    pub watch: bool,

    /// Optional redb runtime journal configuration.
    ///
    /// When unset, runtime state is ephemeral (in-memory only).
    #[serde(default)]
    pub runtime_journal: Option<JournalConfig>,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    #[serde(default = "default_drain_timeout_ms")]
    pub drain_timeout_ms: u64,

    #[serde(default = "default_watch_debounce_ms")]
    // TODO(CONFIG-004): Hot-reload via file watcher not yet implemented.
    // watch_debounce_ms is parsed but currently unused.
    pub watch_debounce_ms: u64,

    #[serde(default)]
    pub components: ComponentsConfig,

    #[serde(default)]
    pub observability: ObservabilityConfig,

    #[serde(default)]
    pub supervision: Option<SupervisionCamelConfig>,

    #[serde(default)]
    pub platform: PlatformCamelConfig,

    #[serde(default)]
    pub stream_caching: StreamCachingConfig,

    #[serde(default)]
    pub beans: HashMap<String, BeanConfig>,

    #[serde(default)]
    pub security: SecurityConfig,

    /// Catch-all for extra keys injected by config sources (e.g. CAMEL_* env vars)
    /// or unknown fields in TOML. Nested structs use `deny_unknown_fields` to
    /// catch typos in sections like `[observability.health]`.
    #[serde(flatten)]
    pub _extra: HashMap<String, toml::Value>,
}

#[derive(Debug, Default, Clone)]
pub struct CamelConfigBuilder {
    pub routes: Option<Vec<String>>,
    pub watch: Option<bool>,
    pub log_level: Option<String>,
    pub timeout_ms: Option<u64>,
    pub drain_timeout_ms: Option<u64>,
    pub watch_debounce_ms: Option<u64>,
}

impl CamelConfigBuilder {
    pub fn routes(mut self, v: Vec<String>) -> Self {
        self.routes = Some(v);
        self
    }

    pub fn watch(mut self, v: bool) -> Self {
        self.watch = Some(v);
        self
    }

    pub fn log_level(mut self, v: impl Into<String>) -> Self {
        self.log_level = Some(v.into());
        self
    }

    pub fn timeout_ms(mut self, v: u64) -> Self {
        self.timeout_ms = Some(v);
        self
    }

    pub fn drain_timeout_ms(mut self, v: u64) -> Self {
        self.drain_timeout_ms = Some(v);
        self
    }

    pub fn watch_debounce_ms(mut self, v: u64) -> Self {
        self.watch_debounce_ms = Some(v);
        self
    }

    pub fn build(self) -> CamelConfig {
        let defaults = CamelConfig::default();
        CamelConfig {
            routes: self.routes.unwrap_or(defaults.routes),
            watch: self.watch.unwrap_or(defaults.watch),
            runtime_journal: defaults.runtime_journal,
            log_level: self.log_level.unwrap_or(defaults.log_level),
            timeout_ms: self.timeout_ms.unwrap_or(defaults.timeout_ms),
            drain_timeout_ms: self.drain_timeout_ms.unwrap_or(defaults.drain_timeout_ms),
            watch_debounce_ms: self.watch_debounce_ms.unwrap_or(defaults.watch_debounce_ms),
            components: defaults.components,
            observability: defaults.observability,
            supervision: defaults.supervision,
            platform: defaults.platform,
            stream_caching: defaults.stream_caching,
            beans: defaults.beans,
            security: defaults.security,
            _extra: defaults._extra,
        }
    }
}

impl Default for CamelConfig {
    fn default() -> Self {
        Self {
            routes: Vec::new(),
            watch: false,
            runtime_journal: None,
            log_level: default_log_level(),
            timeout_ms: default_timeout_ms(),
            drain_timeout_ms: default_drain_timeout_ms(),
            watch_debounce_ms: default_watch_debounce_ms(),
            components: ComponentsConfig::default(),
            observability: ObservabilityConfig::default(),
            supervision: None,
            platform: PlatformCamelConfig::default(),
            stream_caching: StreamCachingConfig::default(),
            beans: HashMap::new(),
            security: SecurityConfig::default(),
            _extra: HashMap::new(),
        }
    }
}

/// Platform selection for leader election, readiness, and identity.
///
/// `[platform]` in Camel.toml. Defaults to noop (always leader, always ready).
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlatformCamelConfig {
    #[default]
    Noop,
    Kubernetes(KubernetesPlatformCamelConfig),
}

/// Kubernetes platform configuration for `[platform]` in Camel.toml.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesPlatformCamelConfig {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default = "default_lease_name_prefix")]
    pub lease_name_prefix: String,
    #[serde(default = "default_lease_duration_secs")]
    pub lease_duration_secs: u64,
    #[serde(default = "default_renew_deadline_secs")]
    pub renew_deadline_secs: u64,
    #[serde(default = "default_retry_period_secs")]
    pub retry_period_secs: u64,
    #[serde(default = "default_kubernetes_jitter_factor")]
    pub jitter_factor: f64,
}

impl Default for KubernetesPlatformCamelConfig {
    fn default() -> Self {
        Self {
            namespace: None,
            lease_name_prefix: default_lease_name_prefix(),
            lease_duration_secs: default_lease_duration_secs(),
            renew_deadline_secs: default_renew_deadline_secs(),
            retry_period_secs: default_retry_period_secs(),
            jitter_factor: default_kubernetes_jitter_factor(),
        }
    }
}

fn default_lease_name_prefix() -> String {
    "camel-".to_string()
}
fn default_lease_duration_secs() -> u64 {
    15
}
fn default_renew_deadline_secs() -> u64 {
    10
}
fn default_retry_period_secs() -> u64 {
    2
}
fn default_kubernetes_jitter_factor() -> f64 {
    0.2
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct ComponentsConfig {
    /// Raw per-component config blocks, keyed by component name.
    /// Each bundle is responsible for deserializing its own block.
    #[serde(flatten)]
    pub raw: HashMap<String, toml::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrometheusCamelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_prometheus_host")]
    pub host: String,
    #[serde(default = "default_prometheus_port")]
    pub port: u16,
}

impl Default for PrometheusCamelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_prometheus_host(),
            port: default_prometheus_port(),
        }
    }
}

fn default_prometheus_host() -> String {
    "0.0.0.0".to_string()
}
fn default_prometheus_port() -> u16 {
    9090
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HealthCamelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_health_host")]
    pub host: String,
    #[serde(default = "default_health_port")]
    pub port: u16,
}

impl Default for HealthCamelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_health_host(),
            port: default_health_port(),
        }
    }
}

fn default_health_host() -> String {
    "0.0.0.0".to_string()
}

fn default_health_port() -> u16 {
    8081
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityConfig {
    #[serde(default)]
    pub tracer: TracerConfig,

    #[serde(default)]
    pub otel: Option<OtelCamelConfig>,

    #[serde(default)]
    pub prometheus: Option<PrometheusCamelConfig>,

    #[serde(default)]
    pub health: Option<HealthCamelConfig>,
}

/// Protocol for OTLP export.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OtelProtocol {
    #[default]
    Grpc,
    Http,
}

/// Sampling strategy.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OtelSampler {
    #[default]
    AlwaysOn,
    AlwaysOff,
    Ratio,
}

/// OpenTelemetry configuration for `[observability.otel]` in Camel.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OtelCamelConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_otel_endpoint")]
    pub endpoint: String,

    #[serde(default = "default_otel_service_name")]
    pub service_name: String,

    #[serde(default = "default_otel_log_level")]
    pub log_level: String,

    #[serde(default)]
    pub protocol: OtelProtocol,

    #[serde(default)]
    pub sampler: OtelSampler,

    #[serde(default)]
    pub sampler_ratio: Option<f64>,

    #[serde(default = "default_otel_metrics_interval_ms")]
    pub metrics_interval_ms: u64,

    #[serde(default = "default_true")]
    pub logs_enabled: bool,

    #[serde(default)]
    pub resource_attrs: HashMap<String, String>,
}

impl Default for OtelCamelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_otel_endpoint(),
            service_name: default_otel_service_name(),
            log_level: default_otel_log_level(),
            protocol: OtelProtocol::default(),
            sampler: OtelSampler::default(),
            sampler_ratio: None,
            metrics_interval_ms: default_otel_metrics_interval_ms(),
            logs_enabled: true,
            resource_attrs: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SupervisionCamelConfig {
    /// Maximum number of restart attempts. `None` means retry forever.
    pub max_attempts: Option<u32>,

    /// Delay before the first restart attempt in milliseconds.
    #[serde(default = "default_initial_delay_ms")]
    pub initial_delay_ms: u64,

    /// Multiplier applied to the delay after each failed attempt.
    #[serde(default = "default_backoff_multiplier")]
    pub backoff_multiplier: f64,

    /// Maximum delay cap between restart attempts in milliseconds.
    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u64,
}

impl Default for SupervisionCamelConfig {
    fn default() -> Self {
        Self {
            max_attempts: Some(5),
            initial_delay_ms: 1000,
            backoff_multiplier: 2.0,
            max_delay_ms: 60000,
        }
    }
}

impl SupervisionCamelConfig {
    /// Convert to camel_api::SupervisionConfig
    pub fn into_supervision_config(self) -> camel_api::SupervisionConfig {
        camel_api::SupervisionConfig {
            max_attempts: self.max_attempts,
            initial_delay: Duration::from_millis(self.initial_delay_ms),
            backoff_multiplier: self.backoff_multiplier,
            max_delay: Duration::from_millis(self.max_delay_ms),
        }
    }
}

/// Durability mode for the redb journal. Mirrors `camel_core::JournalDurability`.
///
/// Defined here (in camel-config) for TOML deserialization. Mapped to the
/// camel-core type in `context_ext.rs` via `From`. No circular dependency —
/// camel-config already depends on camel-core.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum JournalDurability {
    /// fsync on every commit — protects against power loss (default).
    #[default]
    Immediate,
    /// No fsync — suitable for dev/test.
    Eventual,
}

impl From<JournalDurability> for camel_core::JournalDurability {
    fn from(d: JournalDurability) -> Self {
        match d {
            JournalDurability::Immediate => camel_core::JournalDurability::Immediate,
            JournalDurability::Eventual => camel_core::JournalDurability::Eventual,
        }
    }
}

fn default_compaction_threshold_events() -> u64 {
    10_000
}

/// Configuration for the redb runtime event journal.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JournalConfig {
    /// Path to the `.db` file. Created if it does not exist.
    pub path: std::path::PathBuf,

    /// Durability mode. Default: `immediate`.
    #[serde(default)]
    pub durability: JournalDurability,

    /// Trigger compaction after this many events. Default: 10_000.
    #[serde(default = "default_compaction_threshold_events")]
    pub compaction_threshold_events: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StreamCachingConfig {
    #[serde(default = "default_stream_cache_threshold")]
    pub threshold: usize,
}

fn default_stream_cache_threshold() -> usize {
    camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD
}

impl Default for StreamCachingConfig {
    fn default() -> Self {
        Self {
            threshold: default_stream_cache_threshold(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BeanConfig {
    pub plugin: String,
    #[serde(default)]
    pub config: HashMap<String, String>,
    /// WASM runtime limits for this bean. All `None` by default — runtime
    /// defaults apply. See `WasmLimitsConfig`.
    #[serde(default)]
    pub limits: crate::wasm_limits::WasmLimitsConfig,
}

// ---------------------------------------------------------------------------
// Security configuration (Keycloak etc.)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    #[serde(default)]
    pub oidc: Option<OidcSecurityConfig>,
    #[serde(default)]
    pub native: Option<NativeAuthConfig>,
    #[serde(default)]
    pub keycloak: Option<KeycloakSecurityConfig>,
    #[serde(default)]
    pub permissions: Option<HashMap<String, PermissionProviderConfig>>,
    #[serde(default)]
    pub policies: Option<WasmSecurityPoliciesConfig>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OidcSecurityConfig {
    pub issuer: String,
    #[serde(default)]
    pub jwks_uri: Option<String>,
    #[serde(default)]
    pub audience: Vec<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(skip_serializing)]
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub token_endpoint: Option<String>,
    #[serde(default)]
    pub introspection_endpoint: Option<String>,
}

impl fmt::Debug for OidcSecurityConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcSecurityConfig")
            .field("issuer", &self.issuer)
            .field("jwks_uri", &self.jwks_uri)
            .field("audience", &self.audience)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("token_endpoint", &self.token_endpoint)
            .field("introspection_endpoint", &self.introspection_endpoint)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct NativeIssuerConfig {
    pub issuer: String,
    #[serde(default)]
    pub audience: Vec<String>,
    #[serde(default = "default_token_ttl")]
    pub token_ttl_secs: u64,
    pub signing_key_env: String,
}

fn default_token_ttl() -> u64 {
    900
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct NativeM2mClientConfig {
    pub client_id: String,
    pub client_secret_env: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl fmt::Debug for NativeM2mClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeM2mClientConfig")
            .field("client_id", &self.client_id)
            .field("client_secret_env", &"[REDACTED]")
            .field("roles", &self.roles)
            .field("scopes", &self.scopes)
            .finish()
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NativeAuthConfig {
    pub subject: String,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub bearer_token: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub token_issuer: Option<NativeIssuerConfig>,
    #[serde(default)]
    pub clients: Vec<NativeM2mClientConfig>,
}

impl fmt::Debug for NativeAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeAuthConfig")
            .field("subject", &self.subject)
            .field("issuer", &self.issuer)
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("roles", &self.roles)
            .field("scopes", &self.scopes)
            .field("token_issuer", &self.token_issuer)
            .field("clients", &self.clients)
            .finish()
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeycloakSecurityConfig {
    pub server_url: String,
    pub realm: String,
    pub client_id: String,
    #[serde(skip_serializing)]
    pub client_secret: String,
    #[serde(default)]
    pub validation: KeycloakValidationConfig,
    #[serde(default)]
    pub jwks: KeycloakJwksConfig,
    #[serde(default)]
    pub introspection: KeycloakIntrospectionConfig,
    #[serde(default)]
    pub uma: Option<KeycloakUmaConfig>,
}

impl fmt::Debug for KeycloakSecurityConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeycloakSecurityConfig")
            .field("server_url", &self.server_url)
            .field("realm", &self.realm)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("validation", &self.validation)
            .field("jwks", &self.jwks)
            .field("introspection", &self.introspection)
            .field("uma", &self.uma)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeycloakValidationConfig {
    #[serde(default = "default_validation_method")]
    pub method: String,
    #[serde(default)]
    pub audience: Vec<String>,
    #[serde(default = "default_clock_skew")]
    pub clock_skew_secs: u64,
}

impl Default for KeycloakValidationConfig {
    fn default() -> Self {
        Self {
            method: default_validation_method(),
            audience: Vec::new(),
            clock_skew_secs: default_clock_skew(),
        }
    }
}

fn default_validation_method() -> String {
    "local".into()
}

fn default_clock_skew() -> u64 {
    30
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeycloakJwksConfig {
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_secs: u64,
    #[serde(default = "default_refresh_skew")]
    pub refresh_skew_secs: u64,
}

impl Default for KeycloakJwksConfig {
    fn default() -> Self {
        Self {
            cache_ttl_secs: default_cache_ttl(),
            refresh_skew_secs: default_refresh_skew(),
        }
    }
}

fn default_cache_ttl() -> u64 {
    3600
}

fn default_refresh_skew() -> u64 {
    60
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeycloakIntrospectionConfig {
    #[serde(default = "default_introspection_max_entries")]
    pub max_entries: usize,
    #[serde(default = "default_introspection_ttl")]
    pub default_ttl_secs: u64,
    #[serde(default = "default_introspection_negative_ttl")]
    pub negative_ttl_secs: u64,
}

impl Default for KeycloakIntrospectionConfig {
    fn default() -> Self {
        Self {
            max_entries: default_introspection_max_entries(),
            default_ttl_secs: default_introspection_ttl(),
            negative_ttl_secs: default_introspection_negative_ttl(),
        }
    }
}

fn default_introspection_max_entries() -> usize {
    10_000
}

fn default_introspection_ttl() -> u64 {
    60
}

fn default_introspection_negative_ttl() -> u64 {
    5
}

// ---------------------------------------------------------------------------
// Keycloak UMA (User-Managed Access) configuration
// ---------------------------------------------------------------------------

/// Cache configuration for permission evaluators.
/// Default positive TTL is 30s (shorter than token introspection's 60s)
/// because authorization decisions can change faster than identity claims.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PermissionCacheConfig {
    #[serde(default = "default_positive_ttl_secs")]
    pub positive_ttl_secs: u64,
    #[serde(default = "default_negative_ttl_secs")]
    pub negative_ttl_secs: u64,
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PermissionProviderConfig {
    pub provider: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub config: Option<HashMap<String, String>>,
    #[serde(default)]
    pub cache: PermissionCacheConfig,
    /// WASM runtime limits applied when `provider = "wasm"`. Ignored otherwise.
    #[serde(default)]
    pub limits: crate::wasm_limits::WasmLimitsConfig,
}

/// Configuration for a single WASM-based security policy, referenced by name from
/// `[security.policies.wasm.<name>]` in Camel.toml.
///
/// ```toml
/// [security.policies.wasm.corp-auth]
/// path = "plugins/authz.wasm"
/// [security.policies.wasm.corp-auth.limits]
/// timeout-secs = 30
/// [security.policies.wasm.corp-auth.config]
/// ldap_url = "ldap://corp"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WasmSecurityPolicyConfig {
    /// Path to the .wasm file, relative to the project root or absolute.
    pub path: String,
    /// WASM runtime limits (timeout, memory, concurrency).
    #[serde(default)]
    pub limits: crate::wasm_limits::WasmLimitsConfig,
    /// Key-value pairs passed to the guest's `init()` function.
    #[serde(default)]
    pub config: HashMap<String, String>,
}

/// Wrapper for `[security.policies]` sub-tables.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WasmSecurityPoliciesConfig {
    /// Named WASM security policies, keyed by registry name.
    pub wasm: HashMap<String, WasmSecurityPolicyConfig>,
}

fn default_positive_ttl_secs() -> u64 {
    30
}
fn default_negative_ttl_secs() -> u64 {
    5
}
fn default_max_entries() -> usize {
    10_000
}

impl Default for PermissionCacheConfig {
    fn default() -> Self {
        Self {
            positive_ttl_secs: default_positive_ttl_secs(),
            negative_ttl_secs: default_negative_ttl_secs(),
            max_entries: default_max_entries(),
        }
    }
}

/// Configuration for Keycloak UMA (User-Managed Access) authorization.
///
/// Inherits `server_url`, `realm`, `client_id`, `client_secret` from the parent
/// [`KeycloakSecurityConfig`]; only provider selection and cache tuning live here.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeycloakUmaConfig {
    pub provider: String,
    #[serde(default)]
    pub cache: PermissionCacheConfig,
}

impl From<&JournalConfig> for camel_core::RedbJournalOptions {
    fn from(cfg: &JournalConfig) -> Self {
        camel_core::RedbJournalOptions {
            durability: cfg.durability.clone().into(),
            compaction_threshold_events: cfg.compaction_threshold_events,
        }
    }
}

fn default_log_level() -> String {
    "INFO".to_string()
}
fn default_timeout_ms() -> u64 {
    5000
}
fn default_drain_timeout_ms() -> u64 {
    10_000
}
fn default_watch_debounce_ms() -> u64 {
    300
}

fn default_otel_endpoint() -> String {
    "http://localhost:4317".to_string()
}
fn default_otel_service_name() -> String {
    "rust-camel".to_string()
}
fn default_otel_log_level() -> String {
    "info".to_string()
}
fn default_otel_metrics_interval_ms() -> u64 {
    60000
}
fn default_true() -> bool {
    true
}

fn default_initial_delay_ms() -> u64 {
    1000
}

fn default_backoff_multiplier() -> f64 {
    2.0
}

fn default_max_delay_ms() -> u64 {
    60000
}

/// Deep merge two TOML values
/// Tables are merged recursively, with overlay values taking precedence
pub(crate) fn merge_toml_values(base: &mut toml::Value, overlay: &toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, value) in overlay_table {
                if let Some(base_value) = base_table.get_mut(key) {
                    // Both have this key - merge recursively
                    merge_toml_values(base_value, value);
                } else {
                    // Only overlay has this key - insert it
                    base_table.insert(key.clone(), value.clone());
                }
            }
        }
        // For non-table values, overlay replaces base entirely
        (base, overlay) => {
            *base = overlay.clone();
        }
    }
}

impl CamelConfig {
    fn resolve_placeholders(&mut self) {
        let resolver = PropertiesResolver::new();

        for route in &mut self.routes {
            if let Ok(resolved) = resolver.resolve(route) {
                *route = resolved;
            } else {
                tracing::warn!(route = %route, "Failed to resolve placeholder in routes entry; keeping original");
            }
        }

        if let Ok(resolved) = resolver.resolve(&self.log_level) {
            self.log_level = resolved;
        } else {
            tracing::warn!(log_level = %self.log_level, "Failed to resolve placeholder in log_level; keeping original");
        }

        if let Some(otel) = self.observability.otel.as_mut() {
            resolve_string_in_place(&resolver, &mut otel.endpoint, "observability.otel.endpoint");
            resolve_string_in_place(
                &resolver,
                &mut otel.service_name,
                "observability.otel.service_name",
            );
            resolve_string_in_place(
                &resolver,
                &mut otel.log_level,
                "observability.otel.log_level",
            );
            for (k, v) in &mut otel.resource_attrs {
                let field = format!("observability.otel.resource_attrs.{k}");
                resolve_string_in_place(&resolver, v, &field);
            }
        }

        if let Some(prom) = self.observability.prometheus.as_mut() {
            resolve_string_in_place(&resolver, &mut prom.host, "observability.prometheus.host");
        }

        if let Some(health) = self.observability.health.as_mut() {
            resolve_string_in_place(&resolver, &mut health.host, "observability.health.host");
        }

        if let PlatformCamelConfig::Kubernetes(k8s) = &mut self.platform {
            if let Some(namespace) = k8s.namespace.as_mut() {
                resolve_string_in_place(&resolver, namespace, "platform.namespace");
            }
            resolve_string_in_place(
                &resolver,
                &mut k8s.lease_name_prefix,
                "platform.lease_name_prefix",
            );
        }

        for (component_name, value) in &mut self.components.raw {
            resolve_toml_value_placeholders(
                &resolver,
                value,
                &format!("components.{component_name}"),
            );
        }

        for bean in self.beans.values_mut() {
            resolve_string_in_place(&resolver, &mut bean.plugin, "beans.*.plugin");
            let resolved: HashMap<String, String> = bean
                .config
                .drain()
                .map(|(k, v)| match resolver.resolve(&v) {
                    Ok(resolved) => (k, resolved),
                    Err(err) => {
                        tracing::warn!(key = %k, value = %v, error = %err, "Failed to resolve bean config placeholder; keeping original");
                        (k, v)
                    }
                })
                .collect();
            bean.config = resolved;
        }
    }

    pub fn validate(&self) -> Result<(), CamelError> {
        if self.timeout_ms == 0 {
            return Err(CamelError::Config("timeout_ms must be > 0".to_string()));
        }
        if self.drain_timeout_ms == 0 {
            return Err(CamelError::Config(
                "drain_timeout_ms must be > 0".to_string(),
            ));
        }
        if self.watch_debounce_ms == 0 {
            return Err(CamelError::Config(
                "watch_debounce_ms must be > 0".to_string(),
            ));
        }
        if let Some(ref journal) = self.runtime_journal {
            if journal.path.as_os_str().is_empty() {
                return Err(CamelError::Config(
                    "runtime_journal.path must not be empty".to_string(),
                ));
            }
            if journal.compaction_threshold_events == 0 {
                return Err(CamelError::Config(
                    "runtime_journal.compaction_threshold_events must be > 0".to_string(),
                ));
            }
        }
        for (name, bean) in &self.beans {
            if bean.plugin.trim().is_empty() {
                return Err(CamelError::Config(format!(
                    "bean '{}' must have a non-empty plugin",
                    name
                )));
            }
        }
        if let Some(ref sup) = self.supervision {
            if sup.initial_delay_ms == 0 {
                return Err(CamelError::Config(
                    "supervision.initial_delay_ms must be > 0".to_string(),
                ));
            }
            if sup.max_delay_ms == 0 {
                return Err(CamelError::Config(
                    "supervision.max_delay_ms must be > 0".to_string(),
                ));
            }
            if sup.backoff_multiplier < 1.0 {
                return Err(CamelError::Config(
                    "supervision.backoff_multiplier must be >= 1.0".to_string(),
                ));
            }
        }
        if let Some(ref otel) = self.observability.otel
            && otel.metrics_interval_ms == 0
        {
            return Err(CamelError::Config(
                "observability.otel.metrics_interval_ms must be > 0".to_string(),
            ));
        }
        if let PlatformCamelConfig::Kubernetes(ref k8s) = self.platform {
            if k8s.lease_duration_secs == 0 {
                return Err(CamelError::Config(
                    "platform.lease_duration_secs must be > 0".to_string(),
                ));
            }
            if k8s.renew_deadline_secs == 0 {
                return Err(CamelError::Config(
                    "platform.renew_deadline_secs must be > 0".to_string(),
                ));
            }
            if k8s.retry_period_secs == 0 {
                return Err(CamelError::Config(
                    "platform.retry_period_secs must be > 0".to_string(),
                ));
            }
            if k8s.jitter_factor < 0.0 || k8s.jitter_factor > 1.0 {
                return Err(CamelError::Config(
                    "platform.jitter_factor must be between 0.0 and 1.0".to_string(),
                ));
            }
        }
        Ok(())
    }

    pub fn from_file(path: &str) -> Result<Self, ConfigError> {
        Self::from_file_with_profile(path, None)
    }

    pub fn from_file_with_env(path: &str) -> Result<Self, ConfigError> {
        Self::from_file_with_profile_and_env(path, None)
    }

    pub fn from_file_with_profile(path: &str, profile: Option<&str>) -> Result<Self, ConfigError> {
        Self::load_from_file_inner(path, profile, false)
    }

    pub fn from_file_with_profile_and_env(
        path: &str,
        profile: Option<&str>,
    ) -> Result<Self, ConfigError> {
        Self::load_from_file_inner(path, profile, true)
    }

    fn load_from_file_inner(
        path: &str,
        profile: Option<&str>,
        merge_env: bool,
    ) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Message(format!("Failed to read config file: {}", e)))?;

        let base_dir = std::path::Path::new(path)
            .parent()
            .unwrap_or(std::path::Path::new("."));

        let mut root_value: toml::Value = toml::from_str(&content)
            .map_err(|e| ConfigError::Message(format!("Failed to parse TOML: {}", e)))?;

        let includes = Self::extract_includes(&root_value)?;

        // Strip `include` before passing to inner builder (not a CamelConfig field)
        if let toml::Value::Table(ref mut table) = root_value {
            table.remove("include");
        }

        let env_profile = std::env::var("CAMEL_PROFILE").ok();
        let effective_profile = profile.or(env_profile.as_deref());

        let pre_sources = crate::include::load_includes(base_dir, &includes, effective_profile)?;

        Self::build_from_toml_value_inner(root_value, profile, merge_env, pre_sources)
    }

    /// Validates and extracts the `include` field from a parsed TOML value.
    /// Returns an error if `include` is present but not an array of strings.
    fn extract_includes(raw_value: &toml::Value) -> Result<Vec<String>, ConfigError> {
        match raw_value.get("include") {
            None => Ok(vec![]),
            Some(toml::Value::Array(arr)) => {
                let mut paths = Vec::with_capacity(arr.len());
                for (i, item) in arr.iter().enumerate() {
                    match item.as_str() {
                        Some(s) => paths.push(s.to_string()),
                        None => {
                            return Err(ConfigError::Message(format!(
                                "include[{}] must be a string, got: {}",
                                i, item
                            )));
                        }
                    }
                }
                Ok(paths)
            }
            Some(other) => Err(ConfigError::Message(format!(
                "'include' must be an array of strings, got: {}",
                other.type_str()
            ))),
        }
    }

    pub fn from_env_or_default() -> Result<Self, ConfigError> {
        let path = env::var("CAMEL_CONFIG_FILE").unwrap_or_else(|_| "Camel.toml".to_string());

        Self::from_file(&path)
    }

    /// Async version of [`Self::from_file`] — uses `tokio::fs` to avoid blocking the executor.
    pub async fn from_file_async(path: &str) -> Result<Self, ConfigError> {
        Self::from_file_async_with_profile(path, None).await
    }

    /// Async version of [`Self::from_file_with_profile`] — uses `tokio::fs`.
    pub async fn from_file_async_with_profile(
        path: &str,
        profile: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| ConfigError::Message(format!("Failed to read config file: {}", e)))?;

        let base_dir_owned = std::path::Path::new(path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();

        let mut root_value: toml::Value = toml::from_str(&content)
            .map_err(|e| ConfigError::Message(format!("Failed to parse TOML: {}", e)))?;

        let includes = Self::extract_includes(&root_value)?;

        if let toml::Value::Table(ref mut table) = root_value {
            table.remove("include");
        }

        let env_profile = std::env::var("CAMEL_PROFILE").ok();
        let effective_profile = profile.or(env_profile.as_deref());

        let pre_sources =
            crate::include::load_includes(&base_dir_owned, &includes, effective_profile)?;

        Self::build_from_toml_value_inner(root_value, profile, false, pre_sources)
    }

    /// Async version of [`Self::from_file_with_env`] — uses `tokio::fs`.
    pub async fn from_file_async_with_env(path: &str) -> Result<Self, ConfigError> {
        Self::from_file_async_with_profile_and_env(path, None).await
    }

    /// Async version of [`Self::from_file_with_profile_and_env`] — uses `tokio::fs`.
    pub async fn from_file_async_with_profile_and_env(
        path: &str,
        profile: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| ConfigError::Message(format!("Failed to read config file: {}", e)))?;

        let base_dir_owned = std::path::Path::new(path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();

        let mut root_value: toml::Value = toml::from_str(&content)
            .map_err(|e| ConfigError::Message(format!("Failed to parse TOML: {}", e)))?;

        let includes = Self::extract_includes(&root_value)?;

        if let toml::Value::Table(ref mut table) = root_value {
            table.remove("include");
        }

        let env_profile = std::env::var("CAMEL_PROFILE").ok();
        let effective_profile = profile.or(env_profile.as_deref());

        let pre_sources =
            crate::include::load_includes(&base_dir_owned, &includes, effective_profile)?;

        Self::build_from_toml_value_inner(root_value, profile, true, pre_sources)
    }

    /// Core config builder. Accepts a pre-parsed (and `include`-stripped) `toml::Value`
    /// so callers do not need to re-parse the content.
    fn build_from_toml_value_inner(
        mut config_value: toml::Value,
        profile: Option<&str>,
        merge_env: bool,
        pre_sources: Vec<String>,
    ) -> Result<Self, ConfigError> {
        let env_profile = env::var("CAMEL_PROFILE").ok();
        let profile = profile.or(env_profile.as_deref());

        // Defensively strip `include` in case callers forgot — it is not a CamelConfig field.
        if let toml::Value::Table(ref mut table) = config_value {
            table.remove("include");
        }

        // Detect whether the root file has profile sections (e.g. [default], [production]).
        // If it does, use strict profile handling (unknown profile → error).
        // If it doesn't (flat config), use lenient handling (keep as-is).
        let has_profile_structure = if let toml::Value::Table(ref table) = config_value {
            table.contains_key("default") || profile.is_some_and(|p| table.contains_key(p))
        } else {
            false
        };

        if has_profile_structure {
            apply_profile(&mut config_value, profile)?;
        } else {
            // Flat config — no profile sections, keep as-is
            apply_profile_lenient(&mut config_value, profile);
        }

        let merged_toml = toml::to_string(&config_value).map_err(|e| {
            ConfigError::Message(format!("Failed to serialize merged config: {}", e))
        })?;

        let mut builder = Config::builder();
        for source_toml in pre_sources {
            builder = builder.add_source(config::File::from_str(
                &source_toml,
                config::FileFormat::Toml,
            ));
        }
        builder = builder.add_source(config::File::from_str(
            &merged_toml,
            config::FileFormat::Toml,
        ));
        if merge_env {
            builder =
                builder.add_source(config::Environment::with_prefix("CAMEL").try_parsing(true));
        }
        let config = builder.build()?;

        let mut config: Self = config.try_deserialize()?;
        config.resolve_placeholders();
        config
            .validate()
            .map_err(|e| ConfigError::Message(e.to_string()))?;
        Ok(config)
    }
}

fn resolve_string_in_place(resolver: &PropertiesResolver, value: &mut String, field: &str) {
    match resolver.resolve(value) {
        Ok(resolved) => *value = resolved,
        Err(err) => {
            tracing::warn!(field = field, value = %value, error = %err, "Failed to resolve placeholder; keeping original");
        }
    }
}

fn resolve_toml_value_placeholders(
    resolver: &PropertiesResolver,
    value: &mut toml::Value,
    path: &str,
) {
    match value {
        toml::Value::String(s) => resolve_string_in_place(resolver, s, path),
        toml::Value::Array(arr) => {
            for (idx, item) in arr.iter_mut().enumerate() {
                resolve_toml_value_placeholders(resolver, item, &format!("{path}[{idx}]"));
            }
        }
        toml::Value::Table(table) => {
            for (k, v) in table.iter_mut() {
                resolve_toml_value_placeholders(resolver, v, &format!("{path}.{k}"));
            }
        }
        _ => {}
    }
}

/// Apply profile-based TOML section merging in-place.
pub(crate) fn apply_profile(
    config_value: &mut toml::Value,
    profile: Option<&str>,
) -> Result<(), ConfigError> {
    if let Some(p) = profile {
        let default_value = config_value.get("default").cloned();
        let profile_value = config_value.get(p).cloned();

        if let (Some(mut base), Some(overlay)) = (default_value, profile_value) {
            merge_toml_values(&mut base, &overlay);
            *config_value = base;
        } else if let Some(profile_val) = config_value.get(p).cloned() {
            *config_value = profile_val;
        } else {
            return Err(ConfigError::Message(format!("Unknown profile: {}", p)));
        }
    } else if let Some(default_val) = config_value.get("default").cloned() {
        *config_value = default_val;
    }
    // If no profile active and no [default] → keep as-is
    Ok(())
}

/// Like `apply_profile` but lenient: if the included file has no profile sections,
/// keep it as-is rather than returning an error. Use for included files that may be
/// written as flat config without profile sections.
pub(crate) fn apply_profile_lenient(value: &mut toml::Value, profile: Option<&str>) {
    if let Some(p) = profile {
        let default_value = value.get("default").cloned();
        let profile_value = value.get(p).cloned();
        match (default_value, profile_value) {
            (Some(mut base), Some(overlay)) => {
                merge_toml_values(&mut base, &overlay);
                *value = base;
            }
            (None, Some(profile_val)) => {
                *value = profile_val;
            }
            (Some(default_val), None) => {
                // Has [default] but not this profile → use default
                *value = default_val;
            }
            (None, None) => {
                // No profile structure → use file as-is (flat config without profiles)
            }
        }
    } else if let Some(default_val) = value.get("default").cloned() {
        *value = default_val;
    }
    // If no profile active and no [default] → keep as-is
}

#[cfg(test)]
mod camel_config_defaults_tests {
    use super::*;

    #[test]
    fn watch_debounce_ms_default_is_300() {
        let config: CamelConfig = toml::from_str("").unwrap();
        assert_eq!(config.watch_debounce_ms, 300);
    }

    #[test]
    fn watch_debounce_ms_custom_value() {
        let config: CamelConfig = toml::from_str("watch_debounce_ms = 50").unwrap();
        assert_eq!(config.watch_debounce_ms, 50);
    }

    #[test]
    fn stream_caching_default_threshold_is_set() {
        let config: CamelConfig = toml::from_str("").unwrap();
        assert_eq!(
            config.stream_caching.threshold,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD
        );
    }

    #[test]
    fn stream_caching_custom_threshold_value() {
        let config: CamelConfig = toml::from_str("[stream_caching]\nthreshold = 1234").unwrap();
        assert_eq!(config.stream_caching.threshold, 1234);
    }
}

#[cfg(test)]
mod components_config_tests {
    use super::*;

    #[test]
    fn components_config_deserializes_raw_toml_block() {
        let toml_str = r#"
            [kafka]
            brokers = ["localhost:9092"]

            [redis]
            host = "redis.local"
        "#;
        let cfg: ComponentsConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.raw.contains_key("kafka"));
        assert!(cfg.raw.contains_key("redis"));
    }
}

#[cfg(test)]
mod prometheus_config_tests {
    use super::*;

    fn parse(toml: &str) -> CamelConfig {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(toml, config::FileFormat::Toml))
            .build()
            .unwrap();
        cfg.try_deserialize().unwrap()
    }

    #[test]
    fn test_prometheus_absent_is_none() {
        let cfg = parse("");
        assert!(cfg.observability.prometheus.is_none());
    }

    #[test]
    fn test_prometheus_defaults() {
        let cfg = parse(
            r#"
[observability.prometheus]
enabled = true
"#,
        );
        let p = cfg.observability.prometheus.unwrap();
        assert!(p.enabled);
        assert_eq!(p.host, "0.0.0.0");
        assert_eq!(p.port, 9090);
    }

    #[test]
    fn test_prometheus_full() {
        let cfg = parse(
            r#"
[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = 9091
"#,
        );
        let p = cfg.observability.prometheus.unwrap();
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, 9091);
    }

    #[test]
    fn test_health_config_defaults() {
        let cfg = parse(
            r#"
[observability.health]
enabled = true
"#,
        );
        let h = cfg.observability.health.unwrap();
        assert!(h.enabled);
        assert_eq!(h.host, "0.0.0.0");
        assert_eq!(h.port, 8081);
    }

    #[test]
    fn test_health_config_custom_port() {
        let cfg = parse(
            r#"
[observability.health]
enabled = true
port = 9091
"#,
        );
        let h = cfg.observability.health.unwrap();
        assert_eq!(h.port, 9091);
        assert_eq!(h.host, "0.0.0.0");
    }
}

#[cfg(test)]
mod platform_config_tests {
    use super::*;

    fn parse(toml: &str) -> CamelConfig {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(toml, config::FileFormat::Toml))
            .build()
            .unwrap();
        cfg.try_deserialize().unwrap()
    }

    #[test]
    fn platform_default_is_noop() {
        let cfg = parse("");
        assert!(matches!(cfg.platform, PlatformCamelConfig::Noop));
    }

    #[test]
    fn platform_parses_kubernetes_from_toml() {
        let cfg = parse(
            r#"
[platform]
type = "kubernetes"
namespace = "team-a"
lease_name_prefix = "camel-"
lease_duration_secs = 15
renew_deadline_secs = 10
retry_period_secs = 2
jitter_factor = 0.2
"#,
        );
        match cfg.platform {
            PlatformCamelConfig::Kubernetes(k8s) => {
                assert_eq!(k8s.namespace.as_deref(), Some("team-a"));
                assert_eq!(k8s.lease_name_prefix, "camel-");
                assert_eq!(k8s.lease_duration_secs, 15);
                assert_eq!(k8s.renew_deadline_secs, 10);
                assert_eq!(k8s.retry_period_secs, 2);
                assert!((k8s.jitter_factor - 0.2).abs() < f64::EPSILON);
            }
            other => panic!("expected Kubernetes, got {:?}", other),
        }
    }

    #[test]
    fn platform_kubernetes_defaults() {
        let cfg = parse(
            r#"
[platform]
type = "kubernetes"
"#,
        );
        match cfg.platform {
            PlatformCamelConfig::Kubernetes(k8s) => {
                assert!(k8s.namespace.is_none());
                assert_eq!(k8s.lease_name_prefix, "camel-");
                assert_eq!(k8s.lease_duration_secs, 15);
                assert_eq!(k8s.renew_deadline_secs, 10);
                assert_eq!(k8s.retry_period_secs, 2);
                assert!((k8s.jitter_factor - 0.2).abs() < f64::EPSILON);
            }
            other => panic!("expected Kubernetes, got {:?}", other),
        }
    }

    #[test]
    fn platform_parses_kubernetes_from_file_with_profile() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(
            br#"
[default]
[default.platform]
type = "kubernetes"
namespace = "production"

[dev]
[dev.platform]
type = "noop"
"#,
        )
        .expect("write config");

        let cfg_prod =
            CamelConfig::from_file_with_profile(f.path().to_str().unwrap(), Some("default"))
                .expect("prod config");
        assert!(matches!(
            cfg_prod.platform,
            PlatformCamelConfig::Kubernetes(_)
        ));

        let cfg_dev = CamelConfig::from_file_with_profile(f.path().to_str().unwrap(), Some("dev"))
            .expect("dev config");
        assert!(matches!(cfg_dev.platform, PlatformCamelConfig::Noop));
    }
}

#[cfg(test)]
mod profile_loading_tests {
    use super::*;

    fn write_temp_config(contents: &str) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(contents.as_bytes()).expect("write config");
        f
    }

    #[test]
    fn test_merge_toml_values_merges_nested_tables() {
        let mut base: toml::Value = toml::from_str(
            r#"
[components.http]
connect_timeout_ms = 1000
pool_max_idle_per_host = 50
"#,
        )
        .unwrap();

        let overlay: toml::Value = toml::from_str(
            r#"
[components.http]
response_timeout_ms = 2000
pool_max_idle_per_host = 99
"#,
        )
        .unwrap();

        merge_toml_values(&mut base, &overlay);

        let http = base
            .get("components")
            .and_then(|v| v.get("http"))
            .expect("merged http table");
        assert_eq!(
            http.get("connect_timeout_ms").and_then(|v| v.as_integer()),
            Some(1000)
        );
        assert_eq!(
            http.get("response_timeout_ms").and_then(|v| v.as_integer()),
            Some(2000)
        );
        assert_eq!(
            http.get("pool_max_idle_per_host")
                .and_then(|v| v.as_integer()),
            Some(99)
        );
    }

    #[test]
    fn test_from_file_with_profile_merges_default_and_profile() {
        let file = write_temp_config(
            r#"
[default]
watch = false
[default.components.http]
connect_timeout_ms = 1000
pool_max_idle_per_host = 50

[prod]
watch = true
[prod.components.http]
pool_max_idle_per_host = 200
"#,
        );

        let cfg = CamelConfig::from_file_with_profile(file.path().to_str().unwrap(), Some("prod"))
            .expect("config should load");

        assert!(cfg.watch);
        let http = cfg.components.raw.get("http").expect("http config");
        assert_eq!(
            http.get("connect_timeout_ms").and_then(|v| v.as_integer()),
            Some(1000)
        );
        assert_eq!(
            http.get("pool_max_idle_per_host")
                .and_then(|v| v.as_integer()),
            Some(200)
        );
    }

    #[test]
    fn test_from_file_with_profile_uses_profile_when_no_default() {
        let file = write_temp_config(
            r#"
[dev]
watch = true
timeout_ms = 777
"#,
        );

        let cfg = CamelConfig::from_file_with_profile(file.path().to_str().unwrap(), Some("dev"))
            .expect("config should load");
        assert!(cfg.watch);
        assert_eq!(cfg.timeout_ms, 777);
    }

    #[test]
    fn test_from_file_with_profile_unknown_profile_returns_error() {
        let file = write_temp_config(
            r#"
[default]
watch = false
"#,
        );

        let err = CamelConfig::from_file_with_profile(file.path().to_str().unwrap(), Some("qa"))
            .expect_err("should fail");
        assert!(err.to_string().contains("Unknown profile: qa"));
    }

    #[test]
    fn test_from_file_without_profile_uses_default_section() {
        let file = write_temp_config(
            r#"
[default]
watch = true
timeout_ms = 321
"#,
        );

        let cfg =
            CamelConfig::from_file(file.path().to_str().unwrap()).expect("config should load");
        assert!(cfg.watch);
        assert_eq!(cfg.timeout_ms, 321);
    }

    #[test]
    fn test_from_file_with_env_overrides_timeout() {
        let file = write_temp_config(
            r#"
[default]
timeout_ms = 1000
"#,
        );

        // SAFETY: tests run in controlled process; we set and immediately restore env var.
        unsafe {
            std::env::set_var("CAMEL_TIMEOUT_MS", "9999");
        }

        let cfg = CamelConfig::from_file_with_env(file.path().to_str().unwrap())
            .expect("config should load with env override");
        assert_eq!(cfg.timeout_ms, 9999);

        // SAFETY: restore process env for test isolation.
        unsafe {
            std::env::remove_var("CAMEL_TIMEOUT_MS");
        }
    }

    #[test]
    fn test_from_file_resolves_placeholders_in_components_and_beans() {
        let file = write_temp_config(
            r#"
[default]
routes = ["{{env:RUST_CAMEL_TEST_ROUTE:routes/default.yaml}}"]

[default.components.http]
base_url = "{{env:RUST_CAMEL_TEST_BASE_URL:http://localhost:8080}}"

[default.beans.auth]
plugin = "{{env:RUST_CAMEL_TEST_PLUGIN:test-auth}}"

[default.beans.auth.config]
token = "{{env:RUST_CAMEL_TEST_TOKEN:abc123}}"
"#,
        );

        let cfg =
            CamelConfig::from_file(file.path().to_str().unwrap()).expect("config should load");

        assert_eq!(cfg.routes, vec!["routes/default.yaml"]);
        let http = cfg.components.raw.get("http").expect("http config");
        assert_eq!(
            http.get("base_url").and_then(|v| v.as_str()),
            Some("http://localhost:8080")
        );
        let bean = cfg.beans.get("auth").expect("bean auth");
        assert_eq!(bean.plugin, "test-auth");
        assert_eq!(bean.config.get("token").map(String::as_str), Some("abc123"));
    }

    #[test]
    fn test_from_file_unresolved_placeholder_keeps_original_string() {
        let file = write_temp_config(
            r#"
[default]
[default.components.redis]
url = "redis://{{MISSING_PLACEHOLDER}}"
"#,
        );

        let cfg =
            CamelConfig::from_file(file.path().to_str().unwrap()).expect("config should load");
        let redis = cfg.components.raw.get("redis").expect("redis config");
        assert_eq!(
            redis.get("url").and_then(|v| v.as_str()),
            Some("redis://{{MISSING_PLACEHOLDER}}")
        );
    }
}

#[cfg(test)]
mod additional_config_tests {
    use super::*;

    #[test]
    fn journal_durability_converts_to_core_type() {
        let immediate: camel_core::JournalDurability = JournalDurability::Immediate.into();
        let eventual: camel_core::JournalDurability = JournalDurability::Eventual.into();
        assert_eq!(immediate, camel_core::JournalDurability::Immediate);
        assert_eq!(eventual, camel_core::JournalDurability::Eventual);
    }

    #[test]
    fn supervision_into_supervision_config_converts_durations() {
        let input = SupervisionCamelConfig {
            max_attempts: Some(7),
            initial_delay_ms: 123,
            backoff_multiplier: 1.5,
            max_delay_ms: 999,
        };

        let out = input.into_supervision_config();
        assert_eq!(out.max_attempts, Some(7));
        assert_eq!(out.initial_delay, Duration::from_millis(123));
        assert_eq!(out.backoff_multiplier, 1.5);
        assert_eq!(out.max_delay, Duration::from_millis(999));
    }

    #[test]
    fn redb_journal_options_from_journal_config_copies_fields() {
        let cfg = JournalConfig {
            path: std::path::PathBuf::from("journal.db"),
            durability: JournalDurability::Eventual,
            compaction_threshold_events: 42,
        };

        let options: camel_core::RedbJournalOptions = (&cfg).into();
        assert_eq!(options.durability, camel_core::JournalDurability::Eventual);
        assert_eq!(options.compaction_threshold_events, 42);
    }

    #[test]
    fn from_env_or_default_uses_camel_config_file_env() {
        use std::io::Write;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(
            br#"
watch = true
timeout_ms = 111
"#,
        )
        .unwrap();

        unsafe {
            std::env::set_var("CAMEL_CONFIG_FILE", file.path());
        }
        let cfg = CamelConfig::from_env_or_default().unwrap();
        unsafe {
            std::env::remove_var("CAMEL_CONFIG_FILE");
        }

        assert!(cfg.watch);
        assert_eq!(cfg.timeout_ms, 111);
    }

    #[test]
    fn from_file_with_profile_and_env_unknown_profile_errors() {
        use std::io::Write;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(
            br#"
[default]
watch = false
"#,
        )
        .unwrap();

        let err = CamelConfig::from_file_with_profile_and_env(
            file.path().to_str().unwrap(),
            Some("missing"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Unknown profile: missing"));
    }
}

#[cfg(test)]
mod beans_config_tests {
    use super::*;

    #[test]
    fn beans_default_empty() {
        let config: CamelConfig = toml::from_str("").unwrap();
        assert!(config.beans.is_empty());
    }

    #[test]
    fn beans_parsed_from_config() {
        let toml_str = r#"
[beans.auth]
plugin = "my-auth"

[beans.cache]
plugin = "my-cache"
"#;
        let config: CamelConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.beans.len(), 2);
        assert_eq!(config.beans.get("auth").unwrap().plugin, "my-auth");
        assert_eq!(config.beans.get("cache").unwrap().plugin, "my-cache");
    }

    #[test]
    fn beans_config_parsed_from_toml() {
        let toml_str = r#"
[beans.auth]
plugin = "my-auth"
[beans.auth.config]
api_key = "${API_KEY}"
base_url = "https://api.example.com"
"#;
        let config: CamelConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.beans.len(), 1);
        let auth = config.beans.get("auth").unwrap();
        assert_eq!(auth.plugin, "my-auth");
        assert_eq!(auth.config.get("api_key").unwrap(), "${API_KEY}");
        assert_eq!(
            auth.config.get("base_url").unwrap(),
            "https://api.example.com"
        );
    }

    #[test]
    fn beans_config_defaults_to_empty_map() {
        let toml_str = r#"
[beans.auth]
plugin = "my-auth"
"#;
        let config: CamelConfig = toml::from_str(toml_str).unwrap();
        assert!(config.beans.get("auth").unwrap().config.is_empty());
    }

    #[test]
    fn beans_config_with_profiles_merges() {
        let toml_str = r#"
[default.beans.auth]
plugin = "my-auth"
[default.beans.auth.config]
base_url = "https://dev.example.com"

[production.beans.auth.config]
base_url = "https://prod.example.com"
"#;
        let config_value: toml::Value = toml::from_str(toml_str).unwrap();
        let default_val = config_value.get("default").cloned().unwrap();
        let prod_overlay = config_value.get("production").cloned().unwrap();
        let mut merged = default_val;
        super::merge_toml_values(&mut merged, &prod_overlay);
        let config: CamelConfig = merged.try_into().unwrap();
        let auth = config.beans.get("auth").unwrap();
        assert_eq!(
            auth.config.get("base_url").unwrap(),
            "https://prod.example.com"
        );
    }
}

#[cfg(test)]
mod config_validation_tests {
    use super::*;

    #[test]
    fn test_config_zero_timeout_rejected() {
        let config = CamelConfig {
            timeout_ms: 0,
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_zero_drain_timeout_rejected() {
        let config = CamelConfig {
            drain_timeout_ms: 0,
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_empty_journal_path_rejected() {
        let config = CamelConfig {
            runtime_journal: Some(JournalConfig {
                path: std::path::PathBuf::from(""),
                durability: JournalDurability::default(),
                compaction_threshold_events: 10_000,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_empty_bean_plugin_rejected() {
        let mut beans = HashMap::new();
        beans.insert(
            "my-bean".to_string(),
            BeanConfig {
                plugin: "".to_string(),
                config: HashMap::new(),
                limits: Default::default(),
            },
        );
        let config = CamelConfig {
            beans,
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_whitespace_bean_plugin_rejected() {
        let mut beans = HashMap::new();
        beans.insert(
            "my-bean".to_string(),
            BeanConfig {
                plugin: "   ".to_string(),
                config: HashMap::new(),
                limits: Default::default(),
            },
        );
        let config = CamelConfig {
            beans,
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_valid_defaults_pass() {
        let config = CamelConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_zero_watch_debounce_rejected() {
        let config = CamelConfig {
            watch_debounce_ms: 0,
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_zero_journal_compaction_threshold_rejected() {
        let config = CamelConfig {
            runtime_journal: Some(JournalConfig {
                path: std::path::PathBuf::from("/tmp/test.db"),
                durability: JournalDurability::default(),
                compaction_threshold_events: 0,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_zero_supervision_initial_delay_rejected() {
        let config = CamelConfig {
            supervision: Some(SupervisionCamelConfig {
                max_attempts: Some(5),
                initial_delay_ms: 0,
                backoff_multiplier: 2.0,
                max_delay_ms: 60000,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_zero_supervision_max_delay_rejected() {
        let config = CamelConfig {
            supervision: Some(SupervisionCamelConfig {
                max_attempts: Some(5),
                initial_delay_ms: 1000,
                backoff_multiplier: 2.0,
                max_delay_ms: 0,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_supervision_backoff_below_one_rejected() {
        let config = CamelConfig {
            supervision: Some(SupervisionCamelConfig {
                max_attempts: Some(5),
                initial_delay_ms: 1000,
                backoff_multiplier: 0.5,
                max_delay_ms: 60000,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_zero_otel_metrics_interval_rejected() {
        let mut otel = OtelCamelConfig::default();
        otel.metrics_interval_ms = 0;
        let config = CamelConfig {
            observability: ObservabilityConfig {
                otel: Some(otel),
                ..Default::default()
            },
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_kubernetes_zero_lease_duration_rejected() {
        let config = CamelConfig {
            platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
                namespace: None,
                lease_name_prefix: "camel-".to_string(),
                lease_duration_secs: 0,
                renew_deadline_secs: 10,
                retry_period_secs: 2,
                jitter_factor: 0.2,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_kubernetes_zero_renew_deadline_rejected() {
        let config = CamelConfig {
            platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
                namespace: None,
                lease_name_prefix: "camel-".to_string(),
                lease_duration_secs: 15,
                renew_deadline_secs: 0,
                retry_period_secs: 2,
                jitter_factor: 0.2,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_kubernetes_zero_retry_period_rejected() {
        let config = CamelConfig {
            platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
                namespace: None,
                lease_name_prefix: "camel-".to_string(),
                lease_duration_secs: 15,
                renew_deadline_secs: 10,
                retry_period_secs: 0,
                jitter_factor: 0.2,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_kubernetes_jitter_out_of_range_rejected() {
        let config = CamelConfig {
            platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
                namespace: None,
                lease_name_prefix: "camel-".to_string(),
                lease_duration_secs: 15,
                renew_deadline_secs: 10,
                retry_period_secs: 2,
                jitter_factor: 1.5,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_kubernetes_negative_jitter_rejected() {
        let config = CamelConfig {
            platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
                namespace: None,
                lease_name_prefix: "camel-".to_string(),
                lease_duration_secs: 15,
                renew_deadline_secs: 10,
                retry_period_secs: 2,
                jitter_factor: -0.1,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_valid_kubernetes_passes() {
        let config = CamelConfig {
            platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
                namespace: Some("default".to_string()),
                lease_name_prefix: "camel-".to_string(),
                lease_duration_secs: 15,
                renew_deadline_secs: 10,
                retry_period_secs: 2,
                jitter_factor: 0.2,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_valid_supervision_passes() {
        let config = CamelConfig {
            supervision: Some(SupervisionCamelConfig {
                max_attempts: Some(5),
                initial_delay_ms: 1000,
                backoff_multiplier: 2.0,
                max_delay_ms: 60000,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_valid_journal_passes() {
        let config = CamelConfig {
            runtime_journal: Some(JournalConfig {
                path: std::path::PathBuf::from("/tmp/test.db"),
                durability: JournalDurability::default(),
                compaction_threshold_events: 10_000,
            }),
            ..CamelConfig::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn bean_config_deserialises_with_limits() {
        let toml_str = r#"
        plugin = "my-plugin"
        [limits]
        timeout-secs = 600
        max-memory = 4294967296
        "#;
        let cfg: BeanConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(cfg.plugin, "my-plugin");
        assert_eq!(cfg.limits.timeout_secs, Some(600));
        assert_eq!(cfg.limits.max_memory, Some(4_294_967_296));
        assert_eq!(cfg.limits.max_concurrent_calls, None);
    }

    #[test]
    fn bean_config_defaults_limits_to_none() {
        let toml_str = r#"
        plugin = "my-plugin"
        "#;
        let cfg: BeanConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(cfg.limits, crate::wasm_limits::WasmLimitsConfig::default());
    }
}

#[cfg(test)]
mod security_config_tests {
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
            token_issuer: None,
            clients: Vec::new(),
        };

        let debug = format!("{native:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret-token"));
        assert!(!debug.contains("super-secret-key"));
    }

    #[test]
    fn parse_native_issuer_and_clients() {
        let toml = r#"
[security]
[security.native]
subject = "m2m-system"
[security.native.token_issuer]
issuer = "https://orders.local"
audience = ["orders-api"]
token_ttl_secs = 900
signing_key_env = "CAMEL_NATIVE_ISSUER_KEY_PEM"

[[security.native.clients]]
client_id = "billing-worker"
client_secret_env = "BILLING_CLIENT_SECRET"
roles = ["billing"]
scopes = ["orders:read", "orders:write"]

[[security.native.clients]]
client_id = "reporting-worker"
client_secret_env = "REPORTING_CLIENT_SECRET"
roles = ["reporting"]
scopes = ["orders:read"]
"#;
        let config: CamelConfig = toml::from_str(toml).unwrap();
        let native = config.security.native.as_ref().unwrap();
        let issuer = native.token_issuer.as_ref().unwrap();
        assert_eq!(issuer.issuer, "https://orders.local");
        assert_eq!(issuer.audience, vec!["orders-api"]);
        assert_eq!(issuer.token_ttl_secs, 900);
        assert_eq!(issuer.signing_key_env, "CAMEL_NATIVE_ISSUER_KEY_PEM");
        assert_eq!(native.clients.len(), 2);
        assert_eq!(native.clients[0].client_id, "billing-worker");
        assert_eq!(
            native.clients[0].scopes,
            vec!["orders:read", "orders:write"]
        );
    }

    #[test]
    fn parse_native_issuer_defaults() {
        let toml = r#"
[security]
[security.native]
subject = "m2m-system"
[security.native.token_issuer]
issuer = "https://test.local"
signing_key_env = "KEY"
"#;
        let config: CamelConfig = toml::from_str(toml).unwrap();
        let issuer = config.security.native.unwrap().token_issuer.unwrap();
        assert_eq!(issuer.token_ttl_secs, 900);
        assert!(issuer.audience.is_empty());
    }

    #[test]
    fn parse_native_debug_redacts_client_secret_env() {
        let toml = r#"
[security]
[security.native]
subject = "m2m-system"
[[security.native.clients]]
client_id = "test-worker"
client_secret_env = "MY_SECRET"
"#;
        let config: CamelConfig = toml::from_str(toml).unwrap();
        let debug = format!("{:?}", config.security.native.unwrap());
        assert!(!debug.contains("MY_SECRET"));
        assert!(debug.contains("[REDACTED]"));
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
}

#[cfg(test)]
mod config_builder_tests {
    use super::*;

    #[test]
    fn test_config_builder_sets_application_name() {
        let cfg = CamelConfigBuilder::default().log_level("debug").build();
        assert_eq!(cfg.log_level, "debug");
    }

    #[test]
    fn test_config_builder_default() {
        let built = CamelConfigBuilder::default().build();
        let default_cfg = CamelConfig::default();
        assert_eq!(built.routes, default_cfg.routes);
        assert_eq!(built.watch, default_cfg.watch);
        assert_eq!(built.log_level, default_cfg.log_level);
        assert_eq!(built.timeout_ms, default_cfg.timeout_ms);
        assert_eq!(built.drain_timeout_ms, default_cfg.drain_timeout_ms);
        assert_eq!(built.watch_debounce_ms, default_cfg.watch_debounce_ms);
    }
}

#[cfg(test)]
mod async_io_tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    #[tokio::test]
    async fn test_from_file_async_completes_without_blocking_executor() {
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        write!(
            f,
            r#"
[default]
watch = true
timeout_ms = 42
"#
        )
        .expect("write config");

        let path = f.path().to_str().unwrap().to_string();
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            CamelConfig::from_file_async(&path),
        )
        .await;

        assert!(
            result.is_ok(),
            "from_file_async should not block the executor"
        );
        let config = result.unwrap().expect("config should parse");
        assert!(config.watch);
        assert_eq!(config.timeout_ms, 42);
    }

    #[tokio::test]
    async fn test_from_file_async_with_profile_completes() {
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        write!(
            f,
            r#"
[default]
watch = false
timeout_ms = 1000

[prod]
watch = true
timeout_ms = 99
"#
        )
        .expect("write config");

        let path = f.path().to_str().unwrap().to_string();
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            CamelConfig::from_file_async_with_profile(&path, Some("prod")),
        )
        .await;

        assert!(
            result.is_ok(),
            "from_file_async_with_profile should not block"
        );
        let config = result.unwrap().expect("config should parse");
        assert!(config.watch);
        assert_eq!(config.timeout_ms, 99);
    }

    #[tokio::test]
    async fn test_from_file_async_with_env_completes() {
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        write!(
            f,
            r#"
[default]
timeout_ms = 1000
"#
        )
        .expect("write config");

        let path = f.path().to_str().unwrap().to_string();
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            CamelConfig::from_file_async_with_env(&path),
        )
        .await;

        assert!(result.is_ok(), "from_file_async_with_env should not block");
        let config = result.unwrap().expect("config should parse");
        assert_eq!(config.timeout_ms, 1000);
    }
}

#[cfg(test)]
mod permission_provider_config_tests {
    use super::*;

    #[test]
    fn permission_provider_config_deserialises_with_limits() {
        let toml_str = r#"
        provider = "wasm"
        path = "plugins/authz.wasm"
        [limits]
        timeout-secs = 5
        max-memory = 10485760
        "#;
        let cfg: PermissionProviderConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(cfg.provider, "wasm");
        assert_eq!(cfg.path.as_deref(), Some("plugins/authz.wasm"));
        assert_eq!(cfg.limits.timeout_secs, Some(5));
        assert_eq!(cfg.limits.max_memory, Some(10_485_760));
    }

    #[test]
    fn permission_provider_config_defaults_limits_to_none() {
        let toml_str = r#"
        provider = "keycloak"
        "#;
        let cfg: PermissionProviderConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(cfg.limits, crate::wasm_limits::WasmLimitsConfig::default());
    }
}
