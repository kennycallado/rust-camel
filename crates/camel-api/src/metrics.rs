use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;

/// The closed set of allocator memory statistics published through
/// [`MetricsCollector::set_allocator_memory`].
///
/// # exhaustive-by-contract
///
/// exhaustive-by-contract: a closed 4-variant allocator stat set whose
/// label values (`allocated | resident | active | mapped`) are fixed by the
/// metrics spec; out-of-crate emitters (the camel-cli jemalloc sampler) match
/// every variant, so adding one is a contract change, not a compatible
/// extension.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AllocatorStat {
    /// Total bytes allocated by the allocator (in-use).
    Allocated,
    /// Resident bytes backed by physical pages (RSS contribution).
    Resident,
    /// Bytes in active pages.
    Active,
    /// Bytes in mapped virtual ranges.
    Mapped,
}

impl AllocatorStat {
    /// The Prometheus `stat` label value for this statistic.
    pub fn as_str(&self) -> &'static str {
        match self {
            AllocatorStat::Allocated => "allocated",
            AllocatorStat::Resident => "resident",
            AllocatorStat::Active => "active",
            AllocatorStat::Mapped => "mapped",
        }
    }
}

/// Trait for collecting metrics from the Camel runtime.
/// Implementations can integrate with Prometheus, OpenTelemetry, etc.
pub trait MetricsCollector: Send + Sync {
    /// Record exchange processing time
    fn record_exchange_duration(&self, route_id: &str, duration: Duration);

    /// Increment error counter
    fn increment_errors(&self, route_id: &str, error_type: &str);

    /// Increment exchange counter
    fn increment_exchanges(&self, route_id: &str);

    /// Update the depth of a buffered stage's queue
    /// (`camel_queue_depth{queue}`). The `queue` label is a closed set of
    /// component-declared identifiers (`seda:<endpoint-name>`,
    /// `aggregator:<route>`, `resequencer:<route>`).
    fn set_queue_depth(&self, queue: &str, depth: usize);

    /// Record circuit breaker state change
    fn record_circuit_breaker_change(&self, route_id: &str, from: &str, to: &str);

    /// Record a histogram observation (e.g., cost, latency distribution).
    /// Default: no-op (backward-compatible).
    fn record_histogram(&self, _name: &str, _value: f64, _labels: &[(&str, &str)]) {}

    /// Record a monotonically-increasing counter (e.g. `foo_total`).
    /// Default: no-op (backward-compatible).
    fn record_counter(&self, _name: &str, _value: f64, _labels: &[(&str, &str)]) {}

    /// Increment the per-attempt retry counter (`camel_retry_attempts_total`,
    /// labels scheme+operation). Called once per retry attempt, including the
    /// first. Default: no-op (backward-compatible).
    fn increment_retry_attempt(&self, _scheme: &str, _operation: &str) {}

    /// Increment the circuit-breaker rejection counter
    /// (`camel_circuit_breaker_rejections_total`, label route). Open-breaker
    /// fast-fails count here, not as errors. Default: no-op
    /// (backward-compatible).
    fn increment_circuit_breaker_rejection(&self, _route: &str) {}

    /// Publish a route lifecycle-state transition (`camel_route_state`,
    /// labels route+state). `state` is the projection's state label — a
    /// closed set by construction (`Registered`, `Starting`, `Started`,
    /// `Suspended`, `Stopping`, `Stopped`, `Failed`). Implementations keep
    /// the route's last-published state so a transition sets the new series
    /// to 1 and zeroes the previous one. Default: no-op
    /// (backward-compatible).
    fn set_route_state(&self, _route: &str, _state: &str) {}

    /// Drop a route's state series (route removed/undeployed) so a
    /// scrape reflects only routes that exist.
    fn clear_route_state(&self, _route: &str) {}

    /// Publish build identification (`camel_build_info{git_sha,version}`,
    /// value 1). Called once when the context is built. Default: no-op
    /// (backward-compatible).
    fn record_build_info(&self, _version: &str, _git_sha: &str) {}

    /// Publish process uptime in seconds (`camel_uptime_seconds`),
    /// refreshed periodically by the runtime. Default: no-op
    /// (backward-compatible).
    fn record_uptime(&self, _seconds: f64) {}

