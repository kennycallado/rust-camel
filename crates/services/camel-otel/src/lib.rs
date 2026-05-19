//! OpenTelemetry integration — tracing, metrics, and context propagation for rust-camel pipelines.
//!
//! Main types: `OtelService`, `OtelConfig`, `OtelMetrics`, `OtelProtocol`.
//! Main modules: `config`, `metrics`, `propagation`, `service`.

pub mod config;
pub mod metrics;
pub mod propagation;
pub mod service;

pub use config::{OtelConfig, OtelProtocol, OtelSampler};
pub use metrics::OtelMetrics;
pub use opentelemetry_sdk::logs::SdkLoggerProvider;
pub use propagation::{
    TRACE_PARENT_HEADER, TRACE_STATE_HEADER, extract_context, extract_into_exchange,
    inject_context, inject_from_exchange,
};
pub use service::OtelService;
