use camel_api::CamelError;
use camel_api::datasource::DatasourceConfig;
use camel_core::MetricsLeversConfig;
use camel_core::TracerConfig;
use config::{Config, ConfigError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::time::Duration;

#[derive(Clone, Deserialize)]
pub struct CamelConfig {
    #[serde(default)]
    pub routes: Vec<String>,

    /// Enable file-watcher hot-reload. Defaults to false.
    /// Can be overridden per profile in Camel.toml or via `--watch` / `--no-watch` CLI flags.
    #[serde(default)]
    pub watch: bool,

    /// Optional redb runtime journal configuration.
    ///
    /// When unset, runtime state is ephemeral (in-memory only).
    #[serde(default)]
    pub runtime_journal: Option<JournalConfig>,

    /// Optional idempotent repository configuration.
    ///
    /// When unset, the in-memory idempotent repository is used (ephemeral,
    /// bounded by `DEFAULT_MAX_ENTRIES`). Set `backend = "redb"` (default)
    /// for a persistent on-disk store or `backend = "redis"` for a shared
    /// store.
    pub idempotent_repo: Option<IdempotentRepoConfig>,

    /// Optional cache repository configuration.
    ///
    /// When unset, the default in-memory cache repository is used (ephemeral,
    /// bounded by 10_000 entries). Set `backend = "redb"` to use a persistent
    /// on-disk store with background stale-entry sweep.
    pub cache_repo: Option<CacheRepoConfig>,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    #[serde(default = "default_drain_timeout_ms")]
    pub drain_timeout_ms: u64,

    /// Debounce window (ms) for the file-watcher hot-reload; consumed by `camel run`.
    #[serde(default = "default_watch_debounce_ms")]
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

    /// In-process scripting language limits (Rhai, Boa JS).
    /// See [`LanguagesConfig`] for details.
    #[serde(default)]
    pub languages: crate::LanguagesConfig,

    #[serde(default)]
    pub security: SecurityConfig,

    /// Per-bind public-exposure acknowledgements (ADR-0061).
    ///
    /// Keyed by bind address string (e.g. `"0.0.0.0:8080"`). A bind whose
    /// routes compile to `Public` access refuses to start on non-loopback
    /// addresses unless the operator acknowledges the exposure here.
    #[serde(default)]
    pub binds: HashMap<String, BindExposureConfig>,

    #[serde(default)]
    pub datasources: HashMap<String, DatasourceConfig>,

    /// Catch-all for extra keys injected by config sources (e.g. CAMEL_* env vars)
    /// or unknown fields in TOML. Nested structs use `deny_unknown_fields` to
    /// catch typos in sections like `[observability.health]`.
    ///
    /// When adding or removing top-level fields here, update
    /// [`KNOWN_TOP_LEVEL_KEYS`] in the same change: a new field missing from the
    /// const silently false-warns as an "unselected profile".
    #[serde(flatten)]
    pub _extra: HashMap<String, toml::Value>,
}

// Audit 2026-08-31, F5-5: manual Debug so a stray `debug!(?config)` cannot
// dump cleartext secrets. `_extra` collects UNKNOWN top-level keys (where
// operators plausibly stash credentials) and is redacted wholesale; the
// secrets-adjacent sub-structs already redact via their own manual impls.
impl fmt::Debug for CamelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CamelConfig")
            .field("routes", &self.routes)
            .field("watch", &self.watch)
            .field("runtime_journal", &self.runtime_journal)
            .field("idempotent_repo", &self.idempotent_repo)
            .field("cache_repo", &self.cache_repo)
            .field("log_level", &self.log_level)
            .field("timeout_ms", &self.timeout_ms)
            .field("drain_timeout_ms", &self.drain_timeout_ms)
            .field("watch_debounce_ms", &self.watch_debounce_ms)
            .field("components", &self.components)
            .field("observability", &self.observability)
            .field("supervision", &self.supervision)
            .field("platform", &self.platform)
            .field("stream_caching", &self.stream_caching)
            .field("beans", &self.beans)
            .field("languages", &self.languages)
            .field("security", &self.security)
            .field("binds", &self.binds)
            .field("datasources", &self.datasources)
            .field(
                "_extra",
                &format_args!("<{} keys redacted>", self._extra.len()),
            )
            .finish()
    }
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
            idempotent_repo: defaults.idempotent_repo,
            cache_repo: defaults.cache_repo,
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
            languages: defaults.languages,
            security: defaults.security,
            binds: defaults.binds,
            datasources: defaults.datasources,
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
            idempotent_repo: None,
            cache_repo: None,
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
            languages: crate::LanguagesConfig::default(),
            security: SecurityConfig::default(),
            binds: HashMap::new(),
            datasources: HashMap::new(),
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
    /// R4-L11: handler-level probe timeout in milliseconds. Default 6000 (6s).
    /// Must be > 0. Strictly > internal 5s registry tick.
    #[serde(default = "default_health_handler_timeout_ms")]
    pub handler_timeout_ms: u64,
    /// R4-L12: opt-in TTL (ms) for forced-unhealthy entries. When set, a forced
    /// entry whose age exceeds the TTL has its reason updated to indicate the
    /// TTL expired, but the entry is NOT cleared — TTL alone never declares
    /// Ready. Recovery still requires both a later probe generation AND a
    /// post-force Started marker. Default: None (disabled).
    #[serde(default)]
    pub forced_ttl_ms: Option<u64>,
}

impl Default for HealthCamelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_health_host(),
            port: default_health_port(),
            handler_timeout_ms: default_health_handler_timeout_ms(),
            forced_ttl_ms: None,
        }
    }
}

fn default_health_host() -> String {
    "0.0.0.0".to_string()
}

fn default_health_port() -> u16 {
    8081
}

fn default_health_handler_timeout_ms() -> u64 {
    6000
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityConfig {
    #[serde(default)]
    pub tracer: TracerConfig,

    /// Metric-family levers (`[observability.metrics]`). Absent table means
    /// all-default levers; the error family has no lever and is always on.
    #[serde(default)]
    pub metrics: MetricsLeversConfig,

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

/// Configuration for the idempotent repository.
///
/// Supports two backends: `"redb"` (persistent on-disk store, default) and
/// `"redis"` (persistent, shared across process restarts and instances).
///
/// When omitted from `Camel.toml`, the runtime uses the in-memory
/// `MemoryIdempotentRepository` (ephemeral, bounded by `DEFAULT_MAX_ENTRIES`).
/// Use this struct to opt into a durable store.
///
/// # Durability trade-off (redb backend)
///
/// `durability = "immediate"` (the default) fsyncs the redb file on every
/// added key, matching the runtime event journal's safety guarantee
/// (at-most-once semantics survive OS/power crash). For high-throughput
/// routes, set `durability = "eventual"` to skip fsync — accepted
/// degradation is at-least-once (a key added just before a crash may be
/// silently lost, allowing a duplicate replay).
///
/// # TOML example
///
/// ```toml
/// [default.idempotent_repo]
/// backend = "redis"
/// url = "redis://cache.internal:6379?db=2"
/// key_prefix = "camel:idem"
/// ```
#[derive(Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IdempotentRepoConfig {
    /// Idempotent backend: `"redb"` (default) or `"redis"` (persistent,
    /// shared).
    #[serde(default = "default_idempotent_backend")]
    pub backend: String,

    /// Path to the `.redb` file. Required when `backend = "redb"`.
    /// Created if it does not exist.
    #[serde(default)]
    pub path: Option<String>,

    /// Durability mode for the redb backend: `"immediate"` (default) or
    /// `"eventual"`. Ignored by the redis backend.
    #[serde(default)]
    pub durability: Option<String>,

    /// Standalone Redis endpoint URL (`redis://` or `rediss://`). Mutually
    /// exclusive with `sentinel_nodes`. Required for the redis backend
    /// unless `sentinel_nodes` is set. The database is selected with the
    /// `?db=N` query parameter (default 0); a `/N` path suffix is rejected.
    #[serde(default)]
    pub url: Option<String>,

    /// Sentinel node addresses (e.g. `["s-a:26379", "s-b:26379"]`). Mutually
    /// exclusive with `url`. Requires `master_name`.
    #[serde(default)]
    pub sentinel_nodes: Option<Vec<String>>,

    /// Master name to track in the Sentinel cluster. Only valid when
    /// `sentinel_nodes` is set.
    #[serde(default)]
    pub master_name: Option<String>,

    /// Username for Sentinel authentication. Only valid when
    /// `sentinel_nodes` is set.
    #[serde(default)]
    pub sentinel_username: Option<String>,

    /// Password for Sentinel authentication. Only valid when
    /// `sentinel_nodes` is set. Redacted from `Debug` output.
    #[serde(default)]
    pub sentinel_password: Option<String>,

    /// Password for data-node (master/replica) authentication in sentinel
    /// mode. Only valid when `sentinel_nodes` is set; rejected in `url`
    /// mode, where the credential rides the URL userinfo. Redacted from
    /// `Debug` output.
    #[serde(default)]
    pub password: Option<String>,

    /// Username for data-node (master/replica) authentication in sentinel
    /// mode. Only valid when `sentinel_nodes` is set; rejected in `url`
    /// mode, where the credential rides the URL userinfo.
    #[serde(default)]
    pub username: Option<String>,

    /// Redis database index (0..=16383) for the data nodes in sentinel
    /// mode. Only valid when `sentinel_nodes` is set; rejected in `url`
    /// mode, where the database rides the `?db=N` query parameter.
    /// Defaults to 0.
    #[serde(default)]
    pub db: Option<u16>,

    /// Redis key prefix for this repository's keyspace. Default:
    /// `"camel:idem"` (applied at the registration site, not by serde, so
    /// it stays consistent with the cache repository's `"camel:cache"`).
    /// Only `[A-Za-z0-9:_-]` are allowed (glob metacharacters are
    /// forbidden).
    #[serde(default)]
    pub key_prefix: Option<String>,
}

fn default_idempotent_backend() -> String {
    "redb".to_string()
}

/// Hand-written redacting `Debug`: URL userinfo is replaced by `***` and the
/// sentinel credentials render as `Some("***")` so credentials never reach
/// logs. All other fields keep the derived representation.
impl std::fmt::Debug for IdempotentRepoConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdempotentRepoConfig")
            .field("backend", &self.backend)
            .field("path", &self.path)
            .field("durability", &self.durability)
            .field("url", &self.url.as_deref().map(redact_url))
            .field("sentinel_nodes", &self.sentinel_nodes)
            .field("master_name", &self.master_name)
            .field(
                "sentinel_username",
                &self.sentinel_username.as_deref().map(|_| "***"),
            )
            .field(
                "sentinel_password",
                &self.sentinel_password.as_deref().map(|_| "***"),
            )
            .field("password", &self.password.as_deref().map(|_| "***"))
            .field("username", &self.username.as_deref().map(|_| "***"))
            .field("db", &self.db)
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

impl IdempotentRepoConfig {
    /// Normalize expanded-but-empty redis topology fields to absent.
    ///
    /// Thin delegate to [`normalize_empty_topology_fields`], which owns the
    /// shared contract with `validate_redis_topology_fields`.
    pub(crate) fn normalize_empty_topology(&mut self) {
        normalize_empty_topology_fields(
            &mut self.url,
            &mut self.sentinel_nodes,
            &mut self.master_name,
            &mut self.username,
            &mut self.sentinel_username,
            &mut self.key_prefix,
            &mut self.password,
            &mut self.sentinel_password,
        );
    }
}

/// Configuration for the cache repository.
///
/// Supports three backends: `"memory"` (in-process, bounded by
/// `max_capacity`, default), `"redb"` (persistent on-disk store with
/// background stale-entry sweep), and `"redis"` (persistent, shared across
/// processes).
///
/// When omitted from `Camel.toml`, the runtime uses the in-memory cache repository
/// (ephemeral, bounded by 10_000 entries). Use `backend = "redb"` to opt into
/// a durable on-disk store that survives restarts.
///
/// # TOML example
///
/// ```toml
/// [default.cache_repo]
/// backend = "redb"
/// path = "cache.redb"
/// cache_size = "256MiB"
/// stale_retention = "168h"
/// sweep_interval = "1h"
/// max_entries = 1_000_000
/// ```
/// Payload storage mode for the cache repository: `"inline"` keeps payload
/// bytes inside the repository entry, `"disk"` offloads payload bodies to
/// files under `cache_repo.payload_dir`. Disk mode applies to the redb and
/// redis backends; the memory backend rejects it. Exhaustive by contract
/// (closed 2-variant set, mirroring `ContentType`'s ADR-0049 exception
/// note) — not `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayloadMode {
    Inline,
    Disk,
}

