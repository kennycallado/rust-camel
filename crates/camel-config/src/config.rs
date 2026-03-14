use camel_core::config::TracerConfig;
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
    pub http: Option<HttpConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TimerConfig {
    #[serde(default = "default_timer_period")]
    pub period: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct HttpConfig {
    #[serde(default = "default_http_connect_timeout")]
    pub connect_timeout_ms: u64,

    #[serde(default = "default_http_max_connections")]
    pub max_connections: usize,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ObservabilityConfig {
    #[serde(default)]
    pub metrics_enabled: bool,

    #[serde(default = "default_metrics_port")]
    pub metrics_port: u16,

    #[serde(default)]
    pub tracer: TracerConfig,

    #[serde(default)]
    pub otel: Option<OtelCamelConfig>,
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
fn default_http_connect_timeout() -> u64 {
    5000
}
fn default_http_max_connections() -> usize {
    100
}
fn default_metrics_port() -> u16 {
    9090
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
