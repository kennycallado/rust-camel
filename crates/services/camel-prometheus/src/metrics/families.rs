//! Static (pre-declared) Prometheus metric families.
//!
//! Extracted verbatim from `PrometheusMetrics::new` (inter-phase review
//! finding: metrics.rs at 971 lines) — family registration and text render
//! live here; dynamic collectors and the `MetricsCollector` impl stay in
//! the parent module. Pure move: no behavior change.

use std::collections::HashMap;
use std::sync::Mutex;

use prometheus::{
    CounterVec, Gauge, GaugeVec, HistogramVec, IntCounterVec, IntGaugeVec, Opts, Registry,
    TextEncoder,
};

/// `camel_route_state{route,state}` — route lifecycle-state gauge.
///
/// The collector keeps each route's last-published state so every
/// [`RouteStateGauge::set`] call moves the one-arg transition
/// (`MetricsCollector::set_route_state(route, state)`) onto the gauge:
/// the new `(route, state)` series is set to 1 and the previous state
/// series to 0. State values are the projection's closed set
/// (`Registered`, `Starting`, `Started`, `Suspended`, `Stopping`,
/// `Stopped`, `Failed`).
pub(super) struct RouteStateGauge {
    gauge: IntGaugeVec,
    last: Mutex<HashMap<String, String>>,
}

impl RouteStateGauge {
    fn new(registry: &Registry) -> Self {
        let gauge = IntGaugeVec::new(
            Opts::new(
                "route_state",
                "Route lifecycle state (1 = the route's current state)",
            )
            .namespace("camel"),
            &["route", "state"],
        )
        .expect("Failed to create route_state gauge"); // allow-unwrap
        registry
            .register(Box::new(gauge.clone()))
            .expect("Failed to register route_state gauge"); // allow-unwrap
        Self {
            gauge,
            last: Mutex::new(HashMap::new()),
        }
    }

    /// Remove `route`'s series entirely (route undeployed): zero the
    /// last-published state, drop the gauge child and the map entry.
    pub(super) fn remove(&self, route: &str) {
        let mut last = self
            .last
            .lock()
            .expect("route_state last-state lock poisoned"); // allow-unwrap
        if let Some(prev) = last.remove(route) {
            self.gauge.with_label_values(&[route, &prev]).set(0);
            let _ = self.gauge.remove_label_values(&[route, &prev]);
        }
    }

    /// Publish a state transition for `route`.
    pub(super) fn set(&self, route: &str, state: &str) {
        self.gauge.with_label_values(&[route, state]).set(1);
        let mut last = self
            .last
            .lock()
            .expect("route_state last-state lock poisoned"); // allow-unwrap
        if let Some(prev) = last.insert(route.to_string(), state.to_string())
            && prev != state
        {
            self.gauge.with_label_values(&[route, &prev]).set(0);
        }
    }
}

/// The pre-declared metric families owned by [`super::PrometheusMetrics`].
pub(super) struct StaticFamilies {
    pub(super) exchanges_total: CounterVec,
    pub(super) errors_total: CounterVec,
    pub(super) exchange_duration_seconds: HistogramVec,
    pub(super) queue_depth: GaugeVec,
    pub(super) pinned_client_cache_size: GaugeVec,
    pub(super) pinned_client_cache_hits_total: CounterVec,
    pub(super) pinned_client_cache_misses_total: CounterVec,
    pub(super) allocator_memory_bytes: GaugeVec,
    pub(super) circuit_breaker_state: GaugeVec,
    pub(super) route_state: RouteStateGauge,
    pub(super) retry_attempts_total: IntCounterVec,
    pub(super) circuit_breaker_rejections_total: IntCounterVec,
    pub(super) component_operations_total: IntCounterVec,
    pub(super) build_info: IntGaugeVec,
    pub(super) uptime_seconds: Gauge,
}