#[derive(Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CacheRepoConfig {
    /// Cache backend: `"memory"` (default), `"redb"` (persistent), or
    /// `"redis"` (persistent, shared).
    #[serde(default = "default_cache_backend")]
    pub backend: String,

    /// Maximum entry count before eviction starts. Memory backend only.
    /// Default: 10_000.
    #[serde(default)]
    pub max_capacity: Option<usize>,

    /// Path to the `.redb` file. Required when `backend = "redb"`.
    /// Created if it does not exist.
    #[serde(default)]
    pub path: Option<String>,

    /// Maximum on-disk size of the redb cache file. Required when
    /// `backend = "redb"`. Accepts human-readable sizes like "256MiB",
    /// "384MB", or plain bytes (e.g. "268435456").
    #[serde(default)]
    pub cache_size: Option<String>,

    /// How long after expiry a stale entry stays readable. Redb: the sweep
    /// reclaims the entry after this window. Redis: the key expires at
    /// `expires_at + stale_retention`. Applies to the redb and redis
    /// backends. Accepts human-readable strings like "168h", "7d", "1w".
    /// When omitted, deserializes as `None`; the wiring then falls back to
    /// 7 days (168h).
    #[serde(default = "default_stale_retention")]
    pub stale_retention: Option<String>,

    /// How often the stale-entry sweep runs. Redb backend only. Accepts
    /// human-readable durations like "1h", "30m". Must be positive.
    #[serde(default)]
    pub sweep_interval: Option<String>,

    /// Payload storage mode: `"inline"` (default) keeps payload bytes in
    /// the repository entry; `"disk"` offloads payload bodies to files
    /// under `payload_dir`. Disk mode is rejected on the memory backend.
    #[serde(default)]
    pub payload: Option<PayloadMode>,

    /// Directory holding offloaded payload files. Required when
    /// `payload = "disk"`; rejected otherwise and on the memory backend.
    /// Supports `${env:}` strict interpolation.
    #[serde(default)]
    pub payload_dir: Option<String>,

    /// How often the offloaded-payload sweep runs when `payload = "disk"`.
    /// Accepts human-readable durations like "1h", "30m". Must be positive.
    #[serde(default)]
    pub payload_sweep_interval: Option<String>,

    /// Maximum time an offloaded payload file may outlive its cache entry
    /// when `payload = "disk"`. Accepts human-readable durations like
    /// "720h". Must be positive.
    #[serde(default)]
    pub payload_max_ttl: Option<String>,

    /// Maximum entry count for the redb backend. Default: 1_000_000.
    #[serde(default)]
    pub max_entries: Option<usize>,

    /// Standalone Redis endpoint URL (`redis://` or `rediss://`). Mutually
    /// exclusive with `sentinel_nodes`. Required for the redis backend unless
    /// `sentinel_nodes` is set. The database is selected with the `?db=N`
    /// query parameter (default 0); a `/N` path suffix is rejected.
    #[serde(default)]
    pub url: Option<String>,

    /// Sentinel node addresses (e.g. `["s-a:26379", "s-b:26379"]`). Mutually
    /// exclusive with `url`. Requires `master_name`.
    #[serde(default)]
    pub sentinel_nodes: Option<Vec<String>>,

    /// Master name to track in the Sentinel cluster. Only valid when
    /// `sentinel_nodes` is set.
    #[serde(default)]
    pub master_name: Option<String>,

    /// Username for Sentinel authentication. Only valid when `sentinel_nodes`
    /// is set.
    #[serde(default)]
    pub sentinel_username: Option<String>,

    /// Password for Sentinel authentication. Only valid when `sentinel_nodes`
    /// is set. Redacted from `Debug` output.
    #[serde(default)]
    pub sentinel_password: Option<String>,

    /// Password for data-node (master/replica) authentication in sentinel
    /// mode. Only valid when `sentinel_nodes` is set; rejected in `url`
    /// mode, where the credential rides the URL userinfo. Redacted from
    /// `Debug` output.
    #[serde(default)]
    pub password: Option<String>,

    /// Username for data-node (master/replica) authentication in sentinel
    /// mode. Only valid when `sentinel_nodes` is set; rejected in `url`
    /// mode, where the credential rides the URL userinfo.
    #[serde(default)]
    pub username: Option<String>,

    /// Redis database index (0..=16383) for the data nodes in sentinel
    /// mode. Only valid when `sentinel_nodes` is set; rejected in `url`
    /// mode, where the database rides the `?db=N` query parameter.
    /// Defaults to 0.
    #[serde(default)]
    pub db: Option<u16>,

    /// Redis key prefix for this repository's keyspace. Default: `"camel:cache"`.
    /// Only `[A-Za-z0-9:_-]` are allowed (glob metacharacters are forbidden).
    #[serde(default)]
    pub key_prefix: Option<String>,
}

fn default_cache_backend() -> String {
    "memory".to_string()
}

fn default_stale_retention() -> Option<String> {
    None
}

impl CacheRepoConfig {
    /// Offload wiring durations: `(payload_sweep_interval, payload_max_ttl)`
    /// parsed with humantime, defaulting to 1h and 720h when unset.
    ///
    /// Only the `payload = "disk"` wiring consumes these. `validate()`
    /// (fail-closed) already rejected malformed or non-positive values on
    /// that path, so the fallback covers genuinely unset fields only.
    pub(crate) fn payload_durations(&self) -> (Duration, Duration) {
        const DEFAULT_SWEEP: Duration = Duration::from_secs(3600);
        const DEFAULT_MAX_TTL: Duration = Duration::from_secs(720 * 3600);
        let sweep = self
            .payload_sweep_interval
            .as_deref()
            .and_then(|s| humantime::parse_duration(s).ok())
            .unwrap_or(DEFAULT_SWEEP);
        let max_ttl = self
            .payload_max_ttl
            .as_deref()
            .and_then(|s| humantime::parse_duration(s).ok())
            .unwrap_or(DEFAULT_MAX_TTL);
        (sweep, max_ttl)
    }

    /// Normalize expanded-but-empty redis topology fields to absent.
    ///
    /// Thin delegate to [`normalize_empty_topology_fields`], which owns the
    /// shared contract with `validate_redis_topology_fields`.
    pub(crate) fn normalize_empty_topology(&mut self) {
        normalize_empty_topology_fields(
            &mut self.url,
            &mut self.sentinel_nodes,
            &mut self.master_name,
            &mut self.username,
            &mut self.sentinel_username,
            &mut self.key_prefix,
            &mut self.password,
            &mut self.sentinel_password,
        );
    }
}

/// Parse a human-readable byte size (used by `cache_repo.cache_size`).
///
/// Suffixes are case-insensitive and no space may separate the number from the
/// suffix: `b`, `kb`, `kib`, `mb`, `mib`, `gb`, `gib`. `kb`/`mb`/`gb` are
/// decimal powers of 1000; `kib`/`mib`/`gib` are binary powers of 1024. A bare
/// number counts as plain bytes. Returns `Err` naming the field on invalid
/// input or when the value does not fit in `usize`.
pub(crate) fn parse_byte_size(s: &str) -> Result<usize, String> {
    const SUFFIXES: [(&str, u128); 7] = [
        ("gib", 1_073_741_824),
        ("gb", 1_000_000_000),
        ("mib", 1_048_576),
        ("mb", 1_000_000),
        ("kib", 1_024),
        ("kb", 1_000),
        ("b", 1),
    ];

    let trimmed = s.trim();
    let lower = trimmed.to_ascii_lowercase();

    for (suffix, factor) in SUFFIXES {
        if let Some(num) = lower.strip_suffix(suffix) {
            let value = num
                .parse::<u128>()
                .map_err(|_| format!("cache_repo.cache_size: invalid byte size '{trimmed}'"))?;
            let bytes = value.checked_mul(factor).ok_or_else(|| {
                format!("cache_repo.cache_size: overflow in byte size '{trimmed}'")
            })?;
            return usize::try_from(bytes)
                .map_err(|_| format!("cache_repo.cache_size: overflow in byte size '{trimmed}'"));
        }
    }

    // No suffix: treat the whole input as plain bytes.
    let value = trimmed
        .parse::<u128>()
        .map_err(|_| format!("cache_repo.cache_size: invalid byte size '{trimmed}'"))?;
    usize::try_from(value)
        .map_err(|_| format!("cache_repo.cache_size: overflow in byte size '{trimmed}'"))
}

