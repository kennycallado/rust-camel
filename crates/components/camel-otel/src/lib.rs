pub mod config;
pub mod metrics;
pub mod propagation;
pub mod service;

pub use config::{OtelConfig, OtelProtocol, OtelSampler};
pub use metrics::OtelMetrics;
pub use propagation::{
    extract_context, extract_into_exchange, inject_context, inject_from_exchange,
    TRACE_PARENT_HEADER, TRACE_STATE_HEADER,
};
pub use service::OtelService;
