use camel_core::TracerConfig;
use config::{Config, ConfigError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
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

    #[serde(default = "default_log_level")]
    pub log_level: String,

    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    #[serde(default = "default_drain_timeout_ms")]
    pub drain_timeout_ms: u64,

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
#[derive(Debug, Clone, Deserialize, Default)]
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
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
fn merge_toml_values(base: &mut toml::Value, overlay: &toml::Value) {
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
    pub fn from_file(path: &str) -> Result<Self, ConfigError> {
        Self::from_file_with_profile(path, None)
    }

    pub fn from_file_with_env(path: &str) -> Result<Self, ConfigError> {
        Self::from_file_with_profile_and_env(path, None)
    }

    pub fn from_file_with_profile(path: &str, profile: Option<&str>) -> Result<Self, ConfigError> {
        // Get profile from parameter or environment variable
        let env_profile = env::var("CAMEL_PROFILE").ok();
        let profile = profile.or(env_profile.as_deref());

        // Read the TOML file as a generic value for deep merging
        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Message(format!("Failed to read config file: {}", e)))?;
        let mut config_value: toml::Value = toml::from_str(&content)
            .map_err(|e| ConfigError::Message(format!("Failed to parse TOML: {}", e)))?;

        // If a profile is specified, merge it with default
        if let Some(p) = profile {
            // Extract default config as base
            let default_value = config_value.get("default").cloned();

            // Extract profile config
            let profile_value = config_value.get(p).cloned();

            if let (Some(mut base), Some(overlay)) = (default_value, profile_value) {
                // Deep merge profile onto default
                merge_toml_values(&mut base, &overlay);

                // Replace the entire config with the merged result
                config_value = base;
            } else if let Some(profile_val) = config_value.get(p).cloned() {
                // No default, just use profile
                config_value = profile_val;
            } else {
                return Err(ConfigError::Message(format!("Unknown profile: {}", p)));
            }
        } else {
            // No profile specified, use default section if it exists
            if let Some(default_val) = config_value.get("default").cloned() {
                config_value = default_val;
            }
        }

        // Deserialize the merged config
        let merged_toml = toml::to_string(&config_value).map_err(|e| {
            ConfigError::Message(format!("Failed to serialize merged config: {}", e))
        })?;

        let config = Config::builder()
            .add_source(config::File::from_str(
                &merged_toml,
                config::FileFormat::Toml,
            ))
            .build()?;

        config.try_deserialize()
    }

    pub fn from_file_with_profile_and_env(
        path: &str,
        profile: Option<&str>,
    ) -> Result<Self, ConfigError> {
        // Get profile from parameter or environment variable
        let env_profile = env::var("CAMEL_PROFILE").ok();
        let profile = profile.or(env_profile.as_deref());

        // Read the TOML file as a generic value for deep merging
        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Message(format!("Failed to read config file: {}", e)))?;
        let mut config_value: toml::Value = toml::from_str(&content)
            .map_err(|e| ConfigError::Message(format!("Failed to parse TOML: {}", e)))?;

        // If a profile is specified, merge it with default
        if let Some(p) = profile {
            // Extract default config as base
            let default_value = config_value.get("default").cloned();

            // Extract profile config
            let profile_value = config_value.get(p).cloned();

            if let (Some(mut base), Some(overlay)) = (default_value, profile_value) {
                // Deep merge profile onto default
                merge_toml_values(&mut base, &overlay);

                // Replace the entire config with the merged result
                config_value = base;
            } else if let Some(profile_val) = config_value.get(p).cloned() {
                // No default, just use profile
                config_value = profile_val;
            } else {
                return Err(ConfigError::Message(format!("Unknown profile: {}", p)));
            }
        } else {
            // No profile specified, use default section if it exists
            if let Some(default_val) = config_value.get("default").cloned() {
                config_value = default_val;
            }
        }

        // Deserialize the merged config and apply environment variables
        let merged_toml = toml::to_string(&config_value).map_err(|e| {
            ConfigError::Message(format!("Failed to serialize merged config: {}", e))
        })?;

        let config = Config::builder()
            .add_source(config::File::from_str(
                &merged_toml,
                config::FileFormat::Toml,
            ))
            .add_source(config::Environment::with_prefix("CAMEL").try_parsing(true))
            .build()?;

        config.try_deserialize()
    }

    pub fn from_env_or_default() -> Result<Self, ConfigError> {
        let path = env::var("CAMEL_CONFIG_FILE").unwrap_or_else(|_| "Camel.toml".to_string());

        Self::from_file(&path)
    }
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