/// Redis topology validation matrix shared by `cache_repo` and
/// `idempotent_repo` (task 2.5 rules). `field` is the config path prefix
/// ("cache_repo" / "idempotent_repo") so error messages name the offending
/// repository and the two validation branches cannot drift.
#[allow(clippy::too_many_arguments)] // flat field list mirrors the config surface
pub(crate) fn validate_redis_topology_fields(
    field: &str,
    url: Option<&str>,
    sentinel_nodes: Option<&[String]>,
    master_name: Option<&str>,
    sentinel_username: Option<&str>,
    sentinel_password: Option<&str>,
    password: Option<&str>,
    username: Option<&str>,
    db: Option<u16>,
    key_prefix: Option<&str>,
) -> Result<(), CamelError> {
    let has_url = url.is_some();
    let has_nodes = sentinel_nodes.is_some();
    if !has_url && !has_nodes {
        return Err(CamelError::Config(format!(
            "{field}.url: redis backend requires a topology: set url or sentinel_nodes"
        )));
    }
    if has_url && has_nodes {
        return Err(CamelError::Config(format!(
            "{field}.url: url and sentinel_nodes are mutually exclusive"
        )));
    }
    if let Some(nodes) = sentinel_nodes {
        if nodes.is_empty() || nodes.iter().any(|n| n.trim().is_empty()) {
            return Err(CamelError::Config(format!(
                "{field}.sentinel_nodes: sentinel node entries must be non-empty"
            )));
        }
        let master_empty = match master_name {
            None => true,
            Some(m) => m.trim().is_empty(),
        };
        if master_empty {
            return Err(CamelError::Config(format!(
                "{field}.master_name must be set when sentinel_nodes is set"
            )));
        }
        if let Some(d) = db
            && d > 16383
        {
            return Err(CamelError::Config(format!(
                "{field}.db: sentinel data-node db must be at most 16383, got {d}"
            )));
        }
    } else {
        if master_name.is_some() {
            return Err(CamelError::Config(format!(
                "{field}.master_name: only applies when sentinel_nodes is set"
            )));
        }
        if sentinel_username.is_some() {
            return Err(CamelError::Config(format!(
                "{field}.sentinel_username: only applies when sentinel_nodes is set"
            )));
        }
        if sentinel_password.is_some() {
            return Err(CamelError::Config(format!(
                "{field}.sentinel_password: only applies when sentinel_nodes is set"
            )));
        }
        if password.is_some() {
            return Err(CamelError::Config(format!(
                // Literal field path in the message, not a secret value.
                "{field}.password: only applies when sentinel_nodes is set" // allow-secret
            )));
        }
        if username.is_some() {
            return Err(CamelError::Config(format!(
                "{field}.username: only applies when sentinel_nodes is set"
            )));
        }
        if db.is_some() {
            return Err(CamelError::Config(format!(
                "{field}.db: only applies when sentinel_nodes is set"
            )));
        }
    }
    if let Some(url) = url {
        if !url.starts_with("redis://") && !url.starts_with("rediss://") {
            return Err(CamelError::Config(format!(
                "{field}.url: only \"redis://\" and \"rediss://\" schemes are allowed, got '{url}'"
            )));
        }
        // Deep-parse with the same component URI parser used at registration
        // so grammar failures (e.g. a db number in the path — the dialect
        // takes `?db=N`, not `/N`) surface at validate() time with the
        // identical message registration would produce. Sentinel nodes get
        // only the trim/format checks above; registration reports the rest.
        camel_redis_repo::RedisEndpointConfig::from_uri(url)
            .map_err(|e| CamelError::Config(format!("{field}.url: {e}")))?;
    }
    if let Some(prefix) = key_prefix {
        camel_redis_repo::keyspace::validate_namespace_token(
            &format!("{field}.key_prefix"),
            prefix,
        )?;
    }
    Ok(())
}

/// Normalize empty redis topology values to absent, shared by `cache_repo`
/// and `idempotent_repo` (FR1 of deployment-resolvable-cache-repo-topology).
///
/// Shared contract with [`validate_redis_topology_fields`]: the normalizer
/// removes exactly the values the validator would treat as present-but-empty,
/// so its `Option::is_some()` predicates see clean absence after
/// `${env:}` expansion. Keeping the two functions adjacent prevents them
/// drifting — a field added to the validator's topology matrix must be
/// considered here too. Rules: a `Some(s)` string becomes `None` when
/// `s` is whitespace-only; `sentinel_nodes` becomes `None` when the array is
/// empty or every entry trims empty. Mixed blank/non-blank arrays stay
/// untouched — the validator rejects them loudly. Passwords follow the same
/// blank-to-absent rule as the other string fields: a blank string is not a
/// credential — an expanded-empty `${env:PW:-}` placeholder means "unset" —
/// and leaving it in place would wrongly trip the validator's
/// `only applies when sentinel_nodes is set` checks. Non-blank credentials
/// are never dropped. `db` remains deliberately not a parameter: it is
/// typed (`Option<u16>`) so it cannot carry an empty value.
#[allow(clippy::too_many_arguments)] // flat field list mirrors the config surface
fn normalize_empty_topology_fields(
    url: &mut Option<String>,
    sentinel_nodes: &mut Option<Vec<String>>,
    master_name: &mut Option<String>,
    username: &mut Option<String>,
    sentinel_username: &mut Option<String>,
    key_prefix: &mut Option<String>,
    password: &mut Option<String>,
    sentinel_password: &mut Option<String>,
) {
    for field in [
        url,
        master_name,
        username,
        sentinel_username,
        key_prefix,
        password,
        sentinel_password,
    ] {
        if field.as_deref().is_some_and(|v| v.trim().is_empty()) {
            *field = None;
        }
    }
    if let Some(nodes) = sentinel_nodes
        && (nodes.is_empty() || nodes.iter().all(|n| n.trim().is_empty()))
    {
        *sentinel_nodes = None;
    }
}

/// Canonical redis database identity for the cross-repository
/// prefix-collision rule: standalone endpoints compare by host/port/db,
/// sentinel topologies by (sorted, trimmed, scheme-normalized) node set +
/// master + db. Returns `None` when normalization fails; the caller
/// compares `Option`s directly so `None == None` claims collision, but
/// `validate()` makes this unreachable — a validated endpoint always yields `Some`.
fn redis_database_key(
    url: Option<&str>,
    sentinel_nodes: Option<&[String]>,
    master_name: Option<&str>,
    db: Option<u16>,
) -> Option<String> {
    if let Some(url) = url {
        let endpoint = camel_redis_repo::RedisEndpointConfig::from_uri(url).ok()?;
        // `redis://h` and `redis://h:6379` name the same database: fold the
        // default port and case-fold the host so spelling variants of one
        // endpoint produce the same key instead of silently bypassing the
        // collision rule.
        let host = endpoint.host.unwrap_or_default().to_lowercase();
        let port = endpoint.port.unwrap_or(6379);
        return Some(format!("standalone|{host}|{port}|{}", endpoint.db));
    }
    let nodes = sentinel_nodes?;
    let mut normalized: Vec<String> = nodes
        .iter()
        .map(|n| n.trim().trim_start_matches("redis://").to_string())
        .collect();
    normalized.sort();
    let master = master_name?.trim();
    // Sentinel topologies select the data-node database via `db` (default 0):
    // same nodes + master but different databases are distinct keyspaces.
    Some(format!(
        "sentinel|{}|{master}|{}",
        normalized.join(","),
        db.unwrap_or(0)
    ))
}

#[cfg(test)]
#[path = "config_tests/byte_size_tests.rs"]
mod byte_size_tests;

#[cfg(test)]
#[path = "config_tests/camel_config_defaults_tests.rs"]
mod camel_config_defaults_tests;

#[cfg(test)]
#[path = "config_tests/components_config_tests.rs"]
mod components_config_tests;

#[cfg(test)]
#[path = "config_tests/prometheus_config_tests.rs"]
mod prometheus_config_tests;

#[cfg(test)]
#[path = "config_tests/platform_config_tests.rs"]
mod platform_config_tests;

#[cfg(test)]
#[path = "config_tests/profile_loading_tests.rs"]
mod profile_loading_tests;

#[cfg(test)]
#[path = "config_tests/additional_config_tests.rs"]
mod additional_config_tests;

#[cfg(test)]
#[path = "config_tests/beans_config_tests.rs"]
mod beans_config_tests;

#[cfg(test)]
#[path = "config_tests/config_validation_tests.rs"]
mod config_validation_tests;

#[cfg(test)]
#[path = "config_tests/security_config_tests.rs"]
mod security_config_tests;

#[cfg(test)]
#[path = "config_tests/placeholder_tests.rs"]
mod placeholder_tests;

#[cfg(test)]
#[path = "config_tests/native_credentials_tests.rs"]
mod native_credentials_tests;

#[cfg(test)]
#[path = "config_tests/stale_native_tests.rs"]
mod stale_native_tests;

#[cfg(test)]
#[path = "config_tests/config_builder_tests.rs"]
mod config_builder_tests;

#[cfg(test)]
#[path = "config_tests/async_io_tests.rs"]
mod async_io_tests;

#[cfg(test)]
#[path = "config_tests/permission_provider_config_tests.rs"]
mod permission_provider_config_tests;