    /// Increment the uniform component-operations counter
    /// (`camel_component_operations_total`, labels component+operation+
    /// outcome). `outcome` is a closed set — "success" or "failure"
    /// only; callers derive it from a bool (see `ComponentMetrics`),
    /// never pass free text. Default: no-op (backward-compatible).
    fn record_component_operation(&self, _component: &str, _operation: &str, _outcome: &str) {}

    /// Publish the pinned client cache size for a component
    /// (`camel_pinned_client_cache_size{component}`, gauge, unit: entries).
    /// Emitted by the owning component after each lookup, reflecting the
    /// current (approximate) entry count. Default:
    /// no-op (backward-compatible).
    fn set_pinned_client_cache_size(&self, _component: &str, _entries: u64) {}

    /// Increment the pinned client cache hit counter for a component
    /// (`camel_pinned_client_cache_hits_total{component}`) — a pinned
    /// lookup served by the cache without a rebuild. Default: no-op
    /// (backward-compatible).
    fn increment_pinned_client_cache_hit(&self, _component: &str) {}

    /// Increment the pinned client cache miss counter for a component
    /// (`camel_pinned_client_cache_misses_total{component}`) — a pinned
    /// lookup that required a client rebuild. Default: no-op
    /// (backward-compatible).
    fn increment_pinned_client_cache_miss(&self, _component: &str) {}

    /// Publish an allocator memory statistic
    /// (`camel_allocator_memory_bytes{stat}`, gauge, unit: bytes). `stat`
    /// is a closed [`AllocatorStat`] variant; the sampler refreshes the
    /// current value periodically. Default: no-op (backward-compatible).
    fn set_allocator_memory(&self, _stat: AllocatorStat, _bytes: u64) {}

    /// Publish the leadership state for a master lock
    /// (`camel_master_is_leader{lock}`, gauge): 1 while leadership is
    /// held, 0 after it is lost. Emitted on the same observed state edges
    /// as the `master_leadership_transitions_total` counter; the gauge
    /// exists for steady-state readability ("who leads lock X now"), not
    /// transition counting. Default: no-op (backward-compatible).
    fn set_master_leadership(&self, _lock: &str, _leader: bool) {}
}

/// No-op metrics collector for default behavior
pub struct NoOpMetrics;

impl MetricsCollector for NoOpMetrics {
    fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}
    fn increment_errors(&self, _route_id: &str, _error_type: &str) {}
    fn increment_exchanges(&self, _route_id: &str) {}
    fn set_queue_depth(&self, _queue: &str, _depth: usize) {}
    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}
}

/// Sized slot around `Arc<dyn MetricsCollector>`.
///
/// `ArcSwap`'s `RefCnt` implementation requires a `Sized` target, so a bare
/// `ArcSwap<dyn MetricsCollector>` does not compile; this newtype restores
/// `Sized`-ness without changing the stored pointee.
struct CollectorSlot(Arc<dyn MetricsCollector>);

/// A late-bound [`MetricsCollector`] cell.
///
/// Contract:
///
/// - **Late binding:** a `MetricsHandle` can be handed to consumers before any real
///   collector exists; it seeds itself with [`NoOpMetrics`] so calls before (and
///   without) registration are safe no-ops.
/// - **Composition, not replacement:** each [`MetricsHandle::register`] composes the
///   new collector *over* the currently stored one (see [`CompositeMetricsCollector`]);
///   previously registered collectors keep observing.
/// - **Same-Arc idempotence:** registering the same collector `Arc` twice is a no-op
///   (detected via `Arc::ptr_eq` against the membership list), so a call site that
///   wires the same collector through two builder paths does not double-count.
/// - **Delegation cost:** each trait-method call costs one atomic load of the stored
///   `Arc` (`ArcSwap::load`); the hot path never clones the `Arc`.
pub struct MetricsHandle {
    inner: ArcSwap<CollectorSlot>,
    /// Membership list of every accepted collector, parallel to `inner`.
    /// Kept because the stored `dyn` composite cannot be introspected for
    /// `Arc::ptr_eq` dedupe.
    members: Mutex<Vec<Arc<dyn MetricsCollector>>>,
}

