use std::time::Duration;

use camel_api::metrics::MetricsCollector;
use prometheus::{CounterVec, GaugeVec, HistogramVec, Opts, Registry, TextEncoder};

/// Prometheus metrics collector for rust-camel
///
/// This struct implements the `MetricsCollector` trait and exposes metrics
/// in Prometheus format via the `/metrics` endpoint.
pub struct PrometheusMetrics {
    registry: Registry,
    exchanges_total: CounterVec,
    errors_total: CounterVec,
    exchange_duration_seconds: HistogramVec,
    queue_depth: GaugeVec,
    circuit_breaker_state: GaugeVec,
}

impl PrometheusMetrics {
    /// Creates a new PrometheusMetrics instance with all metrics registered
    pub fn new() -> Self {
        let registry = Registry::new();

        // Create and register exchanges_total counter
        let exchanges_total = CounterVec::new(
            Opts::new("exchanges_total", "Total number of exchanges processed").namespace("camel"),
            &["route"],
        )
        .expect("Failed to create exchanges_total counter");
        registry
            .register(Box::new(exchanges_total.clone()))
            .expect("Failed to register exchanges_total counter");

        // Create and register errors_total counter
        let errors_total = CounterVec::new(
            Opts::new("errors_total", "Total number of errors").namespace("camel"),
            &["route", "error_type"],
        )
        .expect("Failed to create errors_total counter");
        registry
            .register(Box::new(errors_total.clone()))
            .expect("Failed to register errors_total counter");

        // Create and register exchange_duration_seconds histogram
        // Using buckets suitable for typical exchange durations (ms to seconds range)
        let exchange_duration_seconds = HistogramVec::new(
            prometheus::HistogramOpts {
                common_opts: Opts::new(
                    "exchange_duration_seconds",
                    "Exchange processing duration in seconds",
                )
                .namespace("camel"),
                buckets: vec![
                    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                ],
            },
            &["route"],
        )
        .expect("Failed to create exchange_duration_seconds histogram");
        registry
            .register(Box::new(exchange_duration_seconds.clone()))
            .expect("Failed to register exchange_duration_seconds histogram");

        // Create and register queue_depth gauge
        let queue_depth = GaugeVec::new(
            Opts::new("queue_depth", "Current queue depth").namespace("camel"),
            &["route"],
        )
        .expect("Failed to create queue_depth gauge");
        registry
            .register(Box::new(queue_depth.clone()))
            .expect("Failed to register queue_depth gauge");

        // Create and register circuit_breaker_state gauge
        let circuit_breaker_state = GaugeVec::new(
            Opts::new(
                "circuit_breaker_state",
                "Circuit breaker state (0=closed, 1=open, 2=half_open)",
            )
            .namespace("camel"),
            &["route"],
        )
        .expect("Failed to create circuit_breaker_state gauge");
        registry
            .register(Box::new(circuit_breaker_state.clone()))
            .expect("Failed to register circuit_breaker_state gauge");

        Self {
            registry,
            exchanges_total,
            errors_total,
            exchange_duration_seconds,
            queue_depth,
            circuit_breaker_state,
        }
    }

    /// Returns a reference to the underlying Prometheus registry
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Gathers all metrics and returns them in Prometheus text format
    pub fn gather(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        encoder
            .encode_to_string(&metric_families)
            .unwrap_or_else(|e| format!("# Error encoding metrics: {}\n", e))
    }
}

impl Default for PrometheusMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsCollector for PrometheusMetrics {
    fn record_exchange_duration(&self, route_id: &str, duration: Duration) {
        let duration_secs = duration.as_secs_f64();
        self.exchange_duration_seconds
            .with_label_values(&[route_id])
            .observe(duration_secs);
    }

    fn increment_errors(&self, route_id: &str, error_type: &str) {
        self.errors_total
            .with_label_values(&[route_id, error_type])
            .inc();
    }

    fn increment_exchanges(&self, route_id: &str) {
        self.exchanges_total.with_label_values(&[route_id]).inc();
    }

    fn set_queue_depth(&self, route_id: &str, depth: usize) {
        self.queue_depth
            .with_label_values(&[route_id])
            .set(depth as f64);
    }

