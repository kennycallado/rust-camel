//! OpenTelemetry metrics implementation for rust-camel
//!
//! This module provides `OtelMetrics`, an implementation of the `MetricsCollector`
//! trait that integrates with OpenTelemetry for distributed metrics collection.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use camel_api::metrics::MetricsCollector;
use opentelemetry::global;
use opentelemetry::metrics::{Counter, Histogram, Meter, UpDownCounter};
use opentelemetry::KeyValue;

/// Metric names following camel naming conventions
mod metric_names {
    /// Total number of exchanges processed
    pub const EXCHANGES_TOTAL: &str = "camel.exchanges.total";
    /// Total number of errors
    pub const ERRORS_TOTAL: &str = "camel.errors.total";
    /// Exchange processing duration in seconds
    pub const EXCHANGE_DURATION_SECONDS: &str = "camel.exchange.duration.seconds";
    /// Current queue depth
    pub const QUEUE_DEPTH: &str = "camel.queue.depth";
    /// Circuit breaker state (0=closed, 1=open, 2=half_open)
    pub const CIRCUIT_BREAKER_STATE: &str = "camel.circuit.breaker.state";
}

/// Attribute keys for metrics labels (using OTel semantic conventions with dots)
mod attribute_keys {
    pub const ROUTE_ID: &str = "route.id";
    pub const ERROR_TYPE: &str = "error.type";
}

/// OpenTelemetry metrics collector for rust-camel
///
/// This struct implements the `MetricsCollector` trait and exports metrics
/// via OpenTelemetry. It uses the global meter provider, so metrics will be
/// exported according to the configured OTel pipeline.
///
/// # Example
///
/// ```rust,no_run
/// use camel_otel::OtelMetrics;
/// use camel_api::metrics::MetricsCollector;
/// use std::sync::Arc;
///
/// let metrics = OtelMetrics::new("my-service");
/// let collector: Arc<dyn MetricsCollector> = Arc::new(metrics);
/// ```
pub struct OtelMetrics {
    meter: Meter,
    exchanges_total: Counter<u64>,
    errors_total: Counter<u64>,
    exchange_duration_seconds: Histogram<f64>,
    queue_depth: UpDownCounter<i64>,
    circuit_breaker_state: UpDownCounter<i64>,
    /// Track previous queue depths per route for delta calculation
    queue_depths: Mutex<HashMap<String, i64>>,
    /// Track previous circuit breaker states per route+to_state for delta calculation
    cb_states: Mutex<HashMap<String, i64>>,
}

impl OtelMetrics {
    /// Creates a new `OtelMetrics` instance with metrics registered to the global meter provider.
    ///
    /// If called before `OtelService::start()`, this will use a no-op meter provider,
    /// which means metrics will not be exported. This is intentional - the actual
    /// meter provider should be configured via `OtelService` before starting.
    ///
    /// # Arguments
    ///
    /// * `service_name` - The name of the service, used as the meter name.
    ///
    /// # Note
    ///
    /// The service name is leaked to obtain a `'static` lifetime, which is required
    /// by the OpenTelemetry API. This is a small, one-time memory leak per unique
    /// service name.
    pub fn new(service_name: impl Into<String>) -> Self {
        // Box::leak to get 'static str - this is intentional and common for OTel
        let service_name: &'static str = Box::leak(service_name.into().into_boxed_str());
        let meter = global::meter(service_name);

        // Create counters
        let exchanges_total = meter
            .u64_counter(metric_names::EXCHANGES_TOTAL)
            .with_description("Total number of exchanges processed")
            .with_unit("{exchange}")
            .build();

        let errors_total = meter
            .u64_counter(metric_names::ERRORS_TOTAL)
            .with_description("Total number of errors")
            .with_unit("{error}")
            .build();

        // Create histogram for duration
        let exchange_duration_seconds = meter
            .f64_histogram(metric_names::EXCHANGE_DURATION_SECONDS)
            .with_description("Exchange processing duration in seconds")
            .with_unit("s")
            .build();

        // Create up-down counters for gauges
        let queue_depth = meter
            .i64_up_down_counter(metric_names::QUEUE_DEPTH)
            .with_description("Current queue depth")
            .with_unit("{item}")
            .build();

        let circuit_breaker_state = meter
            .i64_up_down_counter(metric_names::CIRCUIT_BREAKER_STATE)
            .with_description("Circuit breaker state (0=closed, 1=open, 2=half_open)")
            .with_unit("{state}")
            .build();

        Self {
            meter,
            exchanges_total,
            errors_total,
            exchange_duration_seconds,
            queue_depth,
            circuit_breaker_state,
            queue_depths: Mutex::new(HashMap::new()),
            cb_states: Mutex::new(HashMap::new()),
        }
    }

