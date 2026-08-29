use std::time::Duration;

use camel_api::metrics::MetricsCollector;
use prometheus::{CounterVec, HistogramVec, Opts, Registry};

mod families;

use families::StaticFamilies;

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
    /// The pre-declared families; registration lives in `families`.
    families: StaticFamilies,
    /// Lazy cache for dynamic counters keyed by normalized name.
    /// `None` = tombstone (registration failed; skip silently on subsequent calls).
    dyn_counters: dashmap::DashMap<String, Option<DynCounter>>,
    /// Lazy cache for dynamic histograms keyed by normalized name.
    /// `None` = tombstone (registration failed; skip silently on subsequent calls).
    dyn_histograms: dashmap::DashMap<String, Option<DynHistogram>>,
    /// Names that have already emitted a `warn!` — dedup so a bad metric
    /// logs once, not per-call (log-flood prevention).
    warned: dashmap::DashSet<String>,
    /// Soft cap on the number of unique dynamic collector names
    /// (counter + histogram) tracked in the DashMaps. Bounds memory growth
    /// from unbounded label-value or name cardinality (rc-0pyv).
    max_dynamic_collectors: usize,
}

impl PrometheusMetrics {
    /// Creates a new PrometheusMetrics instance with all metrics registered.
    ///
    /// # Panics
    ///
    /// Creates a new PrometheusMetrics instance with all metrics registered.
    ///
    /// Static family creation panics only on static-invariant violations —
    /// see [`families::register_static_families`].
    pub fn new() -> Self {
        let registry = Registry::new();
        let families = families::register_static_families(&registry);

        Self {
            registry,
            families,
            dyn_counters: dashmap::DashMap::new(),
            dyn_histograms: dashmap::DashMap::new(),
            warned: dashmap::DashSet::new(),
            max_dynamic_collectors: 1024,
        }
    }

    /// Returns the soft cap on the number of unique dynamic collector names
    /// (counter + histogram) that this instance will accept. The cap is
    /// independent for counters and histograms.
    pub fn max_dynamic_collectors(&self) -> usize {
        self.max_dynamic_collectors
    }

    /// Builder-style override for the dynamic collector cap. Used to set a
    /// tighter bound in tests or to raise/lower the production default.
    pub fn with_max_dynamic_collectors(mut self, n: usize) -> Self {
        self.max_dynamic_collectors = n;
        self
    }

    /// Returns a reference to the underlying Prometheus registry
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Gathers all metrics and returns them in Prometheus text format
    pub fn gather(&self) -> String {
        families::render(&self.registry)
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
        self.families
            .exchange_duration_seconds
            .with_label_values(&[route_id])
            .observe(duration_secs);
    }

    fn increment_errors(&self, route_id: &str, error_type: &str) {
        self.families
            .errors_total
            .with_label_values(&[route_id, error_type])
            .inc();
    }

    fn increment_exchanges(&self, route_id: &str) {
        self.families
            .exchanges_total
            .with_label_values(&[route_id])
            .inc();
    }

    fn set_queue_depth(&self, queue: &str, depth: usize) {
        self.families
            .queue_depth
            .with_label_values(&[queue])
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
        self.families
            .circuit_breaker_state
            .with_label_values(&[route_id])
            .set(state_value(to));
    }

    fn increment_retry_attempt(&self, scheme: &str, operation: &str) {
        // Label keys are declared alphabetically (operation, scheme), so bind
        // values in that order — the call signature is (scheme, operation).
        self.families
            .retry_attempts_total
            .with_label_values(&[operation, scheme])
            .inc();
    }

    fn increment_circuit_breaker_rejection(&self, route: &str) {
        self.families
            .circuit_breaker_rejections_total
            .with_label_values(&[route])
            .inc();
    }

    fn set_route_state(&self, route: &str, state: &str) {
        // The families module keeps the per-route last-state map, so callers
        // stay one-arg: this sets the new state series to 1 and zeroes the
        // previous one.
        self.families.route_state.set(route, state);
    }

    fn clear_route_state(&self, route: &str) {
        self.families.route_state.remove(route);
    }

    fn record_build_info(&self, version: &str, git_sha: &str) {
        // Label keys are declared alphabetically (git_sha, version), so bind
        // positionally in that order — the call signature is (version, git_sha).
        self.families
            .build_info
            .with_label_values(&[git_sha, version])
            .set(1);
    }

    fn record_uptime(&self, seconds: f64) {
        self.families.uptime_seconds.set(seconds);
    }

    fn record_component_operation(&self, component: &str, operation: &str, outcome: &str) {
        // `outcome` is a closed set ("success" | "failure") enforced by
        // the `ComponentMetrics` facade; label keys are declared
        // alphabetically (component, operation, outcome) — the call
        // signature is already in that order.
        self.families
            .component_operations_total
            .with_label_values(&[component, operation, outcome])
            .inc();
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

        // Cap check runs BEFORE acquiring the entry guard: calling `len()`
        // while holding an `Entry` would deadlock the DashMap shard. The
        // `contains_key` short-circuit allows updates to already-tracked
        // names even when at cap (defense in depth, not exact enforcement
        // under contention).
        if self.dyn_counters.len() >= self.max_dynamic_collectors
            && !self.dyn_counters.contains_key(&normalized)
        {
            if self.warned.insert(format!("cap:{}", normalized)) {
                tracing::warn!(
                    name,
                    cap = self.max_dynamic_collectors,
                    "dynamic counter cap exceeded; observation dropped"
                );
            }
            return;
        }

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

        // Cap check runs BEFORE acquiring the entry guard: calling `len()`
        // while holding an `Entry` would deadlock the DashMap shard. The
        // `contains_key` short-circuit allows updates to already-tracked
        // names even when at cap.
        if self.dyn_histograms.len() >= self.max_dynamic_collectors
            && !self.dyn_histograms.contains_key(&normalized)
        {
            if self.warned.insert(format!("cap:{}", normalized)) {
                tracing::warn!(
                    name,
                    cap = self.max_dynamic_collectors,
                    "dynamic histogram cap exceeded; observation dropped"
                );
            }
            return;
        }

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
#[path = "tests.rs"]
mod tests;