    fn record_circuit_breaker_change(&self, route_id: &str, _from: &str, to: &str) {
        // Map state names to numeric values
        let state_value = |state: &str| -> f64 {
            match state.to_lowercase().as_str() {
                "closed" => 0.0,
                "open" => 1.0,
                "half_open" | "halfopen" => 2.0,
                _ => -1.0, // Unknown state
            }
        };

        // Set the new state
        self.circuit_breaker_state
            .with_label_values(&[route_id])
            .set(state_value(to));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_create_prometheus_metrics() {
        let metrics = PrometheusMetrics::new();
        // Verify registry is accessible
        let _ = metrics.registry();
    }

    #[test]
    fn test_default_implementation() {
        let metrics = PrometheusMetrics::default();
        // Verify registry is accessible
        let _ = metrics.registry();
    }

    #[test]
    fn test_increment_exchanges() {
        let metrics = PrometheusMetrics::new();

        // Should not panic
        metrics.increment_exchanges("test-route");
        metrics.increment_exchanges("test-route");
        metrics.increment_exchanges("other-route");

        // Verify the metric is registered
        let output = metrics.gather();
        assert!(output.contains("camel_exchanges_total"));
        assert!(output.contains("test-route"));
        assert!(output.contains("other-route"));
    }

    #[test]
    fn test_increment_errors() {
        let metrics = PrometheusMetrics::new();

        // Should not panic
        metrics.increment_errors("test-route", "timeout");
        metrics.increment_errors("test-route", "connection_failed");
        metrics.increment_errors("other-route", "timeout");

        // Verify the metric is registered
        let output = metrics.gather();
        assert!(output.contains("camel_errors_total"));
        assert!(output.contains("timeout"));
        assert!(output.contains("connection_failed"));
    }

    #[test]
    fn test_record_exchange_duration() {
        let metrics = PrometheusMetrics::new();

        // Should not panic
        metrics.record_exchange_duration("test-route", Duration::from_millis(50));
        metrics.record_exchange_duration("test-route", Duration::from_millis(150));
        metrics.record_exchange_duration("other-route", Duration::from_secs(1));

        // Verify the metric is registered
        let output = metrics.gather();
        assert!(output.contains("camel_exchange_duration_seconds"));
        assert!(output.contains("test-route"));
    }

    #[test]
    fn test_set_queue_depth() {
        let metrics = PrometheusMetrics::new();

        // Should not panic
        metrics.set_queue_depth("test-route", 5);
        metrics.set_queue_depth("test-route", 10);
        metrics.set_queue_depth("other-route", 3);

        // Verify the metric is registered
        let output = metrics.gather();
        assert!(output.contains("camel_queue_depth"));
    }

    #[test]
    fn test_record_circuit_breaker_change() {
        let metrics = PrometheusMetrics::new();

        // Should not panic
        metrics.record_circuit_breaker_change("test-route", "closed", "open");
        metrics.record_circuit_breaker_change("test-route", "open", "half_open");
        metrics.record_circuit_breaker_change("test-route", "half_open", "closed");

        // Verify the metric is registered
        let output = metrics.gather();
        assert!(output.contains("camel_circuit_breaker_state"));
    }

    #[test]
    fn test_gather_returns_prometheus_format() {
        let metrics = PrometheusMetrics::new();

        // Record some metrics
        metrics.increment_exchanges("route-1");
        metrics.increment_errors("route-1", "timeout");
        metrics.set_queue_depth("route-1", 5);

        // Gather metrics
        let output = metrics.gather();

        // Verify output is valid Prometheus text format
        assert!(output.starts_with("# HELP") || output.starts_with("# TYPE"));
        assert!(output.contains("camel_exchanges_total"));
        assert!(output.contains("camel_errors_total"));
        assert!(output.contains("camel_queue_depth"));

        // Verify labels use 'route' not 'route_id'
        assert!(output.contains("route=\"route-1\""));
        assert!(!output.contains("route_id=\"route-1\""));
    }

    #[test]
    fn test_metrics_collector_trait_object() {
        // Verify PrometheusMetrics can be used as a trait object
        let metrics: Arc<dyn MetricsCollector> = Arc::new(PrometheusMetrics::new());

        // All methods should work without panicking
        metrics.increment_exchanges("test-route");
        metrics.increment_errors("test-route", "test-error");
        metrics.record_exchange_duration("test-route", Duration::from_millis(100));
        metrics.set_queue_depth("test-route", 5);
        metrics.record_circuit_breaker_change("test-route", "closed", "open");
    }
}
