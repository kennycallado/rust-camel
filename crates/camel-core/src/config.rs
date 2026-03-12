use serde::Deserialize;

/// Configuration for the Tracer EIP (Enterprise Integration Pattern).
///
/// This struct defines how message tracing should be performed throughout
/// Camel routes. Use `CamelContext::set_tracer_config` to apply configuration
/// programmatically, or configure via `Camel.toml` as shown in the module documentation.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TracerConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_detail_level")]
    pub detail_level: DetailLevel,

    #[serde(default)]
    pub outputs: TracerOutputs,
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
