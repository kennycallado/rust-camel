use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Test double that records observations for later inspection.
    struct RecordingMetrics {
        durations: Mutex<Vec<(String, Duration)>>,
        errors: Mutex<Vec<(String, String)>>,
        exchanges: Mutex<Vec<String>>,
        retries: Mutex<Vec<(String, String)>>,
        rejections: Mutex<Vec<String>>,
    }

    impl RecordingMetrics {
        fn new() -> Self {
            Self {
                durations: Mutex::new(Vec::new()),
                errors: Mutex::new(Vec::new()),
                exchanges: Mutex::new(Vec::new()),
                retries: Mutex::new(Vec::new()),
                rejections: Mutex::new(Vec::new()),
            }
        }
    }

    impl MetricsCollector for RecordingMetrics {
        fn record_exchange_duration(&self, route_id: &str, duration: Duration) {
            self.durations
                .lock()
                .expect("durations lock")
                .push((route_id.to_string(), duration));
        }

        fn increment_errors(&self, route_id: &str, error_type: &str) {
            self.errors
                .lock()
                .expect("errors lock")
                .push((route_id.to_string(), error_type.to_string()));
        }

        fn increment_exchanges(&self, route_id: &str) {
            self.exchanges
                .lock()
                .expect("exchanges lock")
                .push(route_id.to_string());
        }

        fn set_queue_depth(&self, _queue: &str, _depth: usize) {}

        fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}

        fn increment_retry_attempt(&self, scheme: &str, operation: &str) {
            self.retries
                .lock()
                .expect("retries lock")
                .push((scheme.to_string(), operation.to_string()));
        }

        fn increment_circuit_breaker_rejection(&self, route: &str) {
            self.rejections
                .lock()
                .expect("rejections lock")
                .push(route.to_string());
        }
    }

    /// Test double that tags every trait-method call by name, for
    /// delegation-parity assertions over the full `MetricsCollector` surface.
    struct SurfaceProbe {
        calls: Mutex<Vec<&'static str>>,
    }

    impl SurfaceProbe {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        fn tag(&self, name: &'static str) {
            self.calls.lock().expect("calls lock").push(name);
        }
    }

    impl MetricsCollector for SurfaceProbe {
        fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {
            self.tag("record_exchange_duration");
        }
        fn increment_errors(&self, _route_id: &str, _error_type: &str) {
            self.tag("increment_errors");
        }
        fn increment_exchanges(&self, _route_id: &str) {
            self.tag("increment_exchanges");
        }
        fn set_queue_depth(&self, _queue: &str, _depth: usize) {
            self.tag("set_queue_depth");
        }
        fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {
            self.tag("record_circuit_breaker_change");
        }
        fn record_histogram(&self, _name: &str, _value: f64, _labels: &[(&str, &str)]) {
            self.tag("record_histogram");
        }
        fn record_counter(&self, _name: &str, _value: f64, _labels: &[(&str, &str)]) {
            self.tag("record_counter");
        }
        fn increment_retry_attempt(&self, _scheme: &str, _operation: &str) {
            self.tag("increment_retry_attempt");
        }
        fn increment_circuit_breaker_rejection(&self, _route: &str) {
            self.tag("increment_circuit_breaker_rejection");
        }
        fn set_route_state(&self, _route: &str, _state: &str) {
            self.tag("set_route_state");
        }

        fn clear_route_state(&self, _route: &str) {
            self.tag("clear_route_state");
        }
        fn record_build_info(&self, _version: &str, _git_sha: &str) {
            self.tag("record_build_info");
        }
        fn record_uptime(&self, _seconds: f64) {
            self.tag("record_uptime");
        }
        fn record_component_operation(&self, _component: &str, _operation: &str, _outcome: &str) {
            self.tag("record_component_operation");
        }
    }

    #[test]
    fn test_noop_metrics_implements_trait() {
        let metrics = NoOpMetrics;
        let metrics_arc: Arc<dyn MetricsCollector> = Arc::new(metrics);

        // All methods should execute without panicking
        metrics_arc.record_exchange_duration("test-route", Duration::from_millis(100));
        metrics_arc.increment_errors("test-route", "test-error");
        metrics_arc.increment_exchanges("test-route");
        metrics_arc.set_queue_depth("test-route", 5);
        metrics_arc.record_circuit_breaker_change("test-route", "closed", "open");
    }

    #[test]
    fn test_custom_metrics_collector() {
        struct TestMetrics {
            exchange_count: std::sync::atomic::AtomicU64,
        }

        impl MetricsCollector for TestMetrics {
            fn record_exchange_duration(&self, route_id: &str, duration: Duration) {
                // In a real implementation, this would record the duration
                println!("Route {} took {}ms", route_id, duration.as_millis());
            }

            fn increment_errors(&self, route_id: &str, error_type: &str) {
                // In a real implementation, this would increment an error counter
                println!("Route {} had error: {}", route_id, error_type);
            }

            fn increment_exchanges(&self, route_id: &str) {
                // In a real implementation, this would increment an exchange counter
                self.exchange_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                println!("Route {} processed exchange", route_id);
            }

            fn set_queue_depth(&self, queue: &str, depth: usize) {
                // In a real implementation, this would update a gauge
                println!("Queue {queue} depth: {depth}");
            }

            fn record_circuit_breaker_change(&self, route_id: &str, from: &str, to: &str) {
                // In a real implementation, this would record the state change
                println!("Route {} circuit breaker: {} -> {}", route_id, from, to);
            }
        }

        let test_metrics = TestMetrics {
            exchange_count: std::sync::atomic::AtomicU64::new(0),
        };
        let metrics_arc: Arc<dyn MetricsCollector> = Arc::new(test_metrics);

        // Test that all methods work
        metrics_arc.record_exchange_duration("test-route", Duration::from_millis(100));
        metrics_arc.increment_errors("test-route", "test-error");
        metrics_arc.increment_exchanges("test-route");
        metrics_arc.set_queue_depth("test-route", 5);
        metrics_arc.record_circuit_breaker_change("test-route", "closed", "open");

        // Note: We can't easily test the counter value without additional accessors
        // This is just to verify the trait implementation works
    }

    #[test]
    fn handle_delegates_to_stored_collector() {
        let collector = Arc::new(RecordingMetrics::new());
        let handle = MetricsHandle::new();
        handle.register(collector.clone());

        handle.record_exchange_duration("r", Duration::from_millis(1));

        let recorded = collector.durations.lock().expect("durations lock").clone();
        assert_eq!(recorded, vec![("r".to_string(), Duration::from_millis(1))]);
    }

    #[test]
    fn second_registration_composes_both_observe() {
        let a = Arc::new(RecordingMetrics::new());
        let b = Arc::new(RecordingMetrics::new());
        let handle = MetricsHandle::new();
        handle.register(a.clone());
        handle.register(b.clone());

        handle.increment_errors("r", "x");

        let a_errors = a.errors.lock().expect("errors lock").clone();
        let b_errors = b.errors.lock().expect("errors lock").clone();
        assert_eq!(a_errors, vec![("r".to_string(), "x".to_string())]);
        assert_eq!(b_errors, vec![("r".to_string(), "x".to_string())]);
    }

    #[test]
    fn register_same_arc_is_idempotent() {
        let a = Arc::new(RecordingMetrics::new());
        let handle = MetricsHandle::new();
        handle.register(a.clone());
        handle.register(a.clone());

        handle.increment_exchanges("r");

        let recorded = a.exchanges.lock().expect("exchanges lock").clone();
        assert_eq!(recorded, vec!["r".to_string()]);
    }

    #[test]
    fn handle_defaults_to_noop() {
        let handle = MetricsHandle::new();
        handle.record_exchange_duration("r", Duration::from_millis(1));
        handle.increment_errors("r", "x");
        handle.increment_exchanges("r");
        handle.set_queue_depth("r", 5);
        handle.record_circuit_breaker_change("r", "closed", "open");
        handle.record_histogram("h", 1.0, &[("k", "v")]);
        handle.record_counter("c", 1.0, &[("k", "v")]);
    }

    #[test]
    fn composite_delegates_retry_and_rejection() {
        let a = Arc::new(RecordingMetrics::new());
        let b = Arc::new(RecordingMetrics::new());
        let composite = CompositeMetricsCollector::new(vec![
            Arc::clone(&a) as Arc<dyn MetricsCollector>,
            Arc::clone(&b) as Arc<dyn MetricsCollector>,
        ]);

        composite.increment_retry_attempt("kafka", "connect");
        composite.increment_circuit_breaker_rejection("r1");

        for member in [&a, &b] {
            assert_eq!(
                member.retries.lock().expect("retries lock").clone(),
                vec![("kafka".to_string(), "connect".to_string())]
            );
            assert_eq!(
                member.rejections.lock().expect("rejections lock").clone(),
                vec!["r1".to_string()]
            );
        }
    }

    #[test]
    fn noop_defaults_compile_and_do_nothing() {
        let collector: Arc<dyn MetricsCollector> = Arc::new(NoOpMetrics);
        // Both new methods must exist as no-op defaults: compile + no panic.
        collector.increment_retry_attempt("kafka", "connect");
        collector.increment_circuit_breaker_rejection("r1");
    }

    /// Delegation parity: the composite fans the full `MetricsCollector`
    /// surface out to every member.
    #[test]
    fn composite_delegates_full_trait_surface() {
        let a = Arc::new(SurfaceProbe::new());
        let b = Arc::new(SurfaceProbe::new());
        let composite = CompositeMetricsCollector::new(vec![
            Arc::clone(&a) as Arc<dyn MetricsCollector>,
            Arc::clone(&b) as Arc<dyn MetricsCollector>,
        ]);

        composite.record_exchange_duration("r", Duration::from_millis(1));
        composite.increment_errors("r", "x");
        composite.increment_exchanges("r");
        composite.set_queue_depth("r", 1);
        composite.record_circuit_breaker_change("r", "closed", "open");
        composite.record_histogram("h", 1.0, &[("k", "v")]);
        composite.record_counter("c", 1.0, &[("k", "v")]);
        composite.increment_retry_attempt("kafka", "connect");
        composite.increment_circuit_breaker_rejection("r1");
        composite.set_route_state("r", "Started");
        composite.clear_route_state("r");
        composite.record_build_info("1.2.3", "abc1234");
        composite.record_uptime(0.5);
        composite.record_component_operation("redis", "command", "success");

        let expected = vec![
            "record_exchange_duration",
            "increment_errors",
            "increment_exchanges",
            "set_queue_depth",
            "record_circuit_breaker_change",
            "record_histogram",
            "record_counter",
            "increment_retry_attempt",
            "increment_circuit_breaker_rejection",
            "set_route_state",
            "clear_route_state",
            "record_build_info",
            "record_uptime",
            "record_component_operation",
        ];
        for member in [&a, &b] {
            let calls = member.calls.lock().expect("calls lock").clone();
            assert_eq!(calls, expected, "member missed part of the trait surface");
        }
    }
}
