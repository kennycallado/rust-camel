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

    /// Optional stable runtime journal path for CQRS event durability/replay.
    ///
    /// When unset, runtime durability/replay is disabled and runtime state is ephemeral.
    #[serde(default)]
    pub runtime_journal_path: Option<String>,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    #[serde(default)]
    pub components: ComponentsConfig,

    #[serde(default)]
    pub observability: ObservabilityConfig,

    #[serde(default)]
    pub supervision: Option<SupervisionCamelConfig>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct ComponentsConfig {
    #[serde(default)]
    pub timer: Option<TimerConfig>,

    #[serde(default)]
    pub http: Option<HttpCamelConfig>,

    #[serde(default)]
    pub kafka: Option<KafkaCamelConfig>,

    #[serde(default)]
    pub redis: Option<RedisCamelConfig>,

    #[serde(default)]
    pub sql: Option<SqlCamelConfig>,

    #[serde(default)]
    pub file: Option<FileCamelConfig>,

    #[serde(default)]
    pub container: Option<ContainerCamelConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TimerConfig {
    #[serde(default = "default_timer_period")]
    pub period: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct HttpCamelConfig {
    #[serde(default = "default_http_connect_timeout_ms")]
    pub connect_timeout_ms: u64,

    #[serde(default = "default_http_response_timeout_ms")]
    pub response_timeout_ms: u64,

    #[serde(default = "default_http_max_connections")]
    pub max_connections: usize,

    #[serde(default = "default_http_max_body_size")]
    pub max_body_size: usize,

    #[serde(default = "default_http_max_request_body")]
    pub max_request_body: usize,

    #[serde(default)]
    pub allow_private_ips: bool,
}

impl Default for HttpCamelConfig {
    fn default() -> Self {
        Self {
            connect_timeout_ms: default_http_connect_timeout_ms(),
            response_timeout_ms: default_http_response_timeout_ms(),
            max_connections: default_http_max_connections(),
            max_body_size: default_http_max_body_size(),
            max_request_body: default_http_max_request_body(),
            allow_private_ips: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct KafkaCamelConfig {
    #[serde(default = "default_kafka_brokers")]
    pub brokers: String,
    #[serde(default = "default_kafka_group_id")]
    pub group_id: String,
    #[serde(default = "default_kafka_session_timeout_ms")]
    pub session_timeout_ms: u32,
    #[serde(default = "default_kafka_request_timeout_ms")]
    pub request_timeout_ms: u32,
    #[serde(default = "default_kafka_auto_offset_reset")]
    pub auto_offset_reset: String,
    #[serde(default = "default_kafka_security_protocol")]
    pub security_protocol: String,
}

impl Default for KafkaCamelConfig {
    fn default() -> Self {
        Self {
            brokers: default_kafka_brokers(),
            group_id: default_kafka_group_id(),
            session_timeout_ms: default_kafka_session_timeout_ms(),
            request_timeout_ms: default_kafka_request_timeout_ms(),
            auto_offset_reset: default_kafka_auto_offset_reset(),
            security_protocol: default_kafka_security_protocol(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RedisCamelConfig {
    #[serde(default = "default_redis_host")]
    pub host: String,
    #[serde(default = "default_redis_port")]
    pub port: u16,
}

impl Default for RedisCamelConfig {
    fn default() -> Self {
        Self {
            host: default_redis_host(),
            port: default_redis_port(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SqlCamelConfig {
    #[serde(default = "default_sql_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_sql_min_connections")]
    pub min_connections: u32,
    #[serde(default = "default_sql_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    #[serde(default = "default_sql_max_lifetime_secs")]
    pub max_lifetime_secs: u64,
}

impl Default for SqlCamelConfig {
    fn default() -> Self {
        Self {
            max_connections: default_sql_max_connections(),
            min_connections: default_sql_min_connections(),
            idle_timeout_secs: default_sql_idle_timeout_secs(),
            max_lifetime_secs: default_sql_max_lifetime_secs(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct FileCamelConfig {
    #[serde(default = "default_file_delay_ms")]
    pub delay_ms: u64,
    #[serde(default = "default_file_initial_delay_ms")]
    pub initial_delay_ms: u64,
    #[serde(default = "default_file_read_timeout_ms")]
    pub read_timeout_ms: u64,
    #[serde(default = "default_file_write_timeout_ms")]
    pub write_timeout_ms: u64,
}

impl Default for FileCamelConfig {
    fn default() -> Self {
        Self {
            delay_ms: default_file_delay_ms(),
            initial_delay_ms: default_file_initial_delay_ms(),
            read_timeout_ms: default_file_read_timeout_ms(),
            write_timeout_ms: default_file_write_timeout_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ContainerCamelConfig {
    #[serde(default = "default_container_docker_host")]
    pub docker_host: String,
}

impl Default for ContainerCamelConfig {
    fn default() -> Self {
        Self {
            docker_host: default_container_docker_host(),
        }
    }
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

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ObservabilityConfig {
    #[serde(default)]
    pub tracer: TracerConfig,

    #[serde(default)]
    pub otel: Option<OtelCamelConfig>,

    #[serde(default)]
    pub prometheus: Option<PrometheusCamelConfig>,
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

fn default_log_level() -> String {
    "INFO".to_string()
}
fn default_timeout_ms() -> u64 {
    5000
}
fn default_timer_period() -> u64 {
    1000
}
fn default_http_connect_timeout_ms() -> u64 {
    5_000
}
fn default_http_response_timeout_ms() -> u64 {
    30_000
}
fn default_http_max_connections() -> usize {
    100
}
fn default_http_max_body_size() -> usize {
    10_485_760
}
fn default_http_max_request_body() -> usize {
    2_097_152
}

fn default_kafka_brokers() -> String {
    "localhost:9092".to_string()
}
fn default_kafka_group_id() -> String {
    "camel".to_string()
}
fn default_kafka_session_timeout_ms() -> u32 {
    45_000
}
fn default_kafka_request_timeout_ms() -> u32 {
    30_000
}
fn default_kafka_auto_offset_reset() -> String {
    "latest".to_string()
}
fn default_kafka_security_protocol() -> String {
    "plaintext".to_string()
}

fn default_redis_host() -> String {
    "localhost".to_string()
}
fn default_redis_port() -> u16 {
    6379
}

fn default_sql_max_connections() -> u32 {
    5
}
fn default_sql_min_connections() -> u32 {
    1
}
fn default_sql_idle_timeout_secs() -> u64 {
    300
}
fn default_sql_max_lifetime_secs() -> u64 {
    1_800
}

fn default_file_delay_ms() -> u64 {
    500
}
fn default_file_initial_delay_ms() -> u64 {
    1_000
}
fn default_file_read_timeout_ms() -> u64 {
    30_000
}
fn default_file_write_timeout_ms() -> u64 {
    30_000
}

fn default_container_docker_host() -> String {
    "unix:///var/run/docker.sock".to_string()
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
mod http_camel_config_tests {
    use super::*;

    fn parse(toml: &str) -> CamelConfig {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(toml, config::FileFormat::Toml))
            .build()
            .unwrap();
        cfg.try_deserialize().unwrap()
    }

    #[test]
    fn test_http_camel_config_defaults() {
        let cfg = parse("");
        assert!(cfg.components.http.is_none());
    }

    #[test]
    fn test_http_camel_config_default_matches_serde() {
        let default = HttpCamelConfig::default();
        assert_eq!(default.connect_timeout_ms, 5_000);
        assert_eq!(default.response_timeout_ms, 30_000);
        assert_eq!(default.max_connections, 100);
        assert_eq!(default.max_body_size, 10_485_760);
        assert_eq!(default.max_request_body, 2_097_152);
        assert!(!default.allow_private_ips);
    }

    #[test]
    fn test_http_camel_config_partial_override() {
        let cfg = parse(
            r#"
[components.http]
connect_timeout_ms = 1000
"#,
        );
        let http = cfg.components.http.unwrap();
        assert_eq!(http.connect_timeout_ms, 1000);
        assert_eq!(http.response_timeout_ms, 30_000);
        assert_eq!(http.max_connections, 100);
        assert_eq!(http.max_body_size, 10_485_760);
        assert_eq!(http.max_request_body, 2_097_152);
        assert!(!http.allow_private_ips);
    }

    #[test]
    fn test_http_camel_config_all_fields() {
        let cfg = parse(
            r#"
[components.http]
connect_timeout_ms = 2000
response_timeout_ms = 60000
max_connections = 50
max_body_size = 5242880
max_request_body = 1048576
allow_private_ips = true
"#,
        );
        let http = cfg.components.http.unwrap();
        assert_eq!(http.connect_timeout_ms, 2000);
        assert_eq!(http.response_timeout_ms, 60000);
        assert_eq!(http.max_connections, 50);
        assert_eq!(http.max_body_size, 5_242_880);
        assert_eq!(http.max_request_body, 1_048_576);
        assert!(http.allow_private_ips);
    }
}

#[cfg(test)]
mod component_camel_config_tests {
    use super::*;

    fn parse(toml: &str) -> CamelConfig {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(toml, config::FileFormat::Toml))
            .build()
            .unwrap();
        cfg.try_deserialize().unwrap()
    }

    #[test]
    fn test_kafka_defaults() {
        let cfg = parse("");
        assert!(cfg.components.kafka.is_none());
    }

    #[test]
    fn test_kafka_partial_override() {
        let cfg = parse(
            r#"
[components.kafka]
brokers = "prod:9092"
"#,
        );
        let k = cfg.components.kafka.unwrap();
        assert_eq!(k.brokers, "prod:9092");
        assert_eq!(k.group_id, "camel");
        assert_eq!(k.session_timeout_ms, 45_000);
        assert_eq!(k.request_timeout_ms, 30_000);
        assert_eq!(k.auto_offset_reset, "latest");
        assert_eq!(k.security_protocol, "plaintext");
    }

    #[test]
    fn test_redis_defaults() {
        let cfg = parse(
            r#"
[components.redis]
port = 6380
"#,
        );
        let r = cfg.components.redis.unwrap();
        assert_eq!(r.host, "localhost");
        assert_eq!(r.port, 6380);
    }

    #[test]
    fn test_sql_defaults() {
        let cfg = parse(
            r#"
[components.sql]
max_connections = 10
"#,
        );
        let s = cfg.components.sql.unwrap();
        assert_eq!(s.max_connections, 10);
        assert_eq!(s.min_connections, 1);
        assert_eq!(s.idle_timeout_secs, 300);
        assert_eq!(s.max_lifetime_secs, 1_800);
    }

    #[test]
    fn test_file_defaults() {
        let cfg = parse(
            r#"
[components.file]
delay_ms = 1000
"#,
        );
        let f = cfg.components.file.unwrap();
        assert_eq!(f.delay_ms, 1000);
        assert_eq!(f.initial_delay_ms, 1_000);
        assert_eq!(f.read_timeout_ms, 30_000);
        assert_eq!(f.write_timeout_ms, 30_000);
    }

    #[test]
    fn test_container_defaults() {
        let cfg = parse(
            r#"
[components.container]
docker_host = "tcp://remote:2375"
"#,
        );
        let c = cfg.components.container.unwrap();
        assert_eq!(c.docker_host, "tcp://remote:2375");
    }

    #[test]
    fn test_omitted_sections_are_none() {
        let cfg = parse("");
        assert!(cfg.components.kafka.is_none());
        assert!(cfg.components.redis.is_none());
        assert!(cfg.components.sql.is_none());
        assert!(cfg.components.file.is_none());
        assert!(cfg.components.container.is_none());
    }

    #[test]
    fn test_kafka_camel_config_default_matches_serde() {
        let d = KafkaCamelConfig::default();
        assert_eq!(d.brokers, "localhost:9092");
        assert_eq!(d.group_id, "camel");
        assert_eq!(d.session_timeout_ms, 45_000);
        assert_eq!(d.request_timeout_ms, 30_000);
        assert_eq!(d.auto_offset_reset, "latest");
        assert_eq!(d.security_protocol, "plaintext");
    }

    #[test]
    fn test_redis_camel_config_default_matches_serde() {
        let d = RedisCamelConfig::default();
        assert_eq!(d.host, "localhost");
        assert_eq!(d.port, 6379);
    }

    #[test]
    fn test_sql_camel_config_default_matches_serde() {
        let d = SqlCamelConfig::default();
        assert_eq!(d.max_connections, 5);
        assert_eq!(d.min_connections, 1);
        assert_eq!(d.idle_timeout_secs, 300);
        assert_eq!(d.max_lifetime_secs, 1_800);
    }

    #[test]
    fn test_file_camel_config_default_matches_serde() {
        let d = FileCamelConfig::default();
        assert_eq!(d.delay_ms, 500);
        assert_eq!(d.initial_delay_ms, 1_000);
        assert_eq!(d.read_timeout_ms, 30_000);
        assert_eq!(d.write_timeout_ms, 30_000);
    }

    #[test]
    fn test_container_camel_config_default_matches_serde() {
        let d = ContainerCamelConfig::default();
        assert_eq!(d.docker_host, "unix:///var/run/docker.sock");
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
}

#[cfg(all(test, feature = "http"))]
mod http_from_tests {
    use crate::config::HttpCamelConfig;
    use camel_component_http;

    #[test]
    fn test_http_camel_config_to_http_config() {
        let camel_cfg = HttpCamelConfig {
            connect_timeout_ms: 1_000,
            response_timeout_ms: 5_000,
            max_connections: 20,
            max_body_size: 1_000,
            max_request_body: 500,
            allow_private_ips: true,
        };
        let cfg = camel_component_http::HttpConfig::from(&camel_cfg);
        assert_eq!(cfg.connect_timeout_ms, 1_000);
        assert_eq!(cfg.max_connections, 20);
        assert!(cfg.allow_private_ips);
    }
}