    /// Returns a reference to the underlying OpenTelemetry meter
    pub fn meter(&self) -> &Meter {
        &self.meter
    }
}

impl Default for OtelMetrics {
    fn default() -> Self {
        Self::new("rust-camel")
    }
}

impl MetricsCollector for OtelMetrics {
    fn record_exchange_duration(&self, route_id: &str, duration: Duration) {
        let duration_secs = duration.as_secs_f64();
        let attributes = [KeyValue::new(
            attribute_keys::ROUTE_ID,
            route_id.to_string(),
        )];
        self.exchange_duration_seconds
            .record(duration_secs, &attributes);
    }

    fn increment_errors(&self, route_id: &str, error_type: &str) {
        let attributes = [
            KeyValue::new(attribute_keys::ROUTE_ID, route_id.to_string()),
            KeyValue::new(attribute_keys::ERROR_TYPE, error_type.to_string()),
        ];
        self.errors_total.add(1, &attributes);
    }

    fn increment_exchanges(&self, route_id: &str) {
        let attributes = [KeyValue::new(
            attribute_keys::ROUTE_ID,
            route_id.to_string(),
        )];
        self.exchanges_total.add(1, &attributes);
    }

    fn set_queue_depth(&self, route_id: &str, depth: usize) {
        // UpDownCounter accumulates deltas, not absolute values.
        // Track previous value per route and compute the delta to emulate SET semantics.
        let depth_i64 = depth as i64;
        let mut map = self.queue_depths.lock().unwrap();
        let prev = map.insert(route_id.to_string(), depth_i64).unwrap_or(0);
        let delta = depth_i64 - prev;
        drop(map); // Release lock before recording

        let attributes = [KeyValue::new(
            attribute_keys::ROUTE_ID,
            route_id.to_string(),
        )];
        self.queue_depth.add(delta, &attributes);
    }