impl MetricsHandle {
    /// Creates a handle that delegates to [`NoOpMetrics`] until a collector is
    /// registered.
    pub fn new() -> Self {
        Self {
            inner: ArcSwap::from_pointee(CollectorSlot(Arc::new(NoOpMetrics))),
            members: Mutex::new(Vec::new()),
        }
    }

    /// Registers `collector`, composing it over whatever is currently stored.
    ///
    /// If the exact same `Arc` was already registered, this is a no-op
    /// (see *same-Arc idempotence* in the type-level docs).
    pub fn register(&self, collector: Arc<dyn MetricsCollector>) {
        let mut members = self
            .members
            .lock()
            .expect("metrics members lock poisoned by a panicked register"); // allow-unwrap
        if members.iter().any(|m| Arc::ptr_eq(m, &collector)) {
            return;
        }
        let first = members.is_empty();
        members.push(Arc::clone(&collector));
        if first {
            // Store directly — composing over the seeded NoOp would leave a
            // permanent dead leg in every later composite chain.
            self.inner.store(Arc::new(CollectorSlot(collector)));
            return;
        }
        let prev = Arc::clone(&self.inner.load().0);
        self.inner.store(Arc::new(CollectorSlot(Arc::new(
            CompositeMetricsCollector::new(vec![prev, collector]),
        ))));
    }
}

impl Default for MetricsHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsCollector for MetricsHandle {
    fn record_exchange_duration(&self, route_id: &str, duration: Duration) {
        self.inner
            .load()
            .0
            .record_exchange_duration(route_id, duration)
    }

    fn increment_errors(&self, route_id: &str, error_type: &str) {
        self.inner.load().0.increment_errors(route_id, error_type)
    }

    fn increment_exchanges(&self, route_id: &str) {
        self.inner.load().0.increment_exchanges(route_id)
    }

    fn set_queue_depth(&self, queue: &str, depth: usize) {
        self.inner.load().0.set_queue_depth(queue, depth)
    }

    fn record_circuit_breaker_change(&self, route_id: &str, from: &str, to: &str) {
        self.inner
            .load()
            .0
            .record_circuit_breaker_change(route_id, from, to)
    }

    fn record_histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        self.inner.load().0.record_histogram(name, value, labels)
    }

    fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        self.inner.load().0.record_counter(name, value, labels)
    }

    fn increment_retry_attempt(&self, scheme: &str, operation: &str) {
        self.inner
            .load()
            .0
            .increment_retry_attempt(scheme, operation)
    }

    fn increment_circuit_breaker_rejection(&self, route: &str) {
        self.inner
            .load()
            .0
            .increment_circuit_breaker_rejection(route)
    }

    fn set_route_state(&self, route: &str, state: &str) {
        self.inner.load().0.set_route_state(route, state)
    }

    fn clear_route_state(&self, route: &str) {
        self.inner.load().0.clear_route_state(route)
    }

    fn record_build_info(&self, version: &str, git_sha: &str) {
        self.inner.load().0.record_build_info(version, git_sha)
    }

    fn record_uptime(&self, seconds: f64) {
        self.inner.load().0.record_uptime(seconds)
    }

    fn record_component_operation(&self, component: &str, operation: &str, outcome: &str) {
        self.inner
            .load()
            .0
            .record_component_operation(component, operation, outcome)
    }

    fn set_pinned_client_cache_size(&self, component: &str, entries: u64) {
        self.inner
            .load()
            .0
            .set_pinned_client_cache_size(component, entries)
    }

    fn increment_pinned_client_cache_hit(&self, component: &str) {
        self.inner
            .load()
            .0
            .increment_pinned_client_cache_hit(component)
    }

    fn increment_pinned_client_cache_miss(&self, component: &str) {
        self.inner
            .load()
            .0
            .increment_pinned_client_cache_miss(component)
    }

    fn set_allocator_memory(&self, stat: AllocatorStat, bytes: u64) {
        self.inner.load().0.set_allocator_memory(stat, bytes)
    }

    fn set_master_leadership(&self, lock: &str, leader: bool) {
        self.inner.load().0.set_master_leadership(lock, leader)
    }
}

