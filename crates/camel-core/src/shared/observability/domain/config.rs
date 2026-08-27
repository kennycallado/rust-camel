use serde::{Deserialize, Deserializer};

/// Configuration for the Tracer EIP (Enterprise Integration Pattern).
///
/// This struct defines how message tracing should be performed throughout
/// Camel routes. Use `CamelContext::set_tracer_config` to apply configuration
/// programmatically, or configure via `Camel.toml` as shown in the module documentation.
#[derive(Clone, Debug, Default)]
pub struct TracerConfig {
    pub enabled: bool,

    /// Whether `enabled` was explicitly present at the serde boundary.
    ///
    /// Not read from input keys: the custom `Deserialize` impl below sets it
    /// from the presence/absence of the `enabled` key, so it is skipped by
    /// every serde path. Explicit values win over the "otel/prometheus imply
    /// tracing" rule in `effective_tracer_config` (camel-config), in both
    /// directions.
    pub tracing_enabled_explicit: bool,

    pub detail_level: DetailLevel,

    pub outputs: TracerOutputs,
}

/// Deserializes `TracerConfig` with serde-boundary detection for `enabled`:
/// an absent key means "not explicitly set" (`enabled = false`,
/// `tracing_enabled_explicit = false`), while any explicit `enabled` value
/// keeps the flag set so callers can honor it over implied enabling.
///
/// The intermediate `Raw` struct mirrors the public field set and its serde
/// attributes (`#[serde(default)]` behavior on other fields is unchanged).
impl<'de> Deserialize<'de> for TracerConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            enabled: Option<bool>,
            #[serde(default = "default_detail_level")]
            detail_level: DetailLevel,
            #[serde(default)]
            outputs: TracerOutputs,
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            enabled: raw.enabled.unwrap_or(false),
            tracing_enabled_explicit: raw.enabled.is_some(),
            detail_level: raw.detail_level,
            outputs: raw.outputs,
        })
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TracerOutputs {
    #[serde(default)]
    pub stdout: StdoutOutput,

    #[serde(default)]
    pub file: Option<FileOutput>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StdoutOutput {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_format")]
    pub format: OutputFormat,
}

impl Default for StdoutOutput {
    fn default() -> Self {
        Self {
            enabled: true,
            format: OutputFormat::Json,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileOutput {
    pub enabled: bool,
    pub path: String,
    #[serde(default = "default_format")]
    pub format: OutputFormat,
}

/// Controls the level of detail captured in trace spans.
///
/// Each variant progressively adds more fields to the trace output:
///
/// - `Minimal`: Includes the core span attributes (correlation_id, route_id,
///   and step_index). `duration_ms` is a `camel_tracer` log field at
///   every detail level, not a span attribute.
/// - `Medium`: Includes Minimal fields plus headers_count, body_type, has_error,
///   and output_body_type
/// - `Full`: Includes all fields from Minimal and Medium plus up to 3 message headers
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum DetailLevel {
    #[default]
    Minimal,
    Medium,
    Full,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Json,
    Plain,
}

fn default_detail_level() -> DetailLevel {
    DetailLevel::Minimal
}
fn default_format() -> OutputFormat {
    OutputFormat::Json
}
fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracer_config_defaults_are_stable() {
        let cfg = TracerConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.detail_level, DetailLevel::Minimal);
        assert!(cfg.outputs.stdout.enabled);
        assert!(matches!(cfg.outputs.stdout.format, OutputFormat::Json));
        assert!(cfg.outputs.file.is_none());
    }

    #[test]
    fn tracer_config_deserializes_lowercase_enums() {
        let cfg: TracerConfig = serde_json::from_str(
            r#"{
  "enabled": true,
  "detail_level": "full",
  "outputs": {
    "stdout": { "enabled": false, "format": "plain" },
    "file": { "enabled": true, "path": "/tmp/trace.log", "format": "json" }
  }
}"#,
        )
        .unwrap();

        assert!(cfg.enabled);
        assert_eq!(cfg.detail_level, DetailLevel::Full);
        assert!(!cfg.outputs.stdout.enabled);
        assert!(matches!(cfg.outputs.stdout.format, OutputFormat::Plain));
        assert_eq!(cfg.outputs.file.as_ref().unwrap().path, "/tmp/trace.log");
    }
}