    fn record_circuit_breaker_change(&self, route_id: &str, _from: &str, to: &str) {
        // Circuit breaker states are mutually exclusive — a route is only ever in ONE state.
        // Map state names to numeric values (matching Prometheus implementation).
        let to_value = match to.to_lowercase().as_str() {
            "closed" => 0i64,
            "open" => 1i64,
            "half_open" | "halfopen" => 2i64,
            _ => return, // Unknown state - skip recording
        };

        // UpDownCounter accumulates deltas, not absolute values.
        // Track a single value per route (the current state value) and compute delta.
        // Use only route_id as the key since states are mutually exclusive.
        let mut map = self.cb_states.lock().unwrap();
        let prev = map.insert(route_id.to_string(), to_value).unwrap_or(0);
        let delta = to_value - prev;
        drop(map); // Release lock before recording

        let attributes = [KeyValue::new(
            attribute_keys::ROUTE_ID,
            route_id.to_string(),
        )];

        self.circuit_breaker_state.add(delta, &attributes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_create_otel_metrics() {
        let metrics = OtelMetrics::new("test-service");
        // Verify meter is accessible
        let _ = metrics.meter();
    }

    #[test]
    fn test_default_implementation() {
        let metrics = OtelMetrics::default();
        // Verify meter is accessible
        let _ = metrics.meter();
    }

    #[test]
    fn test_metrics_with_noop_provider() {
        // When no OTel provider is configured, the global meter provider returns a no-op
        // This test verifies that all methods work without panicking
        let metrics = OtelMetrics::new("test-service");

        // All methods should execute without panicking
        metrics.increment_exchanges("test-route");
        metrics.increment_exchanges("test-route");
        metrics.increment_exchanges("other-route");

        metrics.increment_errors("test-route", "timeout");
        metrics.increment_errors("test-route", "connection_failed");
        metrics.increment_errors("other-route", "timeout");

        metrics.record_exchange_duration("test-route", Duration::from_millis(50));
        metrics.record_exchange_duration("test-route", Duration::from_millis(150));
        metrics.record_exchange_duration("other-route", Duration::from_secs(1));

        metrics.set_queue_depth("test-route", 5);
        metrics.set_queue_depth("test-route", 10);
        metrics.set_queue_depth("other-route", 3);

        metrics.record_circuit_breaker_change("test-route", "closed", "open");
        metrics.record_circuit_breaker_change("test-route", "open", "half_open");
        metrics.record_circuit_breaker_change("test-route", "half_open", "closed");
    }

    #[test]
    fn test_metrics_collector_trait_object() {
        // Verify OtelMetrics can be used as a trait object
        let metrics: Arc<dyn MetricsCollector> = Arc::new(OtelMetrics::new("test-service"));

        // All methods should work without panicking
        metrics.increment_exchanges("test-route");
        metrics.increment_errors("test-route", "test-error");
        metrics.record_exchange_duration("test-route", Duration::from_millis(100));
        metrics.set_queue_depth("test-route", 5);
        metrics.record_circuit_breaker_change("test-route", "closed", "open");
    }

    #[test]
    fn test_record_exchange_duration_various_values() {
        let metrics = OtelMetrics::new("test-service");

        // Test various duration values
        metrics.record_exchange_duration("route", Duration::from_micros(100));
        metrics.record_exchange_duration("route", Duration::from_millis(1));
        metrics.record_exchange_duration("route", Duration::from_millis(100));
        metrics.record_exchange_duration("route", Duration::from_secs(1));
        metrics.record_exchange_duration("route", Duration::from_secs(10));
    }

    #[test]
    fn test_circuit_breaker_all_states() {
        let metrics = OtelMetrics::new("test-service");

        // Verify delta semantics: states are mutually exclusive.
        // After a full cycle closed→open→half_open→closed, the gauge should return to 0.
        //
        // closed→open: prev=0, to=1, delta=+1 → gauge=1
        // open→half_open: prev=1, to=2, delta=+1 → gauge=2
        // half_open→closed: prev=2, to=0, delta=-2 → gauge=0
        let route = "test-route";
        let map = metrics.cb_states.lock().unwrap();

        // Initial state: no entry
        assert!(!map.contains_key(route));
        drop(map);

        // closed→open: delta should be +1 (0→1)
        metrics.record_circuit_breaker_change(route, "closed", "open");
        let map = metrics.cb_states.lock().unwrap();
        assert_eq!(*map.get(route).unwrap(), 1);
        drop(map);

        // open→half_open: delta should be +1 (1→2)
        metrics.record_circuit_breaker_change(route, "open", "half_open");
        let map = metrics.cb_states.lock().unwrap();
        assert_eq!(*map.get(route).unwrap(), 2);
        drop(map);

        // half_open→closed: delta should be -2 (2→0)
        metrics.record_circuit_breaker_change(route, "half_open", "closed");
        let map = metrics.cb_states.lock().unwrap();
        assert_eq!(*map.get(route).unwrap(), 0);
        drop(map);

        // Test alternate spellings
        metrics.record_circuit_breaker_change(route, "closed", "halfopen");
        let map = metrics.cb_states.lock().unwrap();
        assert_eq!(*map.get(route).unwrap(), 2);
        drop(map);

        // Test unknown state (should not panic and should not change state)
        metrics.record_circuit_breaker_change(route, "closed", "unknown");
        let map = metrics.cb_states.lock().unwrap();
        assert_eq!(*map.get(route).unwrap(), 2); // unchanged
    }
}
