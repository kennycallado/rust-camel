//! Prometheus metrics integration for rust-camel.
//!
//! Implements `MetricsCollector` trait to export rust-camel metrics in Prometheus format.
//!
//! # TODO(PRM-011)
//!
//! - Route policy integration not implemented (per-route metric policies)
//! - Event notifier hook not implemented (metric event callbacks)
//! - Prometheus config struct (scrape interval, custom labels, histogram buckets) not exposed

pub mod metrics;
pub mod server;
pub mod service;

pub use camel_api::HealthChecker;
pub use camel_api::metrics::MetricsCollector;
pub use metrics::PrometheusMetrics;
pub use server::MetricsServer;
pub use service::PrometheusService;
