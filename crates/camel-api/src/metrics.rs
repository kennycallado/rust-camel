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

    /// Update queue depth
    fn set_queue_depth(&self, route_id: &str, depth: usize);

    /// Record circuit breaker state change
    fn record_circuit_breaker_change(&self, route_id: &str, from: &str, to: &str);

    /// Record a histogram observation (e.g., cost, latency distribution).
    /// Default: no-op (backward-compatible).
    fn record_histogram(&self, _name: &str, _value: f64, _labels: &[(&str, &str)]) {}

    /// Record a monotonically-increasing counter (e.g. `foo_total`).
    /// Default: no-op (backward-compatible).
    fn record_counter(&self, _name: &str, _value: f64, _labels: &[(&str, &str)]) {}
}

/// No-op metrics collector for default behavior
pub struct NoOpMetrics;

impl MetricsCollector for NoOpMetrics {
    fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}
    fn increment_errors(&self, _route_id: &str, _error_type: &str) {}
    fn increment_exchanges(&self, _route_id: &str) {}
    fn set_queue_depth(&self, _route_id: &str, _depth: usize) {}
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

    fn set_queue_depth(&self, route_id: &str, depth: usize) {
        self.inner.load().0.set_queue_depth(route_id, depth)
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
}

/// A [`MetricsCollector`] that fans every observation out to a list of collectors,
/// in registration order.
///
/// Built by [`MetricsHandle::register`] — the second registration stores a
/// composite of `[first, second]`; a third composes over that composite, so
/// ordering and prior observation are preserved (composition, not replacement).
pub struct CompositeMetricsCollector {
    collectors: Vec<Arc<dyn MetricsCollector>>,
}

impl CompositeMetricsCollector {
    /// Creates a composite that delegates to `collectors` in order.
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

    fn set_queue_depth(&self, route_id: &str, depth: usize) {
        for collector in &self.collectors {
            collector.set_queue_depth(route_id, depth);
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
    }

    impl RecordingMetrics {
        fn new() -> Self {
            Self {
                durations: Mutex::new(Vec::new()),
                errors: Mutex::new(Vec::new()),
                exchanges: Mutex::new(Vec::new()),
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

        fn set_queue_depth(&self, _route_id: &str, _depth: usize) {}

        fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}
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

            fn set_queue_depth(&self, route_id: &str, depth: usize) {
                // In a real implementation, this would update a gauge
                println!("Route {} queue depth: {}", route_id, depth);
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
}
