use std::time::Duration;

use camel_api::metrics::MetricsCollector;
use prometheus::{CounterVec, GaugeVec, HistogramVec, Opts, Registry, TextEncoder};

/// Normalize a dynamic metric name for Prometheus: prepend `camel_` if missing,
/// replace invalid chars with `_`, prefix `_` if it starts with a digit.
/// Prometheus names must match `^[a-zA-Z_:][a-zA-Z0-9_:]*$`.
fn normalize_prom_name(name: &str) -> String {
    let prefixed = if name.starts_with("camel_") {
        name.to_string()
    } else {
        format!("camel_{name}")
    };
    let sanitized: String = prefixed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
                c
            } else {
                '_'
            }
        })
        .collect();
    sanitized
}

/// Sort label pairs by key so `with_label_values` (positional) binds correctly
/// regardless of caller order.
fn sort_label_pairs<'a>(labels: &'a [(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut pairs = labels.to_vec();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    pairs
}

/// Validate a counter value: must be finite, non-negative, and integer-valued.
/// NaN corrupts Prometheus counters in release; negative silently corrupts;
/// fractional values diverge across backends (Prometheus keeps f64, OTel truncates).
fn counter_value_ok(v: f64) -> bool {
    !v.is_nan() && v >= 0.0 && v.fract() == 0.0
}

/// A dynamically-created counter plus the label-keys frozen at first observation.
struct DynCounter {
    cv: CounterVec,
    keys: Vec<String>,
}

/// A dynamically-created histogram plus the label-keys frozen at first observation.
struct DynHistogram {
    hv: HistogramVec,
    keys: Vec<String>,
}

/// Check whether the incoming label keys match the frozen key-set.
fn keys_match(frozen: &[String], incoming: &[(&str, &str)]) -> bool {
    if frozen.len() != incoming.len() {
        return false;
    }
    frozen.iter().zip(incoming.iter()).all(|(f, (k, _))| f == k)
}

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
    /// Lazy cache for dynamic counters keyed by normalized name.
    /// `None` = tombstone (registration failed; skip silently on subsequent calls).
    dyn_counters: dashmap::DashMap<String, Option<DynCounter>>,
    /// Lazy cache for dynamic histograms keyed by normalized name.
    /// `None` = tombstone (registration failed; skip silently on subsequent calls).
    dyn_histograms: dashmap::DashMap<String, Option<DynHistogram>>,
    /// Names that have already emitted a `warn!` — dedup so a bad metric
    /// logs once, not per-call (log-flood prevention).
    warned: dashmap::DashSet<String>,
}

