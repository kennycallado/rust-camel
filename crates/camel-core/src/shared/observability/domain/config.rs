use std::fmt;
use std::sync::Arc;

use serde::Deserialize;

use camel_api::metrics::MetricsCollector;

/// Configuration for the Tracer EIP (Enterprise Integration Pattern).
///
/// This struct defines how message tracing should be performed throughout
/// Camel routes. Use `CamelContext::set_tracer_config` to apply configuration
/// programmatically, or configure via `Camel.toml` as shown in the module documentation.
#[derive(Clone, Deserialize, Default)]
pub struct TracerConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_detail_level")]
    pub detail_level: DetailLevel,

    #[serde(default)]
    pub outputs: TracerOutputs,

    /// Metrics collector for recording route-level metrics.
    /// Not serializable - injected at runtime.
    #[serde(skip)]
    pub metrics_collector: Option<Arc<dyn MetricsCollector>>,
}

impl fmt::Debug for TracerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TracerConfig")
            .field("enabled", &self.enabled)
            .field("detail_level", &self.detail_level)
            .field("outputs", &self.outputs)
            .field(
                "metrics_collector",
                &self.metrics_collector.as_ref().map(|_| "MetricsCollector"),
            )
            .finish()
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
/// - `Minimal`: Includes only the core fields (correlation_id, route_id, step_id,
///   step_index, timestamp, duration_ms, status)
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
