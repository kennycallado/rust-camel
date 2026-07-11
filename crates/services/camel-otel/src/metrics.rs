//! OpenTelemetry metrics implementation for rust-camel
//!
//! This module provides `OtelMetrics`, an implementation of the `MetricsCollector`
//! trait that integrates with OpenTelemetry for distributed metrics collection.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use camel_api::metrics::MetricsCollector;
use opentelemetry::InstrumentationScope;
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::metrics::{Counter, Histogram, Meter, UpDownCounter};

/// Normalize a dynamic metric name for OTel: prepend `camel.` if missing,
/// convert `_` to `.` to match the dotted convention (`camel.exchanges.total`).
fn normalize_otel_name(name: &str) -> String {
    let dotted = name.replace('_', ".");
    if dotted.starts_with("camel.") {
        dotted
    } else {
        format!("camel.{dotted}")
    }
}

// Must stay identical to camel-prometheus counterparts (spec D5/D10 symmetry).

/// Validate a counter value for OTel: must be finite, non-negative, integer.
fn counter_value_ok(v: f64) -> bool {
    !v.is_nan() && v >= 0.0 && v.fract() == 0.0
}

/// Sort label pairs by key (mirrors the Prometheus path for behavioral parity).
fn sort_label_pairs<'a>(labels: &'a [(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut pairs = labels.to_vec();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    pairs
}

mod metric_names {
    pub const EXCHANGES_TOTAL: &str = "camel.exchanges.total";
    pub const ERRORS_TOTAL: &str = "camel.errors.total";
    pub const EXCHANGE_DURATION_SECONDS: &str = "camel.exchange.duration.seconds";
    pub const QUEUE_DEPTH: &str = "camel.queue.depth";
    pub const CIRCUIT_BREAKER_STATE: &str = "camel.circuit.breaker.state";
}

mod attribute_keys {
    pub const ROUTE_ID: &str = "route.id";
    pub const ERROR_TYPE: &str = "error.type";
}

struct MetricInstruments {
    exchanges_total: Counter<u64>,
    errors_total: Counter<u64>,
    exchange_duration_seconds: Histogram<f64>,
    queue_depth: UpDownCounter<i64>,
    circuit_breaker_state: UpDownCounter<i64>,
}

/// OpenTelemetry metrics collector for rust-camel
///
/// Uses lazy initialization - instruments are created on first use to ensure
/// the global meter provider is configured (by OtelService::start()) before use.
pub struct OtelMetrics {
    service_name: Arc<str>,
    instruments: OnceLock<MetricInstruments>,
    /// Cached meter for dynamic instruments — resolved once to prevent
    /// provider fragmentation (D5). Must use the same scope as `instruments`.
    meter: OnceLock<opentelemetry::metrics::Meter>,
    queue_depths: std::sync::Mutex<HashMap<String, i64>>,
    cb_states: std::sync::Mutex<HashMap<String, i64>>,
    /// Lazy cache for dynamic counters keyed by normalized name.
    dyn_counters: dashmap::DashMap<String, Option<Counter<u64>>>,
    /// Lazy cache for dynamic histograms keyed by normalized name.
    dyn_histograms: dashmap::DashMap<String, Option<Histogram<f64>>>,
    /// Names that have already emitted a `warn!` — dedup per D7.
    warned: dashmap::DashSet<String>,
}

impl OtelMetrics {
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into().into(),
            instruments: OnceLock::new(),
            meter: OnceLock::new(),
            queue_depths: std::sync::Mutex::new(HashMap::new()),
            cb_states: std::sync::Mutex::new(HashMap::new()),
            dyn_counters: dashmap::DashMap::new(),
            dyn_histograms: dashmap::DashMap::new(),
            warned: dashmap::DashSet::new(),
        }
    }

    fn instruments(&self) -> &MetricInstruments {
        self.instruments.get_or_init(|| {
            let meter = self.meter();
            MetricInstruments {
                exchanges_total: meter
                    .u64_counter(metric_names::EXCHANGES_TOTAL)
                    .with_description("Total number of exchanges processed")
                    .with_unit("{exchange}")
                    .build(),
                errors_total: meter
                    .u64_counter(metric_names::ERRORS_TOTAL)
                    .with_description("Total number of errors")
                    .with_unit("{error}")
                    .build(),
                exchange_duration_seconds: meter
                    .f64_histogram(metric_names::EXCHANGE_DURATION_SECONDS)
                    .with_description("Exchange processing duration in seconds")
                    .with_unit("s")
                    .build(),
                queue_depth: meter
                    .i64_up_down_counter(metric_names::QUEUE_DEPTH)
                    .with_description("Current queue depth")
                    .with_unit("{item}")
                    .build(),
                circuit_breaker_state: meter
                    .i64_up_down_counter(metric_names::CIRCUIT_BREAKER_STATE)
                    .with_description("Circuit breaker state (0=closed, 1=open, 2=half_open)")
                    .with_unit("{state}")
                    .build(),
            }
        })
    }
    /// Returns the cached meter for creating dynamic instruments.
    /// Resolved exactly once to prevent provider fragmentation (D5).
    /// Reuses the same InstrumentationScope as the fixed instruments so
    /// dynamic and fixed metrics share scope (no fragmentation).
    fn meter(&self) -> &Meter {
        self.meter.get_or_init(|| {
            global::meter_with_scope(
                InstrumentationScope::builder(self.service_name.to_string()).build(),
            )
        })
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
        self.instruments()
            .exchange_duration_seconds
            .record(duration_secs, &attributes);
    }

    fn increment_errors(&self, route_id: &str, error_type: &str) {
        let attributes = [
            KeyValue::new(attribute_keys::ROUTE_ID, route_id.to_string()),
            KeyValue::new(attribute_keys::ERROR_TYPE, error_type.to_string()),
        ];
        self.instruments().errors_total.add(1, &attributes);
    }

    fn increment_exchanges(&self, route_id: &str) {
        let attributes = [KeyValue::new(
            attribute_keys::ROUTE_ID,
            route_id.to_string(),
        )];
        self.instruments().exchanges_total.add(1, &attributes);
    }

    fn set_queue_depth(&self, route_id: &str, depth: usize) {
        let depth_i64 = depth as i64;
        let mut map = self.queue_depths.lock().unwrap_or_else(|e| e.into_inner());
        let prev = map.insert(route_id.to_string(), depth_i64).unwrap_or(0);
        let delta = depth_i64 - prev;
        drop(map);

        let attributes = [KeyValue::new(
            attribute_keys::ROUTE_ID,
            route_id.to_string(),
        )];
        self.instruments().queue_depth.add(delta, &attributes);
    }

    fn record_circuit_breaker_change(&self, route_id: &str, _from: &str, to: &str) {
        let to_value = match to.to_lowercase().as_str() {
            "closed" => 0i64,
            "open" => 1i64,
            "half_open" | "halfopen" => 2i64,
            _ => return,
        };

        let mut map = self.cb_states.lock().unwrap_or_else(|e| e.into_inner());
        let prev = map.insert(route_id.to_string(), to_value).unwrap_or(0);
        let delta = to_value - prev;
        drop(map);

        let attributes = [KeyValue::new(
            attribute_keys::ROUTE_ID,
            route_id.to_string(),
        )];

        self.instruments()
            .circuit_breaker_state
            .add(delta, &attributes);
    }

    fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        if !counter_value_ok(value) {
            if self.warned.insert(name.to_string()) {
                tracing::warn!(
                    name,
                    value,
                    "dynamic counter value rejected (NaN/negative/non-integer); \
                     further rejections for this name will be silent"
                );
            }
            return;
        }
        let normalized = normalize_otel_name(name);
        if normalized != name && self.warned.insert(format!("sanitize:{name}")) {
            tracing::warn!(name, %normalized, "metric name sanitized for otel");
        }
        let sorted = sort_label_pairs(labels);
        let value_u64 = value as u64;
        let attributes: Vec<KeyValue> = sorted
            .iter()
            .map(|(k, v)| KeyValue::new((*k).to_string(), (*v).to_string()))
            .collect();

        use dashmap::mapref::entry::Entry;
        match self.dyn_counters.entry(normalized.clone()) {
            Entry::Occupied(o) => {
                if let Some(counter) = o.get().as_ref() {
                    counter.add(value_u64, &attributes);
                }
                // None = tombstone → skip silently
            }
            Entry::Vacant(v) => {
                let counter = self.meter().u64_counter(normalized.clone()).build();
                counter.add(value_u64, &attributes);
                v.insert(Some(counter));
            }
        }
    }

    fn record_histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        if value.is_nan() {
            if self.warned.insert(name.to_string()) {
                tracing::warn!(
                    name,
                    "dynamic histogram value rejected (NaN); \
                     further NaN for this name will be silent"
                );
            }
            return;
        }
        let normalized = normalize_otel_name(name);
        if normalized != name && self.warned.insert(format!("sanitize:{name}")) {
            tracing::warn!(name, %normalized, "metric name sanitized for otel");
        }
        let sorted = sort_label_pairs(labels);
        let attributes: Vec<KeyValue> = sorted
            .iter()
            .map(|(k, v)| KeyValue::new((*k).to_string(), (*v).to_string()))
            .collect();

        use dashmap::mapref::entry::Entry;
        match self.dyn_histograms.entry(normalized.clone()) {
            Entry::Occupied(o) => {
                if let Some(histogram) = o.get().as_ref() {
                    histogram.record(value, &attributes);
                }
            }
            Entry::Vacant(v) => {
                let histogram = self.meter().f64_histogram(normalized.clone()).build();
                histogram.record(value, &attributes);
                v.insert(Some(histogram));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_create_otel_metrics() {
        let metrics = OtelMetrics::new("test-service");
        // Instruments are created lazily
        let _ = metrics.instruments();
    }

    #[test]
    fn test_default_implementation() {
        let metrics = OtelMetrics::default();
        let _ = metrics.instruments();
    }

    #[test]
    fn test_metrics_with_noop_provider() {
        let metrics = OtelMetrics::new("test-service");

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
        let map = metrics.cb_states.lock().unwrap_or_else(|e| e.into_inner());

        // Initial state: no entry
        assert!(!map.contains_key(route));
        drop(map);

        // closed→open: delta should be +1 (0→1)
        metrics.record_circuit_breaker_change(route, "closed", "open");
        let map = metrics.cb_states.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(*map.get(route).unwrap(), 1);
        drop(map);

        // open→half_open: delta should be +1 (1→2)
        metrics.record_circuit_breaker_change(route, "open", "half_open");
        let map = metrics.cb_states.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(*map.get(route).unwrap(), 2);
        drop(map);

        // half_open→closed: delta should be -2 (2→0)
        metrics.record_circuit_breaker_change(route, "half_open", "closed");
        let map = metrics.cb_states.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(*map.get(route).unwrap(), 0);
        drop(map);

        // Test alternate spellings
        metrics.record_circuit_breaker_change(route, "closed", "halfopen");
        let map = metrics.cb_states.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(*map.get(route).unwrap(), 2);
        drop(map);

        // Test unknown state (should not panic and should not change state)
        metrics.record_circuit_breaker_change(route, "closed", "unknown");
        let map = metrics.cb_states.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(*map.get(route).unwrap(), 2); // unchanged
    }

    #[test]
    fn test_metrics_under_contention_no_panic() {
        use std::sync::Arc;
        use std::thread;

        let metrics = Arc::new(OtelMetrics::new("test-contention"));
        let mut handles = Vec::new();

        for i in 0..4 {
            let m = Arc::clone(&metrics);
            handles.push(thread::spawn(move || {
                let route = format!("route-{i}");
                for j in 0..100 {
                    m.increment_exchanges(&route);
                    m.increment_errors(&route, "timeout");
                    m.record_exchange_duration(&route, Duration::from_millis(j));
                    m.set_queue_depth(&route, j as usize);
                    m.record_circuit_breaker_change(
                        &route,
                        if j % 2 == 0 { "closed" } else { "open" },
                        if j % 2 == 0 { "open" } else { "closed" },
                    );
                }
            }));
        }

        for h in handles {
            h.join().expect("thread should not panic under contention");
        }
    }

    #[test]
    fn test_record_counter_dynamic_basic() {
        let metrics = OtelMetrics::new("test-service");
        // Under a no-op provider these should not panic and should record.
        metrics.record_counter("exec_spawns_total", 1.0, &[("route", "r1")]);
        metrics.record_counter("exec_spawns_total", 1.0, &[("route", "r2")]);
        // Verify the cache was populated.
        assert!(metrics.dyn_counters.contains_key("camel.exec.spawns.total"));
    }

    #[test]
    fn test_record_counter_multi_label() {
        let metrics = OtelMetrics::new("test-service");
        metrics.record_counter(
            "exec_policy_denials_total",
            1.0,
            &[("reason", "denied"), ("route", "r1")],
        );
        assert!(
            metrics
                .dyn_counters
                .contains_key("camel.exec.policy.denials.total")
        );
    }

    #[test]
    fn test_record_counter_value_guards() {
        let metrics = OtelMetrics::new("test-service");
        metrics.record_counter("bad_total", f64::NAN, &[("route", "r1")]);
        metrics.record_counter("bad_total", -1.0, &[("route", "r1")]);
        metrics.record_counter("bad_total", 1.5, &[("route", "r1")]);
        // Rejected values must NOT create a cache entry.
        assert!(!metrics.dyn_counters.contains_key("camel.bad.total"));
    }

    #[test]
    fn test_record_histogram_dynamic_basic() {
        let metrics = OtelMetrics::new("test-service");
        metrics.record_histogram("exec_duration_secs", 0.15, &[("route", "r1")]);
        metrics.record_histogram("exec_duration_secs", 1.5, &[("route", "r1")]);
        assert!(
            metrics
                .dyn_histograms
                .contains_key("camel.exec.duration.secs")
        );
    }

    #[test]
    fn test_record_histogram_nan_rejected() {
        let metrics = OtelMetrics::new("test-service");
        metrics.record_histogram("nan_hist", f64::NAN, &[("route", "r1")]);
        assert!(!metrics.dyn_histograms.contains_key("camel.nan.hist"));
    }

    #[test]
    fn test_record_counter_trait_object_dispatch() {
        // Retain a concrete handle to verify post-call state; Arc::downcast
        // requires `Any` which MetricsCollector does not have.
        let concrete = Arc::new(OtelMetrics::new("test-service"));
        let dynref: Arc<dyn MetricsCollector> = concrete.clone();
        dynref.record_counter("trait_total", 1.0, &[("route", "r1")]);
        // Proves the override dispatched (the no-op default would leave the cache empty).
        assert!(
            concrete.dyn_counters.contains_key("camel.trait.total"),
            "trait-object dispatch did not populate cache"
        );
    }

    #[test]
    fn test_dynamic_meter_cached_in_once_lock() {
        let metrics = OtelMetrics::new("test-service");
        // Trigger dynamic instrument creation.
        metrics.record_counter("cache_check_total", 1.0, &[("route", "r1")]);
        // Verify the meter OnceLock is populated (resolved exactly once).
        assert!(
            metrics.meter.get().is_some(),
            "meter should be cached in OnceLock after first dynamic instrument"
        );
    }

    #[test]
    fn test_record_counter_warn_dedup() {
        let metrics = OtelMetrics::new("test-service");
        metrics.record_counter("dedup_total", f64::NAN, &[("route", "r1")]);
        metrics.record_counter("dedup_total", -1.0, &[("route", "r1")]);
        metrics.record_counter("dedup_total", 1.5, &[("route", "r1")]);
        assert!(
            metrics.warned.contains("dedup_total"),
            "warned set should contain the offending name"
        );
    }

    #[test]
    fn test_dynamic_metrics_concurrent_no_panic() {
        use std::thread;
        let metrics = Arc::new(OtelMetrics::new("test-contention"));
        let mut handles = Vec::new();
        for i in 0..4 {
            let m = Arc::clone(&metrics);
            handles.push(thread::spawn(move || {
                let route = format!("route-{i}");
                for _ in 0..100 {
                    m.record_counter("concurrent_total", 1.0, &[("route", &route)]);
                    m.record_histogram("concurrent_hist", 0.1, &[("route", &route)]);
                }
            }));
        }
        for h in handles {
            h.join().expect("thread panicked under contention");
        }
        assert!(metrics.dyn_counters.contains_key("camel.concurrent.total"));
        assert!(metrics.dyn_histograms.contains_key("camel.concurrent.hist"));
    }
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn normalize_otel_name_prepends_camel_dot() {
        assert_eq!(
            normalize_otel_name("exec_spawns_total"),
            "camel.exec.spawns.total"
        );
    }

    #[test]
    fn normalize_otel_name_keeps_existing_camel_prefix() {
        assert_eq!(
            normalize_otel_name("camel.exchanges.total"),
            "camel.exchanges.total"
        );
    }

    #[test]
    fn normalize_otel_name_partial_camel_prefix() {
        // "camel_foo_bar" → camel.foo.bar (underscore form gets converted)
        assert_eq!(normalize_otel_name("camel_foo_bar"), "camel.foo.bar");
    }
}