/// A [`MetricsCollector`] that fans every observation out to a list of collectors,
/// in registration order.
///
/// Built by [`MetricsHandle::register`] — the second registration stores a
/// composite of `[first, second]`; a third composes over that composite, so
/// ordering and prior observation are preserved (composition, not replacement).
///
/// Internal type, hidden from the published docs. Out-of-tree code must not
/// construct composites directly: registering an externally built composite
/// plus its inner collector double-counts (the handle's opaque-Arc dedupe
/// cannot see inside a composite). Register collectors via
/// [`MetricsHandle::register`] instead.
#[doc(hidden)]
pub struct CompositeMetricsCollector {
    collectors: Vec<Arc<dyn MetricsCollector>>,
}

impl CompositeMetricsCollector {
    /// Creates a composite that delegates to `collectors` in order.
    ///
    /// Internal constructor, hidden from the published docs. Prefer
    /// [`MetricsHandle::register`], which composes while deduplicating by
    /// `Arc` pointer identity; direct construction bypasses that dedupe and
    /// can double-count.
    #[doc(hidden)]
    pub fn new(collectors: Vec<Arc<dyn MetricsCollector>>) -> Self {
        Self { collectors }
    }
}

impl MetricsCollector for CompositeMetricsCollector {
    fn record_exchange_duration(&self, route_id: &str, duration: Duration) {
        for collector in &self.collectors {
            collector.record_exchange_duration(route_id, duration);
        }
    }

    fn increment_errors(&self, route_id: &str, error_type: &str) {
        for collector in &self.collectors {
            collector.increment_errors(route_id, error_type);
        }
    }

    fn increment_exchanges(&self, route_id: &str) {
        for collector in &self.collectors {
            collector.increment_exchanges(route_id);
        }
    }

    fn set_queue_depth(&self, queue: &str, depth: usize) {
        for collector in &self.collectors {
            collector.set_queue_depth(queue, depth);
        }
    }

    fn record_circuit_breaker_change(&self, route_id: &str, from: &str, to: &str) {
        for collector in &self.collectors {
            collector.record_circuit_breaker_change(route_id, from, to);
        }
    }

    fn record_histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        for collector in &self.collectors {
            collector.record_histogram(name, value, labels);
        }
    }

    fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        for collector in &self.collectors {
            collector.record_counter(name, value, labels);
        }
    }

    fn increment_retry_attempt(&self, scheme: &str, operation: &str) {
        for collector in &self.collectors {
            collector.increment_retry_attempt(scheme, operation);
        }
    }

    fn increment_circuit_breaker_rejection(&self, route: &str) {
        for collector in &self.collectors {
            collector.increment_circuit_breaker_rejection(route);
        }
    }

    fn set_route_state(&self, route: &str, state: &str) {
        for collector in &self.collectors {
            collector.set_route_state(route, state);
        }
    }

    fn clear_route_state(&self, route: &str) {
        for collector in &self.collectors {
            collector.clear_route_state(route);
        }
    }

    fn record_build_info(&self, version: &str, git_sha: &str) {
        for collector in &self.collectors {
            collector.record_build_info(version, git_sha);
        }
    }

    fn record_uptime(&self, seconds: f64) {
        for collector in &self.collectors {
            collector.record_uptime(seconds);
        }
    }

    fn record_component_operation(&self, component: &str, operation: &str, outcome: &str) {
        for collector in &self.collectors {
            collector.record_component_operation(component, operation, outcome);
        }
    }

    fn set_pinned_client_cache_size(&self, component: &str, entries: u64) {
        for collector in &self.collectors {
            collector.set_pinned_client_cache_size(component, entries);
        }
    }

    fn increment_pinned_client_cache_hit(&self, component: &str) {
        for collector in &self.collectors {
            collector.increment_pinned_client_cache_hit(component);
        }
    }

    fn increment_pinned_client_cache_miss(&self, component: &str) {
        for collector in &self.collectors {
            collector.increment_pinned_client_cache_miss(component);
        }
    }

    fn set_allocator_memory(&self, stat: AllocatorStat, bytes: u64) {
        for collector in &self.collectors {
            collector.set_allocator_memory(stat, bytes);
        }
    }

    fn set_master_leadership(&self, lock: &str, leader: bool) {
        for collector in &self.collectors {
            collector.set_master_leadership(lock, leader);
        }
    }
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