#[cfg(test)]
#[path = "config_tests/languages_config_integration_tests.rs"]
mod languages_config_integration_tests;

#[cfg(test)]
#[path = "config_tests/oversized_file_tests.rs"]
mod oversized_file_tests;

/// Config ergonomics batch (rc-k5bz / rc-6gqy / rc-cflo): bare-filename
/// include resolution, handled-elsewhere env var exemptions, and the
/// unset-CAMEL_PROFILE warn.
#[cfg(test)]
#[path = "config_tests/config_ergonomics_tests.rs"]
mod config_ergonomics_tests;

/// FR1 of deployment-resolvable-cache-repo-topology: empty redis topology
/// values (`""`, whitespace-only, all-blank sentinel arrays — typically from
/// `${env:VAR:-}` expanding with the var unset) normalize to absent after
/// placeholder expansion and before validation, so deployments can express
/// "unset" through environment-driven templates without tripping mutual
/// exclusion or topology-required errors.
#[cfg(test)]
#[path = "config_tests/empty_topology_normalization_tests.rs"]
mod empty_topology_normalization_tests;

/// FR2 of deployment-resolvable-cache-repo-topology: non-credential
/// `cache_repo` fields become overridable per deployment via
/// `CAMEL_CACHE_REPO_*` env vars. Scalars coerce as today; `SENTINEL_NODES`
/// is the only CSV (list-typed) override; an EMPTY scalar override is a no-op
/// while an EMPTY CSV override clears the field. Credential vars stay denied
/// by the allowlist.
#[cfg(test)]
#[path = "config_tests/cache_repo_env_override_tests.rs"]
mod cache_repo_env_override_tests;
impl Default for CacheRepoConfig {
    fn default() -> Self {
        Self {
            backend: default_cache_backend(),
            max_capacity: None,
            path: None,
            cache_size: None,
            stale_retention: None,
            sweep_interval: None,
            payload: None,
            payload_dir: None,
            payload_sweep_interval: None,
            payload_max_ttl: None,
            max_entries: None,
            url: None,
            sentinel_nodes: None,
            master_name: None,
            sentinel_username: None,
            sentinel_password: None,
            password: None,
            username: None,
            db: None,
            key_prefix: None,
        }
    }
}

/// Replace URL userinfo with the literal `***`, keeping scheme, host, port,
/// path, and query verbatim (`redis://user:secret@h:6379/0` →
/// `redis://***@h:6379/0`). URLs without userinfo pass through unchanged.
/// An `@` after the first `/` (path or query data) is not userinfo and stays.
fn redact_url(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://")
        && let Some(at) = rest.find('@')
        && !rest[..at].contains('/')
    {
        return format!("{scheme}://***@{}", &rest[at + 1..]);
    }
    url.to_string()
}

/// Hand-written redacting `Debug`: URL userinfo is replaced by `***` and the
/// sentinel credentials render as `Some("***")` so credentials never reach
/// logs. All other fields keep the derived representation.
impl std::fmt::Debug for CacheRepoConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheRepoConfig")
            .field("backend", &self.backend)
            .field("max_capacity", &self.max_capacity)
            .field("path", &self.path)
            .field("cache_size", &self.cache_size)
            .field("stale_retention", &self.stale_retention)
            .field("sweep_interval", &self.sweep_interval)
            .field("payload", &self.payload)
            .field("payload_dir", &self.payload_dir)
            .field("payload_sweep_interval", &self.payload_sweep_interval)
            .field("payload_max_ttl", &self.payload_max_ttl)
            .field("max_entries", &self.max_entries)
            .field("url", &self.url.as_deref().map(redact_url))
            .field("sentinel_nodes", &self.sentinel_nodes)
            .field("master_name", &self.master_name)
            .field(
                "sentinel_username",
                &self.sentinel_username.as_deref().map(|_| "***"),
            )
            .field(
                "sentinel_password",
                &self.sentinel_password.as_deref().map(|_| "***"),
            )
            .field("password", &self.password.as_deref().map(|_| "***"))
            .field("username", &self.username.as_deref().map(|_| "***"))
            .field("db", &self.db)
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
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

#[derive(Clone, Deserialize, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BeanConfig {
    pub plugin: String,
    /// Arbitrary plugin key/value map — MAY carry credentials (F5-5);
    /// redacted in Debug.
    #[serde(default)]
    pub config: HashMap<String, String>,
    /// WASM runtime limits for this bean. All `None` by default — runtime
    /// defaults apply. See `WasmLimitsConfig`.
    #[serde(default)]
    pub limits: crate::wasm_limits::WasmLimitsConfig,
}

// F5-5: `config` is an arbitrary plugin map that may hold credentials.
impl fmt::Debug for BeanConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BeanConfig")
            .field("plugin", &self.plugin)
            .field(
                "config",
                &format_args!("<{} keys redacted>", self.config.len()),
            )
            .field("limits", &self.limits)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Security configuration (Keycloak etc.)
// ---------------------------------------------------------------------------

/// Per-bind public-exposure acknowledgement (ADR-0061). One entry under
/// `[binds."<bind-address>"]` in `Camel.toml`.
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BindExposureConfig {
    /// Operator acknowledges that Public (unauthenticated) routes will be
    /// exposed on this non-loopback bind. Emits a permanent warning at
    /// startup; never silences it.
    #[serde(default)]
    pub allow_public_exposure: bool,
}

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

/// A single credential entry under `[[security.native.credentials]]`.
///
/// Each entry binds a `subject` to a credential supplied either inline
/// (`secret`) or by reference to an environment variable name (`secret_env`).
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NativeCredentialEntry {
    pub subject: String,
    pub secret_env: Option<String>,
    pub secret: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl fmt::Debug for NativeCredentialEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeCredentialEntry")
            .field("subject", &self.subject)
            .field(
                "secret_env",
                &self.secret_env.as_ref().map(|_| "[REDACTED]"),
            )
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
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
    pub credentials: Vec<NativeCredentialEntry>,
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
            .field("credentials", &self.credentials.len())
            .finish()
    }
}

impl NativeAuthConfig {
    /// Validate the `credentials` array: every entry must set exactly one of
    /// `secret_env` / `secret` and carry a non-empty `subject`.
    pub fn validate_credentials(&self) -> Result<(), ConfigError> {
        for (i, cred) in self.credentials.iter().enumerate() {
            if cred.secret_env.is_some() == cred.secret.is_some() {
                return Err(ConfigError::Message(format!(
                    // allow-secret: names the credential field, not a secret value
                    "security.native.credentials[{i}] must set exactly one of secret_env or secret"
                )));
            }
            if cred.subject.trim().is_empty() {
                return Err(ConfigError::Message(format!(
                    "security.native.credentials[{i}] must have a non-empty subject"
                )));
            }
        }
        Ok(())
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
    /// Allow internal/private/loopback addresses and HTTP for local development.
    /// Default: false. Set to true only for local Keycloak instances.
    #[serde(default)]
    pub allow_internal: bool,
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
            .field("allow_internal", &self.allow_internal)
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

/// Maximum size (in bytes) for config files read via `read_capped`.
/// Prevents OOM from abnormally large config/route files.
pub(crate) const MAX_CONFIG_FILE_SIZE: u64 = 16 * 1024 * 1024;

/// Read a file with a size cap. Stats the file first, rejects if too large.
pub(crate) fn read_capped(path: &str, max_bytes: u64) -> Result<String, ConfigError> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| ConfigError::Message(format!("Cannot stat `{path}`: {e}")))?;
    if metadata.len() > max_bytes {
        return Err(ConfigError::Message(format!(
            "Config file `{path}` is {} bytes, exceeds max {} bytes",
            metadata.len(),
            max_bytes
        )));
    }
    std::fs::read_to_string(path)
        .map_err(|e| ConfigError::Message(format!("Cannot read `{path}`: {e}")))
}