/// Create and register every static family on `registry`.
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
pub(super) fn register_static_families(registry: &Registry) -> StaticFamilies {
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

    // Create and register queue_depth gauge. The label is `queue` (spec:
    // `camel_queue_depth{queue}`) — buffered-stage identifiers such as
    // `seda:<endpoint-name>`, `aggregator:<route>`, `resequencer:<route>`.
    let queue_depth = GaugeVec::new(
        Opts::new("queue_depth", "Current queue depth").namespace("camel"),
        &["queue"],
    )
    .expect("Failed to create queue_depth gauge"); // allow-unwrap
    registry
        .register(Box::new(queue_depth.clone()))
        .expect("Failed to register queue_depth gauge"); // allow-unwrap

    // Create and register pinned_client_cache_size gauge. The label is
    // `component` (spec: `camel_pinned_client_cache_size{component}`) — a
    // closed two-value set (`camel-http`, `camel-https`).
    let pinned_client_cache_size = GaugeVec::new(
        Opts::new(
            "pinned_client_cache_size",
            "Pinned client cache size in entries",
        )
        .namespace("camel"),
        &["component"],
    )
    .expect("Failed to create pinned_client_cache_size gauge"); // allow-unwrap
    registry
        .register(Box::new(pinned_client_cache_size.clone()))
        .expect("Failed to register pinned_client_cache_size gauge"); // allow-unwrap

    // Create and register pinned_client_cache_hits_total counter
    let pinned_client_cache_hits_total = CounterVec::new(
        Opts::new(
            "pinned_client_cache_hits_total",
            "Total pinned client cache hits",
        )
        .namespace("camel"),
        &["component"],
    )
    .expect("Failed to create pinned_client_cache_hits_total counter"); // allow-unwrap
    registry
        .register(Box::new(pinned_client_cache_hits_total.clone()))
        .expect("Failed to register pinned_client_cache_hits_total counter"); // allow-unwrap

    // Create and register pinned_client_cache_misses_total counter
    let pinned_client_cache_misses_total = CounterVec::new(
        Opts::new(
            "pinned_client_cache_misses_total",
            "Total pinned client cache misses (client builds)",
        )
        .namespace("camel"),
        &["component"],
    )
    .expect("Failed to create pinned_client_cache_misses_total counter"); // allow-unwrap
    registry
        .register(Box::new(pinned_client_cache_misses_total.clone()))
        .expect("Failed to register pinned_client_cache_misses_total counter"); // allow-unwrap

    // Create and register allocator_memory_bytes gauge. The label is
    // `stat` (spec: `camel_allocator_memory_bytes{stat}`) — a closed
    // four-value set (`allocated | resident | active | mapped`, see
    // `AllocatorStat` in camel-api).
    let allocator_memory_bytes = GaugeVec::new(
        Opts::new(
            "allocator_memory_bytes",
            "Allocator memory statistics in bytes",
        )
        .namespace("camel"),
        &["stat"],
    )
    .expect("Failed to create allocator_memory_bytes gauge"); // allow-unwrap
    registry
        .register(Box::new(allocator_memory_bytes.clone()))
        .expect("Failed to register allocator_memory_bytes gauge"); // allow-unwrap

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

    // Create and register route_state gauge (transition-aware; the
    // last-state map zeroes the previous series on each move)
    let route_state = RouteStateGauge::new(registry);

    // Create and register retry_attempts_total counter
    let retry_attempts_total = IntCounterVec::new(
        Opts::new(
            "retry_attempts_total",
            "Total number of retry attempts (including the first attempt)",
        )
        .namespace("camel"),
        &["operation", "scheme"],
    )
    .expect("Failed to create retry_attempts_total counter"); // allow-unwrap
    registry
        .register(Box::new(retry_attempts_total.clone()))
        .expect("Failed to register retry_attempts_total counter"); // allow-unwrap

    // Create and register circuit_breaker_rejections_total counter
    let circuit_breaker_rejections_total = IntCounterVec::new(
        Opts::new(
            "circuit_breaker_rejections_total",
            "Total number of exchanges rejected fast by an open circuit breaker",
        )
        .namespace("camel"),
        &["route"],
    )
    .expect("Failed to create circuit_breaker_rejections_total counter"); // allow-unwrap
    registry
        .register(Box::new(circuit_breaker_rejections_total.clone()))
        .expect("Failed to register circuit_breaker_rejections_total counter"); // allow-unwrap

    // Create and register component_operations_total counter
    // (dashboard-observability D5). `outcome` is a closed set —
    // "success" or "failure" only; the `ComponentMetrics` facade derives
    // it from a bool, so no other value reaches this family. Label keys
    // are declared alphabetically (component, operation, outcome), which
    // matches the `MetricsCollector::record_component_operation`
    // signature order.
    let component_operations_total = IntCounterVec::new(
        Opts::new(
            "component_operations_total",
            "Total component operations by outcome (success | failure)",
        )
        .namespace("camel"),
        &["component", "operation", "outcome"],
    )
    .expect("Failed to create component_operations_total counter"); // allow-unwrap
    registry
        .register(Box::new(component_operations_total.clone()))
        .expect("Failed to register component_operations_total counter"); // allow-unwrap

    // Create and register build_info gauge. Label keys are declared
    // alphabetically (git_sha, version) — the text render follows, so
    // positional `with_label_values` binds (git_sha, version).
    let build_info = IntGaugeVec::new(
        Opts::new(
            "build_info",
            "Build metadata (git_sha, version); value is always 1",
        )
        .namespace("camel"),
        &["git_sha", "version"],
    )
    .expect("Failed to create build_info gauge"); // allow-unwrap
    registry
        .register(Box::new(build_info.clone()))
        .expect("Failed to register build_info gauge"); // allow-unwrap

    // Create and register uptime_seconds gauge (refreshed by the runtime;
    // the endpoint is pull-based, nothing else touches the collector).
    let uptime_seconds = Gauge::with_opts(
        Opts::new("uptime_seconds", "Process uptime in seconds").namespace("camel"),
    )
    .expect("Failed to create uptime_seconds gauge"); // allow-unwrap
    registry
        .register(Box::new(uptime_seconds.clone()))
        .expect("Failed to register uptime_seconds gauge"); // allow-unwrap

    StaticFamilies {
        exchanges_total,
        errors_total,
        exchange_duration_seconds,
        queue_depth,
        pinned_client_cache_size,
        pinned_client_cache_hits_total,
        pinned_client_cache_misses_total,
        allocator_memory_bytes,
        circuit_breaker_state,
        route_state,
        retry_attempts_total,
        circuit_breaker_rejections_total,
        component_operations_total,
        build_info,
        uptime_seconds,
    }
}

/// Gather all metrics from `registry` in Prometheus text format.
pub(super) fn render(registry: &Registry) -> String {
    let encoder = TextEncoder::new();
    let metric_families = registry.gather();
    encoder
        .encode_to_string(&metric_families)
        .unwrap_or_else(|e| format!("# Error encoding metrics: {}\n", e))
}
