use serde::{Deserialize, Deserializer};

/// Configuration for the Tracer EIP (Enterprise Integration Pattern).
///
/// This struct defines how message tracing should be performed throughout
/// Camel routes. Use `CamelContext::set_tracer_config` to apply configuration
/// programmatically, or configure via `Camel.toml` as shown in the module documentation.
#[derive(Clone, Debug, Default)]
pub struct TracerConfig {
    /// SPAN enablement (metrics-configuration Req 1: `tracer.enabled` gates
    /// spans ONLY). Explicit values win over the "otel/prometheus imply
    /// tracing" rule in `effective_tracer_config` (camel-config), in both
    /// directions.
    pub enabled: bool,

    /// Whether `enabled` was explicitly present at the serde boundary.
    ///
    /// Not read from input keys: the custom `Deserialize` impl below sets it
    /// from the presence/absence of the `enabled` key, so it is skipped by
    /// every serde path. Explicit values win over the "otel/prometheus imply
    /// tracing" rule in `effective_tracer_config` (camel-config), in both
    /// directions.
    pub tracing_enabled_explicit: bool,

    /// PIPELINE enablement: whether routes are wrapped with the observability
    /// adapters at all. Unlike `enabled`, this is not a TOML key — the
    /// effective-config assembly (camel-config) raises it whenever an
    /// exporter (otel/prometheus) or tracing itself is active, because the
    /// pipeline carries the metric families incl. the non-disableable error
    /// family (metrics-collection-wiring MODIFIED requirement). Programmatic
    /// users leave it `false` and pipeline wrapping follows `enabled`.
    pub pipeline_enabled: bool,

    pub detail_level: DetailLevel,

    pub outputs: TracerOutputs,

    /// Metric-family levers (`[observability.metrics]`). Not a
    /// `[observability.tracer]` key: the levers deserialize at their own
    /// table and camel-config attaches them here during effective-config
    /// assembly, so the tracer serde boundary below never reads them.
    pub metrics_levers: MetricsLeversConfig,
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
            pipeline_enabled: false,
            detail_level: raw.detail_level,
            outputs: raw.outputs,
            metrics_levers: MetricsLeversConfig::default(),
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

/// Metric-family levers for `[observability.metrics]` in Camel.toml
/// (dashboard-observability D3).
///
/// `enabled` is the master switch for the non-error families; `exchange`,
/// `duration`, and `components` are per-family opt-outs (a family flows
/// only when `enabled && <family>`). No lever exists for the error family —
/// `camel_errors_total` is structurally non-disableable
/// (metrics-configuration Req 2).
#[derive(Clone, Debug, PartialEq)]
pub struct MetricsLeversConfig {
    pub enabled: bool,
    pub exchange: bool,
    pub duration: bool,
    pub components: bool,
}

impl MetricsLeversConfig {
    /// Whether the exchanges counter family may flow.
    pub fn exchanges_enabled(&self) -> bool {
        self.enabled && self.exchange
    }

    /// Whether the duration histogram family may flow.
    pub fn durations_enabled(&self) -> bool {
        self.enabled && self.duration
    }

    /// Whether the uniform component-operations counter family may flow
    /// (`camel_component_operations_total`; the error family is never
    /// gated by any lever).
    pub fn components_enabled(&self) -> bool {
        self.enabled && self.components
    }
}

impl Default for MetricsLeversConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            exchange: true,
            duration: true,
            components: false,
        }
    }
}

/// Deserializes `MetricsLeversConfig` via a `Raw` intermediate with
/// `Option<bool>` fields so an absent table and an absent key both mean
/// "default" (same serde-boundary technique as `TracerConfig`). Unknown
/// keys are denied, consistent with the sibling observability tables.
impl<'de> Deserialize<'de> for MetricsLeversConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            #[serde(default)]
            enabled: Option<bool>,
            #[serde(default)]
            exchange: Option<bool>,
            #[serde(default)]
            duration: Option<bool>,
            #[serde(default)]
            components: Option<bool>,
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            enabled: raw.enabled.unwrap_or(true),
            exchange: raw.exchange.unwrap_or(true),
            duration: raw.duration.unwrap_or(true),
            components: raw.components.unwrap_or(false),
        })
    }
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