/// Async variant of [`read_capped`] — uses `spawn_blocking` so the stat + read
/// does not block the async executor. Config loading is startup-only and files
/// are always under the cap in normal use, so the blocking cost is negligible.
async fn read_capped_async(path: &str, max_bytes: u64) -> Result<String, ConfigError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || read_capped(&path, max_bytes))
        .await
        .map_err(|e| ConfigError::Message(format!("spawn_blocking join error: {e}")))?
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
        if let Some(ref repo) = self.idempotent_repo {
            match repo.backend.as_str() {
                "redb" | "redis" => {}
                other => {
                    return Err(CamelError::Config(format!(
                        "idempotent_repo.backend must be \"redb\" or \"redis\", got \"{other}\""
                    )));
                }
            }
            if repo.backend == "redb" {
                if repo.url.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.url does not apply to the \"redb\" backend".to_string(),
                    ));
                }
                if repo.sentinel_nodes.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.sentinel_nodes does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                if repo.master_name.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.master_name does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                if repo.sentinel_username.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.sentinel_username does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                if repo.sentinel_password.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.sentinel_password does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                if repo.password.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.password does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                if repo.username.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.username does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                if repo.db.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.db does not apply to the \"redb\" backend".to_string(),
                    ));
                }
                if repo.key_prefix.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.key_prefix does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                let path_empty = match repo.path.as_deref() {
                    None => true,
                    Some(p) => p.is_empty(),
                };
                if path_empty {
                    return Err(CamelError::Config(
                        "idempotent_repo.path must not be empty".to_string(),
                    ));
                }
                if let Some(d) = repo.durability.as_deref()
                    && d != "immediate"
                    && d != "eventual"
                {
                    return Err(CamelError::Config(format!(
                        "idempotent_repo.durability must be \"immediate\" or \"eventual\", got \"{d}\""
                    )));
                }
            }
            if repo.backend == "redis" {
                if repo.path.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.path does not apply to the \"redis\" backend".to_string(),
                    ));
                }
                if repo.durability.is_some() {
                    return Err(CamelError::Config(
                        "idempotent_repo.durability does not apply to the \"redis\" backend"
                            .to_string(),
                    ));
                }
                validate_redis_topology_fields(
                    "idempotent_repo",
                    repo.url.as_deref(),
                    repo.sentinel_nodes.as_deref(),
                    repo.master_name.as_deref(),
                    repo.sentinel_username.as_deref(),
                    repo.sentinel_password.as_deref(),
                    repo.password.as_deref(),
                    repo.username.as_deref(),
                    repo.db,
                    repo.key_prefix.as_deref(),
                )?;
            }
        }
        if let Some(ref cache) = self.cache_repo {
            match cache.backend.as_str() {
                "memory" | "redb" | "redis" => {}
                other => {
                    return Err(CamelError::Config(format!(
                        "cache_repo.backend must be \"memory\", \"redb\", or \"redis\", got \"{other}\""
                    )));
                }
            }
            if cache.backend == "memory" {
                if cache.path.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.path does not apply to the \"memory\" backend".to_string(),
                    ));
                }
                if cache.cache_size.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.cache_size does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.stale_retention.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.stale_retention does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.sweep_interval.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.sweep_interval does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.payload == Some(PayloadMode::Disk) {
                    return Err(CamelError::Config(
                        "cache_repo.payload = \"disk\" does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.payload_dir.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.payload_dir does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.payload_sweep_interval.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.payload_sweep_interval does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.payload_max_ttl.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.payload_max_ttl does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.max_entries.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.max_entries does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.url.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.url does not apply to the \"memory\" backend".to_string(),
                    ));
                }
                if cache.sentinel_nodes.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.sentinel_nodes does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.master_name.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.master_name does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.sentinel_username.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.sentinel_username does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.sentinel_password.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.sentinel_password does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
                if cache.password.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.password does not apply to the \"memory\" backend".to_string(),
                    ));
                }
                if cache.username.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.username does not apply to the \"memory\" backend".to_string(),
                    ));
                }
                if cache.db.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.db does not apply to the \"memory\" backend".to_string(),
                    ));
                }
                if cache.key_prefix.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.key_prefix does not apply to the \"memory\" backend"
                            .to_string(),
                    ));
                }
            }
            // Duration parse check shared by every non-memory backend that
            // carries the field (redb and redis): malformed values fail at
            // validate() time, never silently at build time.
            if cache.backend != "memory"
                && let Some(stale) = cache.stale_retention.as_deref()
            {
                humantime::parse_duration(stale).map_err(|_| {
                    CamelError::Config(format!(
                        "cache_repo.stale_retention: invalid duration '{stale}' — use a unit-bearing form such as '7d' or '24h'"
                    ))
                })?;
            }
            // Disk-offload payload matrix (fail-closed), shared by the redb
            // and redis backends: the memory branch above has already
            // rejected every payload field, so only the two persistent
            // backends reach this block.
            if cache.backend != "memory" {
                if cache.payload == Some(PayloadMode::Disk) {
                    let dir_empty = match cache.payload_dir.as_deref() {
                        None => true,
                        Some(d) => d.is_empty(),
                    };
                    if dir_empty {
                        return Err(CamelError::Config(
                            "cache_repo.payload_dir must be set when payload = \"disk\""
                                .to_string(),
                        ));
                    }
                    if let Some(sweep) = cache.payload_sweep_interval.as_deref() {
                        let parsed = humantime::parse_duration(sweep).map_err(|_| {
                            CamelError::Config(format!(
                                "cache_repo.payload_sweep_interval: invalid duration '{sweep}'"
                            ))
                        })?;
                        // Sub-second intervals are rejected: the blob
                        // death epoch truncates to whole seconds, so a
                        // sub-second sweep could collapse the grace to zero
                        // (a live index row out-living its blob).
                        if parsed < Duration::from_secs(1) {
                            return Err(CamelError::Config(
                                "cache_repo.payload_sweep_interval must be at least one second"
                                    .to_string(),
                            ));
                        }
                    }
                    if let Some(ttl) = cache.payload_max_ttl.as_deref() {
                        let parsed = humantime::parse_duration(ttl).map_err(|_| {
                            CamelError::Config(format!(
                                "cache_repo.payload_max_ttl: invalid duration '{ttl}'"
                            ))
                        })?;
                        if parsed < Duration::from_secs(1) {
                            return Err(CamelError::Config(
                                "cache_repo.payload_max_ttl must be at least one second"
                                    .to_string(),
                            ));
                        }
                    }
                } else {
                    // Inline/unset mode: no payload field may carry a value.
                    if cache.payload_dir.is_some() {
                        return Err(CamelError::Config(
                            "cache_repo.payload_dir requires payload = \"disk\"".to_string(),
                        ));
                    }
                    if cache.payload_sweep_interval.is_some() {
                        return Err(CamelError::Config(
                            "cache_repo.payload_sweep_interval requires payload = \"disk\""
                                .to_string(),
                        ));
                    }
                    if cache.payload_max_ttl.is_some() {
                        return Err(CamelError::Config(
                            "cache_repo.payload_max_ttl requires payload = \"disk\"".to_string(),
                        ));
                    }
                }
            }
            if cache.backend == "redb" {
                if cache.max_capacity.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.max_capacity does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                if cache.url.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.url does not apply to the \"redb\" backend".to_string(),
                    ));
                }
                if cache.sentinel_nodes.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.sentinel_nodes does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                if cache.master_name.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.master_name does not apply to the \"redb\" backend".to_string(),
                    ));
                }
                if cache.sentinel_username.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.sentinel_username does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                if cache.sentinel_password.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.sentinel_password does not apply to the \"redb\" backend"
                            .to_string(),
                    ));
                }
                if cache.password.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.password does not apply to the \"redb\" backend".to_string(),
                    ));
                }
                if cache.username.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.username does not apply to the \"redb\" backend".to_string(),
                    ));
                }
                if cache.db.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.db does not apply to the \"redb\" backend".to_string(),
                    ));
                }
                if cache.key_prefix.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.key_prefix does not apply to the \"redb\" backend".to_string(),
                    ));
                }
                let path_empty = match &cache.path {
                    None => true,
                    Some(p) => p.is_empty(),
                };
                if path_empty {
                    return Err(CamelError::Config(
                        "cache_repo.path must be set when backend is \"redb\"".to_string(),
                    ));
                }
                let cache_size = cache.cache_size.as_deref().ok_or_else(|| {
                    CamelError::Config(
                        "cache_repo.cache_size must be set when backend is \"redb\" (e.g. \"256MiB\", \"384MB\", plain bytes)"
                            .to_string(),
                    )
                })?;
                parse_byte_size(cache_size).map_err(CamelError::Config)?;
                if let Some(sweep) = cache.sweep_interval.as_deref() {
                    let parsed = humantime::parse_duration(sweep).map_err(|_| {
                        CamelError::Config(format!(
                            "cache_repo.sweep_interval: invalid duration '{sweep}' — use a unit-bearing form such as '7d' or '24h'"
                        ))
                    })?;
                    if parsed.is_zero() {
                        return Err(CamelError::Config(
                            "cache_repo.sweep_interval must be positive (greater than zero)"
                                .to_string(),
                        ));
                    }
                }
            }
            if cache.backend == "redis" {
                if cache.path.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.path does not apply to the \"redis\" backend".to_string(),
                    ));
                }
                if cache.cache_size.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.cache_size does not apply to the \"redis\" backend".to_string(),
                    ));
                }
                if cache.sweep_interval.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.sweep_interval does not apply to the \"redis\" backend"
                            .to_string(),
                    ));
                }
                if cache.max_entries.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.max_entries does not apply to the \"redis\" backend"
                            .to_string(),
                    ));
                }
                if cache.max_capacity.is_some() {
                    return Err(CamelError::Config(
                        "cache_repo.max_capacity does not apply to the \"redis\" backend"
                            .to_string(),
                    ));
                }
                validate_redis_topology_fields(
                    "cache_repo",
                    cache.url.as_deref(),
                    cache.sentinel_nodes.as_deref(),
                    cache.master_name.as_deref(),
                    cache.sentinel_username.as_deref(),
                    cache.sentinel_password.as_deref(),
                    cache.password.as_deref(),
                    cache.username.as_deref(),
                    cache.db,
                    cache.key_prefix.as_deref(),
                )?;
            }
        }
        // Cross-repository prefix collision (task 3.3): two redis
        // repositories sharing one database must use distinct key prefixes,
        // or `clear` on one repository would unlink the other's keyspace.
        // The defaults ("camel:cache" vs "camel:idem") differ, so only
        // user-set identical effective prefixes collide.
        if let (Some(cache), Some(idem)) = (&self.cache_repo, &self.idempotent_repo)
            && cache.backend == "redis"
            && idem.backend == "redis"
            && redis_database_key(
                cache.url.as_deref(),
                cache.sentinel_nodes.as_deref(),
                cache.master_name.as_deref(),
                cache.db,
            ) == redis_database_key(
                idem.url.as_deref(),
                idem.sentinel_nodes.as_deref(),
                idem.master_name.as_deref(),
                idem.db,
            )
        {
            let cache_prefix = cache.key_prefix.as_deref().unwrap_or("camel:cache");
            let idem_prefix = idem.key_prefix.as_deref().unwrap_or("camel:idem");
            if cache_prefix == idem_prefix {
                return Err(CamelError::Config(format!(
                    "cache_repo.key_prefix and idempotent_repo.key_prefix must be distinct when both repositories share one redis database (both resolve to '{cache_prefix}')"
                )));
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
        for (name, ds) in &self.datasources {
            if let Err(e) = ds.validate() {
                return Err(CamelError::Config(format!("datasource '{}': {}", name, e)));
            }
        }
        if let Some(ref native) = self.security.native {
            native
                .validate_credentials()
                .map_err(|e| CamelError::Config(e.to_string()))?;
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
        let content = read_capped(path, MAX_CONFIG_FILE_SIZE)?;

        let base_dir = &include_base_dir(path);

        let mut root_value: toml::Value = toml::from_str(&content)
            .map_err(|e| ConfigError::Message(format!("Failed to parse TOML: {}", e)))?;

        let env_profile = std::env::var("CAMEL_PROFILE").ok();
        let effective_profile = profile.or(env_profile.as_deref());

        let includes = Self::extract_includes(&mut root_value, effective_profile)?;

        let pre_sources = crate::include::load_includes(base_dir, &includes, effective_profile)?;

        build_from_toml_value_inner(root_value, profile, merge_env, pre_sources)
    }

    /// Validates and extracts `include` lists from a parsed TOML value.
    ///
    /// Extraction covers the top-level table plus, when present, the `[default]`
    /// and active `[<profile>]` sections (profile-scoped includes). Returns an
    /// error if any `include` is present but not an array of strings.
    ///
    /// Order (lowest priority first): top-level, `[default]`, `[<profile>]` —
    /// profile-scoped includes override top-level ones on key conflicts, mirroring
    /// profile-overlay semantics. The `include` keys are stripped from all
    /// extracted locations so they never reach profile merging or deserialization.
    fn extract_includes(
        raw_value: &mut toml::Value,
        profile: Option<&str>,
    ) -> Result<Vec<String>, ConfigError> {
        let mut paths = Vec::new();

        let Some(table) = raw_value.as_table_mut() else {
            return Ok(paths);
        };

        if let Some(value) = table.remove("include") {
            paths.extend(parse_include_list(&value, "include")?);
        }

        let mut sections = vec!["default"];
        if let Some(p) = profile.filter(|p| *p != "default") {
            sections.push(p);
        }
        for section in sections {
            if let Some(toml::Value::Table(section_table)) = table.get_mut(section)
                && let Some(value) = section_table.remove("include")
            {
                paths.extend(parse_include_list(&value, &format!("{section}.include"))?);
            }
        }

        Ok(paths)
    }

    pub fn from_env_or_default() -> Result<Self, ConfigError> {
        let path = env::var("CAMEL_CONFIG_FILE").unwrap_or_else(|_| "Camel.toml".to_string());

        // from_file_with_env applies the allowlisted CAMEL_* env overrides
        // on top of the loaded file, matching `camel run`'s loader.
        Self::from_file_with_env(&path)
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
        let content = read_capped_async(path, MAX_CONFIG_FILE_SIZE).await?;

        let base_dir_owned = include_base_dir(path);

        let mut root_value: toml::Value = toml::from_str(&content)
            .map_err(|e| ConfigError::Message(format!("Failed to parse TOML: {}", e)))?;

        let env_profile = std::env::var("CAMEL_PROFILE").ok();
        let effective_profile = profile.or(env_profile.as_deref());

        let includes = Self::extract_includes(&mut root_value, effective_profile)?;

        let pre_sources =
            crate::include::load_includes(&base_dir_owned, &includes, effective_profile)?;

        build_from_toml_value_inner(root_value, profile, false, pre_sources)
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
        let content = read_capped_async(path, MAX_CONFIG_FILE_SIZE).await?;

        let base_dir_owned = include_base_dir(path);

        let mut root_value: toml::Value = toml::from_str(&content)
            .map_err(|e| ConfigError::Message(format!("Failed to parse TOML: {}", e)))?;

        let env_profile = std::env::var("CAMEL_PROFILE").ok();
        let effective_profile = profile.or(env_profile.as_deref());

        let includes = Self::extract_includes(&mut root_value, effective_profile)?;

        let pre_sources =
            crate::include::load_includes(&base_dir_owned, &includes, effective_profile)?;

        build_from_toml_value_inner(root_value, profile, true, pre_sources)
    }
}

/// Parse an env var value into a JSON value with type awareness.
/// Tries int, float, bool, then falls back to string.
fn parse_env_value(val: &str) -> serde_json::Value {
    if let Ok(n) = val.parse::<i64>() {
        serde_json::Value::from(n)
    } else if let Ok(f) = val.parse::<f64>() {
        serde_json::Value::from(f)
    } else if let Ok(b) = val.parse::<bool>() {
        serde_json::Value::from(b)
    } else {
        serde_json::Value::from(val)
    }
}

/// Split a CSV env override value into trimmed, non-empty entries for the
/// list-typed overrides in [`CSV_ENV_OVERRIDES`]. Empty (or all-blank) input
/// yields an empty Vec — the caller turns that into an empty JSON array,
/// which replaces the file value.
fn parse_env_csv_list(val: &str) -> Vec<String> {
    val.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// Allowlisted env vars that can override config fields (L-C2 security hardening).
///
/// Only these `CAMEL_*` vars are read as config overrides via the
/// `build_from_toml_value_inner` env-merging path. Non-allowlisted `CAMEL_*` vars
/// are silently ignored (NOT rejected). This is intentional: many `CAMEL_*` env
/// vars are component-specific (e.g. `CAMEL_JMS_BRIDGE_BINARY_PATH`,
/// `CAMEL_NATIVE_ISSUER_KEY_PEM`) or consumed directly by the caller
/// (`CAMEL_CONFIG_FILE`) or read directly in `build_from_toml_value_inner`
/// (`CAMEL_PROFILE`). The security property — that no security-sensitive field can
/// be overridden via env — is still achieved.
///
/// Two vars handled outside this allowlist:
/// - `CAMEL_CONFIG_FILE` — consumed by `from_env_or_default()` before config loading.
/// - `CAMEL_PROFILE` — read directly in `build_from_toml_value_inner` (needed before
///   the config source is built).
const ALLOWED_ENV_OVERRIDES: &[&str] = &[
    "CAMEL_TIMEOUT_MS",
    "CAMEL_DRAIN_TIMEOUT_MS",
    "CAMEL_WATCH",
    "CAMEL_WATCH_DEBOUNCE_MS",
    "CAMEL_LOG_LEVEL",
    "CAMEL_RUNTIME_JOURNAL_PATH",
    "CAMEL_RUNTIME_JOURNAL_DURABILITY",
    "CAMEL_RUNTIME_JOURNAL_COMPACTION_THRESHOLD_EVENTS",
    "CAMEL_IDEMPOTENT_REPO_PATH",
    "CAMEL_IDEMPOTENT_REPO_DURABILITY",
    "CAMEL_CACHE_REPO_BACKEND",
    "CAMEL_CACHE_REPO_PATH",
    "CAMEL_CACHE_REPO_MAX_CAPACITY",
    "CAMEL_CACHE_REPO_STALE_RETENTION",
    "CAMEL_CACHE_REPO_MAX_ENTRIES",
    "CAMEL_CACHE_REPO_PAYLOAD",
    "CAMEL_CACHE_REPO_PAYLOAD_DIR",
    "CAMEL_CACHE_REPO_CACHE_SIZE",
    "CAMEL_CACHE_REPO_SWEEP_INTERVAL",
    "CAMEL_CACHE_REPO_MASTER_NAME",
    "CAMEL_CACHE_REPO_KEY_PREFIX",
    "CAMEL_CACHE_REPO_DB",
    "CAMEL_CACHE_REPO_SENTINEL_NODES",
    "CAMEL_SUPERVISION_INITIAL_DELAY_MS",
    "CAMEL_SUPERVISION_MAX_ATTEMPTS",
];

/// Env override vars whose raw value is a comma-separated list. This is the
/// ONLY list-typed override in the allowlist: entries are split on `,`,
/// trimmed, and blanks dropped; empty input yields an EMPTY array. The empty
/// array REPLACES the file value (unlike the scalar empty-skip in
/// [`EMPTY_SCALAR_ENV_OVERRIDES`]) — for `sentinel_nodes` it is then
/// normalized to absent, so operators can force-unset the field.
const CSV_ENV_OVERRIDES: &[&str] = &["CAMEL_CACHE_REPO_SENTINEL_NODES"];

/// Env override vars whose raw value must reach deserialization as a JSON
/// string, verbatim — no numeric/bool coercion. These cache_repo fields are
/// String-typed in the config shape, and strict deserialization rejects a
/// JSON integer where the file format carries a string:
/// `CAMEL_CACHE_REPO_CACHE_SIZE=268435456` (the documented bare-bytes form)
/// must deserialize as `Some("268435456")`, not fail with
/// "invalid type: integer" on `Option<String>`. The same applies to
/// leading-zero values (`KEY_PREFIX=007` must not collapse to integer `7`).
/// `CAMEL_CACHE_REPO_PAYLOAD` is included even though `PayloadMode` is an
/// enum: it deserializes FROM a string (`"inline"`/`"disk"`), so its raw
/// value must pass through un-coerced — [`parse_env_value`]'s string
/// fallback would cover that only incidentally (valid mode names are never
/// numeric-like); here the passthrough is the contract. Empty values never
/// reach this arm: all six vars are also in [`EMPTY_SCALAR_ENV_OVERRIDES`],
/// and the empty-skip dispatch runs first.
const STRING_ENV_OVERRIDES: &[&str] = &[
    "CAMEL_CACHE_REPO_PAYLOAD",
    "CAMEL_CACHE_REPO_PAYLOAD_DIR",
    "CAMEL_CACHE_REPO_CACHE_SIZE",
    "CAMEL_CACHE_REPO_SWEEP_INTERVAL",
    "CAMEL_CACHE_REPO_MASTER_NAME",
    "CAMEL_CACHE_REPO_KEY_PREFIX",
];

/// Verbatim-string passthrough for the PRE-EXISTING String-typed `cache_repo`
/// vars. Their values (backend names, filesystem paths, durations) may be
/// numeric-like (`CAMEL_CACHE_REPO_STALE_RETENTION=604800`,
/// `CAMEL_CACHE_REPO_PATH=007`), so they route through the same merge arm as
/// [`STRING_ENV_OVERRIDES`] instead of [`parse_env_value`]'s integer/bool
/// guessing — a coerced JSON integer would fail `Option<String>`
/// deserialization with "invalid type: integer". They are deliberately NOT in
/// [`STRING_ENV_OVERRIDES`] — that list pins the `STRING ⊆ EMPTY_SCALAR`
/// subset invariant, and adding these would silently grant them the
/// empty-skip semantics — and deliberately NOT in
/// [`EMPTY_SCALAR_ENV_OVERRIDES`]: legacy vars keep their established empty
/// behavior (an empty value overrides the file value verbatim and fails
/// validation loudly).
const LEGACY_STRING_ENV_OVERRIDES: &[&str] = &[
    "CAMEL_CACHE_REPO_BACKEND",
    "CAMEL_CACHE_REPO_PATH",
    "CAMEL_CACHE_REPO_STALE_RETENTION",
];

/// The ONLY vars for which an empty raw value (`""`) is skipped by the merge
/// loop. Deliberately scoped to the new cache_repo scalars so every
/// pre-existing allowlisted var keeps its exact current behavior (e.g.
/// `CAMEL_TIMEOUT_MS=""` still fails typed deserialization loudly instead of
/// being silently ignored). An empty string must never reach these vars'
/// typed deserialization (`Option<u16>` for `db` would hard-fail on `""`);
/// skipping lets the file/profile value stay effective.
const EMPTY_SCALAR_ENV_OVERRIDES: &[&str] = &[
    "CAMEL_CACHE_REPO_PAYLOAD",
    "CAMEL_CACHE_REPO_PAYLOAD_DIR",
    "CAMEL_CACHE_REPO_CACHE_SIZE",
    "CAMEL_CACHE_REPO_SWEEP_INTERVAL",
    "CAMEL_CACHE_REPO_MASTER_NAME",
    "CAMEL_CACHE_REPO_KEY_PREFIX",
    "CAMEL_CACHE_REPO_DB",
];

/// `CAMEL_*` vars handled OUTSIDE the allowlist merge, so the "ignored" warn
/// must stay silent for them (rc-6gqy): each has its own consumer and is
/// honored, not ignored.
///
/// - `CAMEL_CONFIG_FILE` — selects the config file itself (clap `env` on
///   `camel run --config`, plus `from_env_or_default()`).
/// - `CAMEL_PROFILE` — selects the active profile (read at the top of
///   `build_from_toml_value_inner` and during include extraction).
const HANDLED_ELSEWHERE_ENV_VARS: &[&str] = &["CAMEL_CONFIG_FILE", "CAMEL_PROFILE"];

/// Directory used to resolve `include` paths from a config file path.
///
/// A bare filename (`"root.toml"`) yields an EMPTY parent from
/// [`std::path::Path::parent`] — NOT `None` — so `unwrap_or(".")`
/// fallbacks never fire and an empty base canonicalizes to ENOENT.
/// Map that case to `.` so bare relative filenames resolve against the
/// current directory (rc-k5bz).
fn include_base_dir(path: &str) -> std::path::PathBuf {
    let parent = std::path::Path::new(path).parent();
    parent
        .filter(|p| !p.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Top-level CamelConfig keys recognized by serde deserialization, used to
/// tell real config sections apart from profile-like tables (`[<name>]`) when
/// warning about an unset `CAMEL_PROFILE` (rc-cflo).
///
/// MUST mirror [`CamelConfig`]'s serde field names. Structural keys with their
/// own consumers (`default` for profiles, `include` for file composition) are
/// excluded at the check site instead. The
/// `config_ergonomics_tests::known_top_level_keys_*` tripwires guard drift:
/// a name here that stops being a real field fails `_extra`; a new CamelConfig
/// field missing from this list makes its section warn like an unselected
/// profile.
const KNOWN_TOP_LEVEL_KEYS: &[&str] = &[
    "routes",
    "watch",
    "runtime_journal",
    "idempotent_repo",
    "cache_repo",
    "log_level",
    "timeout_ms",
    "drain_timeout_ms",
    "watch_debounce_ms",
    "components",
    "observability",
    "supervision",
    "platform",
    "stream_caching",
    "beans",
    "languages",
    "security",
    "binds",
    "datasources",
];

/// Core config builder. Accepts a pre-parsed (and `include`-stripped) `toml::Value`
/// so callers do not need to re-parse the content.
fn build_from_toml_value_inner(
    mut config_value: toml::Value,
    profile: Option<&str>,
    merge_env: bool,
    pre_sources: Vec<String>,
) -> Result<CamelConfig, ConfigError> {
    // CAMEL_PROFILE is read directly (not through the allowlist) because it's
    // needed before the config source is built — the profile determines which
    // TOML sections to merge.
    let env_profile = env::var("CAMEL_PROFILE").ok();
    let profile = profile.or(env_profile.as_deref());

    // rc-cflo operator feedback: with no active profile and profile structure
    // present ([default]), apply_profile keeps ONLY [default] — any other
    // section silently vanishes and the process may start without routes/cache.
    // Warn only then: without [default] there is no profile structure and the
    // lenient path drops NOTHING, so unknown sections are not a hazard.
    if profile.is_none()
        && let toml::Value::Table(ref table) = config_value
        && table.contains_key("default")
    {
        let profile_like: Vec<&str> = table
            .iter()
            .filter(|(k, v)| {
                v.is_table()
                    && k.as_str() != "default"
                    && k.as_str() != "include"
                    && !KNOWN_TOP_LEVEL_KEYS.contains(&k.as_str())
            })
            .map(|(k, _)| k.as_str())
            .collect();
        if !profile_like.is_empty() {
            tracing::warn!(
                sections = %profile_like.join(", "),
                "top-level sections look like unselected profiles; only [default] applies — set CAMEL_PROFILE to activate one"
            );
        }
    }

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

    let merged_toml = toml::to_string(&config_value)
        .map_err(|e| ConfigError::Message(format!("Failed to serialize merged config: {}", e)))?;

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
        // L-C2: Only allowlisted env vars override config (security hardening).
        // Warn about non-allowlisted CAMEL_* vars (operator feedback for typos/stale vars).
        for (key, _) in std::env::vars().filter(|(k, _)| {
            k.starts_with("CAMEL_")
                && !ALLOWED_ENV_OVERRIDES.contains(&k.as_str())
                && !HANDLED_ELSEWHERE_ENV_VARS.contains(&k.as_str())
        }) {
            tracing::warn!(var = %key, "CAMEL_* env var not in config override allowlist; ignored");
        }
        // Build a JSON object with nested structure for fields like supervision.*.
        let mut env_map = serde_json::Map::new();
        for var in ALLOWED_ENV_OVERRIDES {
            if let Ok(val) = env::var(var) {
                let Some(key) = var.strip_prefix("CAMEL_") else {
                    continue;
                };
                let key = key.to_lowercase();
                // Four-way kind dispatch (FR2
                // deployment-resolvable-cache-repo-topology): (a) empty
                // scalar override → skip entirely (file/profile value stays
                // effective; "" must never reach typed deserialization);
                // (b) CSV override → JSON array of trimmed non-empty entries
                // (empty input → empty array, which replaces the file list);
                // (c) string-kind override → verbatim JSON string (no
                // numeric/bool guessing — see [`STRING_ENV_OVERRIDES`] and
                // the pre-existing legacy vars in
                // [`LEGACY_STRING_ENV_OVERRIDES`]);
                // (d) everything else → unchanged typed coercion.
                if EMPTY_SCALAR_ENV_OVERRIDES.contains(var) && val.is_empty() {
                    continue;
                }
                let parsed = if CSV_ENV_OVERRIDES.contains(var) {
                    serde_json::Value::Array(
                        parse_env_csv_list(&val)
                            .into_iter()
                            .map(serde_json::Value::String)
                            .collect(),
                    )
                } else if STRING_ENV_OVERRIDES.contains(var)
                    || LEGACY_STRING_ENV_OVERRIDES.contains(var)
                {
                    serde_json::Value::String(val)
                } else {
                    parse_env_value(&val)
                };
                // Handle nested keys (e.g., supervision_initial_delay_ms → supervision.initial_delay_ms)
                if let Some(nested_key) = key.strip_prefix("supervision_") {
                    let sup = env_map
                        .entry("supervision".to_string())
                        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                    if let serde_json::Value::Object(sup_map) = sup {
                        sup_map.insert(nested_key.to_string(), parsed);
                    }
                } else if let Some(nested_key) = key.strip_prefix("runtime_journal_") {
                    let journal = env_map
                        .entry("runtime_journal".to_string())
                        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                    if let serde_json::Value::Object(journal_map) = journal {
                        journal_map.insert(nested_key.to_string(), parsed);
                    }
                } else if let Some(nested_key) = key.strip_prefix("idempotent_repo_") {
                    let repo = env_map
                        .entry("idempotent_repo".to_string())
                        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                    if let serde_json::Value::Object(repo_map) = repo {
                        repo_map.insert(nested_key.to_string(), parsed);
                    }
                } else if let Some(nested_key) = key.strip_prefix("cache_repo_") {
                    let cache = env_map
                        .entry("cache_repo".to_string())
                        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                    if let serde_json::Value::Object(cache_map) = cache {
                        cache_map.insert(nested_key.to_string(), parsed);
                    }
                } else {
                    env_map.insert(key, parsed);
                }
            }
        }
        if !env_map.is_empty() {
            let json = serde_json::to_string(&serde_json::Value::Object(env_map)).map_err(|e| {
                ConfigError::Message(format!("Failed to serialize env overrides: {}", e))
            })?;
            builder = builder.add_source(config::File::from_str(&json, config::FileFormat::Json));
        }
    }
    let built = builder.build()?;

    // Materialize the builder's MERGED state (main file + include files +
    // `CAMEL_*` env overrides) as a raw TOML tree, then walk it. The walk must
    // run on the POST-merge tree: placeholders can arrive via include files
    // and env overrides, which the builder merges internally — walking the
    // pre-builder value would miss them.
    let mut merged_tree: toml::Value = built.try_deserialize()?;
    resolve_tree_placeholders(&mut merged_tree)?;

    // Strict deserialization: unlike the config crate's lenient coercion,
    // `toml::Value::try_into` rejects type mismatches (e.g. a quoted numeric
    // on a numeric field). This swap is intentional and pinned by
    // `placeholder_e2e::quoted_numeric_root_field_is_rejected_after_materialization`.
    let mut config: CamelConfig = merged_tree
        .try_into()
        .map_err(|e| ConfigError::Message(format!("Failed to deserialize merged config: {e}")))?;
    // FR1 (deployment-resolvable-cache-repo-topology): topology values that
    // expanded empty (e.g. `url = "${env:REDIS_URL:-}"` with the var unset,
    // or a literal `""`) become absent before validation selects the
    // topology. Gated on backend == "redis": unconditional normalization
    // would legitimize `url = ""` on memory/redb sections that cross-backend
    // validation rejects today. Blank passwords normalize to unset too
    // (non-blank credentials are never dropped); only `db` stays untouched.
    if let Some(repo) = config.cache_repo.as_mut()
        && repo.backend == "redis"
    {
        repo.normalize_empty_topology();
    }
    if let Some(repo) = config.idempotent_repo.as_mut()
        && repo.backend == "redis"
    {
        repo.normalize_empty_topology();
    }
    config
        .validate()
        .map_err(|e| ConfigError::Message(e.to_string()))?;
    Ok(config)
}

/// Parses one `include` value (array of strings) for [`CamelConfig::extract_includes`].
/// `where_` names the location ("include" or "<section>.include") for error messages.
fn parse_include_list(value: &toml::Value, where_: &str) -> Result<Vec<String>, ConfigError> {
    match value {
        toml::Value::Array(arr) => {
            let mut paths = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                match item.as_str() {
                    Some(s) => paths.push(s.to_string()),
                    None => {
                        return Err(ConfigError::Message(format!(
                            "{where_}[{i}] must be a string, got: {item}",
                        )));
                    }
                }
            }
            Ok(paths)
        }
        other => Err(ConfigError::Message(format!(
            "'{where_}' must be an array of strings, got: {}",
            other.type_str()
        ))),
    }
}

/// Top-level Camel.toml sections whose string leaves resolve strictly
/// (interpolation first, then residual-marker rejection). Credential-bearing
/// surfaces: `security`, `datasources`, and the two repository sections
/// (redis `sentinel_password` + URL userinfo).
pub(crate) const STRICT_PREFIXES: &[&str] =
    &["security", "datasources", "idempotent_repo", "cache_repo"];

/// Recursively resolve every string leaf of a TOML tree in place.
///
/// Leaves whose top-level path segment is in [`STRICT_PREFIXES`] resolve via
/// [`resolve_strict_leaf`]; every other leaf via [`resolve_plain_leaf`].
/// Path segments join with `.`; array indices render as `[i]`
/// (e.g. `security.native.credentials[1].secret`).
pub fn resolve_tree_placeholders(root: &mut toml::Value) -> Result<(), ConfigError> {
    resolve_tree_walk(root, "")
}

fn resolve_tree_walk(value: &mut toml::Value, path: &str) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(s) => {
            let top = path.split('.').next().unwrap_or_default();
            if STRICT_PREFIXES.contains(&top) {
                resolve_strict_leaf(s, path)?;
            } else {
                resolve_plain_leaf(s, path)?;
            }
        }
        toml::Value::Array(arr) => {
            for (i, item) in arr.iter_mut().enumerate() {
                resolve_tree_walk(item, &format!("{path}[{i}]"))?;
            }
        }
        toml::Value::Table(table) => {
            for (k, v) in table.iter_mut() {
                let child_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                resolve_tree_walk(v, &child_path)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Reject the legacy `{{...}}` placeholder syntax, pointing at the `${env:}`
/// replacement forms. Runs on the raw leaf before any resolution.
fn reject_legacy_braces(value: &str, path: &str) -> Result<(), ConfigError> {
    if value.contains("{{") {
        return Err(ConfigError::Message(format!(
            "legacy '{{{{...}}}}' placeholder in {path}: Camel.toml placeholders use \
             '${{env:NAME}}' or '${{env:NAME:-default}}'"
        )));
    }
    Ok(())
}

/// Resolve one strict-class string leaf: interpolate `${env:}` forms, then
/// reject any residual placeholder marker.
///
/// Ordering is the contract: interpolation runs first, so a consumed
/// `${env:NAME}` leaves no `${` and passes the residual gate; malformed,
/// truncated, or escaped forms (`$${env:NAME}` → literal residue) die there.
fn resolve_strict_leaf(value: &mut String, path: &str) -> Result<(), ConfigError> {
    reject_legacy_braces(value, path)?;
    let resolved = camel_dsl::env_interpolation::interpolate_env(value).map_err(|var| {
        ConfigError::Message(format!(
            "placeholder unresolved: {path}: env var {var} not set"
        ))
    })?;
    if resolved.contains("${") || resolved.contains("{{") {
        return Err(ConfigError::Message(format!(
            "unresolved placeholder marker in {path}"
        )));
    }
    *value = resolved;
    Ok(())
}

/// Resolve one non-strict string leaf: interpolate only when an `${env:` or
/// `$$` marker is present; leaves with neither marker pass through untouched
/// (a `${body}` without the `env:` prefix stays).
///
/// Fail-closed is uniform with the strict path (Q9): an unset referenced var
/// surfaces as an error naming the field, never a warn-and-continue.
fn resolve_plain_leaf(value: &mut String, path: &str) -> Result<(), ConfigError> {
    reject_legacy_braces(value, path)?;
    if value.contains("${env:") || value.contains("$$") {
        let resolved = camel_dsl::env_interpolation::interpolate_env(value).map_err(|var| {
            ConfigError::Message(format!(
                "placeholder unresolved: {path}: env var {var} not set"
            ))
        })?;
        *value = resolved;
    }
    Ok(())
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

/// Serializes tests that touch `CAMEL_TIMEOUT_MS` / `CAMEL_PROFILE` env vars.
///
/// `test_from_file_with_env_overrides_timeout` (in `profile_loading_tests`)
/// sets `CAMEL_TIMEOUT_MS=9999` and the async loader in `async_io_tests`
/// reads env via `config::Environment::with_prefix("CAMEL")` after an
/// `.await` point, so without serialization the two race and the async
/// test flakes ~1/5 runs in workspace mode.
#[cfg(test)]
static ENV_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test-only coordination mutex acquisition. Recovery from poison is safe
/// because every env test restores vars before assertions.
#[cfg(test)]
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_OVERRIDE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Sets an env var for a test. Caller MUST hold [`ENV_OVERRIDE_LOCK`].
#[cfg(test)]
fn set_env(key: &str, value: &str) {
    // SAFETY: serialized against every other env-touching test via ENV_OVERRIDE_LOCK.
    unsafe { std::env::set_var(key, value) }
}

/// Removes an env var for a test. Caller MUST hold [`ENV_OVERRIDE_LOCK`].
#[cfg(test)]
fn unset_env(key: &str) {
    // SAFETY: serialized against every other env-touching test via ENV_OVERRIDE_LOCK.
    unsafe { std::env::remove_var(key) }
}

/// Serializes tests that change the process working directory (include
/// resolution tests resolve bare filenames against the cwd).
///
/// Acquire BEFORE [`ENV_OVERRIDE_LOCK`]-using code paths that also chdir to
/// keep lock ordering consistent across tests.
#[cfg(test)]
static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Poison-safe acquisition of [`CWD_LOCK`]. Test-only coordination mutex:
/// recovery is safe because `CwdGuard` restores the cwd on unwind, so a
/// panicking test never leaves a broken invariant behind the lock —
/// `CwdGuard` is declared after the lock guard, so it unwinds first.
#[cfg(test)]
fn cwd_lock() -> std::sync::MutexGuard<'static, ()> {
    CWD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// RAII restore of the process working directory. Declare it after reading the
/// original cwd and before `set_current_dir` into a tempdir; dropping restores,
/// including on panic unwind, so the tempdir's own `Drop` never runs while cwd
/// is inside the directory it deletes.
#[cfg(test)]
struct CwdGuard(std::path::PathBuf);

#[cfg(test)]
impl Drop for CwdGuard {
    fn drop(&mut self) {
        // Cannot report errors from Drop; callers hold CWD_LOCK for the whole span.
        let _ = std::env::set_current_dir(&self.0);
    }
}

/// Test-only log capture: installs a minimal subscriber via
/// `tracing::subscriber::with_default` for the duration of one closure and
/// records WARN-level event messages. No global state — safe under parallel
/// test threads.
#[cfg(test)]
pub(crate) mod log_capture {
    use std::fmt;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Record};
    use tracing::{Event, Id, Level, Metadata, Subscriber};

    type Sink = Arc<Mutex<Vec<String>>>;

    struct Recorder {
        warnings: Sink,
        next_span_id: std::sync::atomic::AtomicU64,
    }

    struct MessageVisitor(String);

    impl Visit for MessageVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            if !self.0.is_empty() {
                self.0.push(' ');
            }
            let _ = fmt::write(&mut self.0, format_args!("{}={:?}", field.name(), value));
        }
    }

    impl Subscriber for Recorder {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _attrs: &Attributes<'_>) -> Id {
            let id = self
                .next_span_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            Id::from_u64(id)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}
        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            if *event.metadata().level() >= Level::WARN {
                let mut visitor = MessageVisitor(String::new());
                event.record(&mut visitor);
                if let Ok(mut slot) = self.warnings.lock() {
                    slot.push(visitor.0);
                }
            }
        }

        fn enter(&self, _span: &Id) {}
        fn exit(&self, _span: &Id) {}
    }

    /// Runs `f` with a capturing subscriber installed and returns
    /// `(f's result, captured warn/error messages)` in emission order.
    /// Messages are rendered as `field="value"` pairs joined by spaces, with
    /// the human-readable text under the standard `message` field.
    pub(crate) fn capture_warns<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
        let sink: Sink = Default::default();
        let recorder = Recorder {
            warnings: Arc::clone(&sink),
            next_span_id: Default::default(),
        };
        let out = tracing::subscriber::with_default(recorder, f);
        let collected = sink
            .lock()
            .ok()
            .map(|slot| slot.clone())
            .unwrap_or_default();
        (out, collected)
    }
}

// The hand-enumerated exhaustiveness guard mods (`security_walk_exhaustiveness`,
// `datasource_walk_exhaustiveness`) were deleted together with the typed
// fail-closed walks they guarded. The path-prefix dispatch in
// `resolve_tree_placeholders` replaces them; the successor guard lives in
// `tests/placeholder_walk.rs` (`strict_dispatch_is_exhaustive_over_security_subtree`
// et al.) plus the `strict_prefixes_content_is_deliberate` tripwire above.