impl PrometheusMetrics {
    /// Creates a new PrometheusMetrics instance with all metrics registered.
    ///
    /// # Panics
    ///
    /// Panics if metric creation or registration fails. This can only happen if:
    /// - A metric name is invalid (must match `^[a-zA-Z_:][a-zA-Z0-9_:]*$`). All names are
    ///   hardcoded below and comply with this requirement by convention.
    /// - A metric is registered twice. This is impossible here because each call creates a
    ///   fresh [`Registry`].
    ///
    /// Since both conditions are static invariants enforced by code review, these `expect()`
    /// calls are intentional and will never fail in practice.
    pub fn new() -> Self {
        let registry = Registry::new();

        // Create and register exchanges_total counter
        let exchanges_total = CounterVec::new(
            Opts::new("exchanges_total", "Total number of exchanges processed").namespace("camel"),
            &["route"],
        )
        .expect("Failed to create exchanges_total counter"); // allow-unwrap
        registry
            .register(Box::new(exchanges_total.clone()))
            .expect("Failed to register exchanges_total counter"); // allow-unwrap

        // Create and register errors_total counter
        let errors_total = CounterVec::new(
            Opts::new("errors_total", "Total number of errors").namespace("camel"),
            &["route", "error_type"],
        )
        .expect("Failed to create errors_total counter"); // allow-unwrap
        registry
            .register(Box::new(errors_total.clone()))
            .expect("Failed to register errors_total counter"); // allow-unwrap

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
        .expect("Failed to create exchange_duration_seconds histogram"); // allow-unwrap
        registry
            .register(Box::new(exchange_duration_seconds.clone()))
            .expect("Failed to register exchange_duration_seconds histogram"); // allow-unwrap

        // Create and register queue_depth gauge
        let queue_depth = GaugeVec::new(
            Opts::new("queue_depth", "Current queue depth").namespace("camel"),
            &["route"],
        )
        .expect("Failed to create queue_depth gauge"); // allow-unwrap
        registry
            .register(Box::new(queue_depth.clone()))
            .expect("Failed to register queue_depth gauge"); // allow-unwrap

        // Create and register circuit_breaker_state gauge
        let circuit_breaker_state = GaugeVec::new(
            Opts::new(
                "circuit_breaker_state",
                "Circuit breaker state (0=closed, 1=open, 2=half_open)",
            )
            .namespace("camel"),
            &["route"],
        )
        .expect("Failed to create circuit_breaker_state gauge"); // allow-unwrap
        registry
            .register(Box::new(circuit_breaker_state.clone()))
            .expect("Failed to register circuit_breaker_state gauge"); // allow-unwrap

        Self {
            registry,
            exchanges_total,
            errors_total,
            exchange_duration_seconds,
            queue_depth,
            circuit_breaker_state,
            dyn_counters: dashmap::DashMap::new(),
            dyn_histograms: dashmap::DashMap::new(),
            warned: dashmap::DashSet::new(),
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

    fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        // Guard value first — cheap check, no cache access needed for bad values.
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

        let normalized = normalize_prom_name(name);
        if normalized != name && self.warned.insert(format!("sanitize:{name}")) {
            tracing::warn!(name, %normalized, "metric name sanitized for prometheus");
        }
        let sorted = sort_label_pairs(labels);
        let values: Vec<&str> = sorted.iter().map(|(_, v)| *v).collect();

        use dashmap::mapref::entry::Entry;
        match self.dyn_counters.entry(normalized.clone()) {
            Entry::Occupied(o) => match o.get() {
                Some(dc) => {
                    if keys_match(&dc.keys, &sorted) {
                        dc.cv.with_label_values(&values).inc_by(value);
                    } else if self.warned.insert(name.to_string()) {
                        tracing::warn!(
                            name,
                            "dynamic counter label arity/key drift; observation dropped \
                             (further drift for this name will be silent)"
                        );
                    }
                }
                None => { /* tombstone — skip silently */ }
            },
            Entry::Vacant(v) => {
                let keys: Vec<String> = sorted.iter().map(|(k, _)| (*k).to_string()).collect();
                let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                let cv = match CounterVec::new(Opts::new(&normalized, "Dynamic counter"), &key_refs)
                {
                    Ok(cv) => cv,
                    Err(_) => {
                        v.insert(None);
                        if self.warned.insert(name.to_string()) {
                            tracing::warn!(name, "dynamic counter creation failed; tombstoned");
                        }
                        return;
                    }
                };
                match self.registry.register(Box::new(cv.clone())) {
                    Ok(()) => {
                        cv.with_label_values(&values).inc_by(value);
                        v.insert(Some(DynCounter { cv, keys }));
                    }
                    Err(_) => {
                        v.insert(None);
                        if self.warned.insert(name.to_string()) {
                            tracing::warn!(
                                name,
                                "dynamic counter registration failed (possible name collision); \
                                 tombstoned"
                            );
                        }
                    }
                }
            }
        }
    }

    fn record_histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        // Histograms only reject NaN (fractional values are legitimate).
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

        let normalized = normalize_prom_name(name);
        if normalized != name && self.warned.insert(format!("sanitize:{name}")) {
            tracing::warn!(name, %normalized, "metric name sanitized for prometheus");
        }
        let sorted = sort_label_pairs(labels);
        let values: Vec<&str> = sorted.iter().map(|(_, v)| *v).collect();

        use dashmap::mapref::entry::Entry;
        match self.dyn_histograms.entry(normalized.clone()) {
            Entry::Occupied(o) => match o.get() {
                Some(dh) => {
                    if keys_match(&dh.keys, &sorted) {
                        dh.hv.with_label_values(&values).observe(value);
                    } else if self.warned.insert(name.to_string()) {
                        tracing::warn!(
                            name,
                            "dynamic histogram label arity/key drift; observation dropped"
                        );
                    }
                }
                None => { /* tombstone — skip silently */ }
            },
            Entry::Vacant(v) => {
                let keys: Vec<String> = sorted.iter().map(|(k, _)| (*k).to_string()).collect();
                let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                let hv = match HistogramVec::new(
                    prometheus::HistogramOpts {
                        common_opts: Opts::new(&normalized, "Dynamic histogram"),
                        buckets: vec![
                            0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                        ],
                    },
                    &key_refs,
                ) {
                    Ok(hv) => hv,
                    Err(_) => {
                        v.insert(None);
                        if self.warned.insert(name.to_string()) {
                            tracing::warn!(name, "dynamic histogram creation failed; tombstoned");
                        }
                        return;
                    }
                };
                match self.registry.register(Box::new(hv.clone())) {
                    Ok(()) => {
                        hv.with_label_values(&values).observe(value);
                        v.insert(Some(DynHistogram { hv, keys }));
                    }
                    Err(_) => {
                        v.insert(None);
                        if self.warned.insert(name.to_string()) {
                            tracing::warn!(
                                name,
                                "dynamic histogram registration failed; tombstoned"
                            );
                        }
                    }
                }
            }
        }
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

    #[test]
    fn test_record_counter_dynamic_basic() {
        let metrics = PrometheusMetrics::new();
        metrics.record_counter("exec_spawns_total", 1.0, &[("route", "r1")]);
        metrics.record_counter("exec_spawns_total", 1.0, &[("route", "r1")]);
        let out = metrics.gather();
        assert!(
            out.contains("camel_exec_spawns_total"),
            "normalized name missing: {out}"
        );
        assert!(out.contains("route=\"r1\""));
    }

    #[test]
    fn test_record_counter_multi_label_ordering_invariant() {
        let metrics = PrometheusMetrics::new();
        // Same metric, labels in different orders — must land on the same series.
        metrics.record_counter(
            "exec_policy_denials_total",
            1.0,
            &[("reason", "denied"), ("route", "r1")],
        );
        metrics.record_counter(
            "exec_policy_denials_total",
            1.0,
            &[("route", "r1"), ("reason", "denied")],
        );
        let out = metrics.gather();
        assert!(out.contains("camel_exec_policy_denials_total"));
        // Both observations recorded on one series (count == 2). Extract the
        // numeric value robustly from the Prometheus text line.
        let count: f64 = out
            .lines()
            .filter(|l| {
                l.contains("camel_exec_policy_denials_total") && l.contains("reason=\"denied\"")
            })
            .filter_map(|l| l.rsplit(' ').next().and_then(|v| v.parse::<f64>().ok()))
            .sum();
        assert_eq!(count, 2.0, "expected count 2, got {count}");
    }

    #[test]
    fn test_record_counter_arity_drift_dropped() {
        let metrics = PrometheusMetrics::new();
        // First observation freezes key-set {route}.
        metrics.record_counter("drift_total", 1.0, &[("route", "r1")]);
        // Second observation adds a key — must be dropped (arity mismatch).
        metrics.record_counter("drift_total", 1.0, &[("route", "r1"), ("extra", "x")]);
        let out = metrics.gather();
        // Only the first observation recorded (count == 1).
        let count: f64 = out
            .lines()
            .filter(|l| l.contains("camel_drift_total"))
            .filter_map(|l| l.rsplit(' ').next().and_then(|v| v.parse::<f64>().ok()))
            .sum();
        assert_eq!(count, 1.0, "expected count 1 (drift dropped), got {count}");
    }

    #[test]
    fn test_record_counter_value_guards() {
        let metrics = PrometheusMetrics::new();
        metrics.record_counter("bad_total", f64::NAN, &[("route", "r1")]);
        metrics.record_counter("bad_total", -1.0, &[("route", "r1")]);
        metrics.record_counter("bad_total", 1.5, &[("route", "r1")]);
        // All values rejected — metric must NOT appear in gather output.
        let out = metrics.gather();
        assert!(
            !out.contains("camel_bad_total"),
            "value guards should prevent cache population; found metric in output"
        );
    }

    #[test]
    fn test_record_counter_tombstone_on_collision() {
        let metrics = PrometheusMetrics::new();
        // "camel_exchanges_total" already registered as a fixed metric.
        // A dynamic call with the same normalized name must tombstone (AlreadyReg),
        // and NOT panic.
        metrics.record_counter("exchanges_total", 1.0, &[("route", "r1")]);
        // Call again — tombstone should make this a silent no-op (no re-attempt).
        metrics.record_counter("exchanges_total", 1.0, &[("route", "r2")]);
        // Verify the tombstone was inserted (None = registration failed).
        let tombstoned = metrics
            .dyn_counters
            .get("camel_exchanges_total")
            .map(|e| e.is_none())
            .unwrap_or(false);
        assert!(tombstoned, "expected tombstone for colliding metric name");
    }

    #[test]
    fn test_record_counter_warn_dedup() {
        let metrics = PrometheusMetrics::new();
        // Three bad-value calls for the same name.
        metrics.record_counter("dedup_total", f64::NAN, &[("route", "r1")]);
        metrics.record_counter("dedup_total", -1.0, &[("route", "r1")]);
        metrics.record_counter("dedup_total", 1.5, &[("route", "r1")]);
        // The warned set should contain the name exactly once (dedup).
        assert!(
            metrics.warned.contains("dedup_total"),
            "warned set should contain the offending name"
        );
    }

    #[test]
    fn test_record_counter_fixed_and_dynamic_in_one_gather() {
        let metrics = PrometheusMetrics::new();
        metrics.increment_exchanges("r1"); // fixed metric
        metrics.record_counter("dyn_total", 1.0, &[("route", "r1")]); // dynamic metric
        let out = metrics.gather();
        assert!(
            out.contains("camel_exchanges_total"),
            "fixed metric missing"
        );
        assert!(out.contains("camel_dyn_total"), "dynamic metric missing");
    }

    #[test]
    fn test_record_counter_trait_object_dispatch() {
        // Retain a concrete handle to verify post-call state; Arc::downcast
        // requires `Any` which MetricsCollector does not have.
        let concrete = Arc::new(PrometheusMetrics::new());
        let dynref: Arc<dyn MetricsCollector> = concrete.clone();
        dynref.record_counter("trait_total", 1.0, &[("route", "r1")]);
        // Verify it dispatched to the real impl (not the no-op default).
        let out = concrete.gather();
        assert!(out.contains("camel_trait_total"));
    }

    #[test]
    fn test_record_counter_concurrent_no_panic() {
        use std::thread;
        let metrics = Arc::new(PrometheusMetrics::new());
        let mut handles = Vec::new();
        for i in 0..4 {
            let m = Arc::clone(&metrics);
            handles.push(thread::spawn(move || {
                let route = format!("route-{i}");
                for _ in 0..100 {
                    m.record_counter("concurrent_total", 1.0, &[("route", &route)]);
                }
            }));
        }
        for h in handles {
            h.join().expect("thread panicked under contention");
        }
        // Verify the metric was recorded (total == 400).
        let out = metrics.gather();
        let total: f64 = out
            .lines()
            .filter(|l| l.contains("camel_concurrent_total"))
            .filter_map(|l| l.rsplit(' ').next().and_then(|v| v.parse::<f64>().ok()))
            .sum();
        assert_eq!(total, 400.0, "expected 400 total observations, got {total}");
    }

    #[test]
    fn test_record_histogram_dynamic_basic() {
        let metrics = PrometheusMetrics::new();
        metrics.record_histogram("exec_duration_secs", 0.15, &[("route", "r1")]);
        metrics.record_histogram("exec_duration_secs", 0.5, &[("route", "r1")]);
        let out = metrics.gather();
        assert!(
            out.contains("camel_exec_duration_secs"),
            "normalized name missing: {out}"
        );
        assert!(out.contains("route=\"r1\""));
        // Prometheus text format emits histogram count as a _count suffixed series.
        assert!(
            out.contains("camel_exec_duration_secs_count"),
            "expected histogram count series, got: {out}"
        );
    }

    #[test]
    fn test_record_histogram_nan_rejected() {
        let metrics = PrometheusMetrics::new();
        metrics.record_histogram("nan_hist", f64::NAN, &[("route", "r1")]);
        let out = metrics.gather();
        assert!(
            !out.contains("camel_nan_hist"),
            "NaN value should not create a histogram; found it in output: {out}"
        );
    }

    #[test]
    fn test_record_histogram_accepts_fractional() {
        let metrics = PrometheusMetrics::new();
        // Histograms legitimately accept fractional values (durations, cost).
        metrics.record_histogram("frac_hist", 1.5, &[("route", "r1")]);
        let out = metrics.gather();
        assert!(out.contains("camel_frac_hist"));
    }

    #[test]
    fn test_record_histogram_trait_object_dispatch() {
        let concrete = Arc::new(PrometheusMetrics::new());
        let dynref: Arc<dyn MetricsCollector> = concrete.clone();
        dynref.record_histogram("trait_hist", 0.25, &[("route", "r1")]);
        let out = concrete.gather();
        assert!(out.contains("camel_trait_hist"));
    }
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn normalize_prom_name_prepends_camel_prefix() {
        assert_eq!(
            normalize_prom_name("exec_spawns_total"),
            "camel_exec_spawns_total"
        );
    }

    #[test]
    fn normalize_prom_name_keeps_existing_camel_prefix() {
        assert_eq!(normalize_prom_name("camel_foo_total"), "camel_foo_total");
    }

    #[test]
    fn normalize_prom_name_replaces_invalid_chars() {
        // dots and spaces are invalid in Prometheus metric names
        assert_eq!(
            normalize_prom_name("my.metric name"),
            "camel_my_metric_name"
        );
    }

    #[test]
    fn normalize_prom_name_numeric_name_gets_camel_prefix() {
        assert_eq!(normalize_prom_name("123foo"), "camel_123foo");
    }

    #[test]
    fn sort_label_pairs_orders_by_key() {
        let labels = [("route", "r1"), ("reason", "denied")];
        let sorted = sort_label_pairs(&labels);
        assert_eq!(sorted, vec![("reason", "denied"), ("route", "r1")]);
    }

    #[test]
    fn sort_label_pairs_already_sorted_is_noop() {
        let labels = [("code", "0"), ("route", "r1")];
        let sorted = sort_label_pairs(&labels);
        assert_eq!(sorted, vec![("code", "0"), ("route", "r1")]);
    }

    #[test]
    fn counter_value_ok_accepts_positive_integers() {
        assert!(counter_value_ok(1.0));
        assert!(counter_value_ok(5.0));
        assert!(counter_value_ok(0.0));
    }

    #[test]
    fn counter_value_ok_rejects_nan_negative_and_fractional() {
        assert!(!counter_value_ok(f64::NAN));
        assert!(!counter_value_ok(-1.0));
        assert!(!counter_value_ok(1.5));
    }
}
