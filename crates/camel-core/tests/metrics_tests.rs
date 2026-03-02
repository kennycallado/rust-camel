use camel_api::MetricsCollector;
use camel_core::CamelContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[test]
fn test_context_with_default_metrics() {
    let context = CamelContext::new();
    let metrics = context.metrics();

    // Should have NoOpMetrics by default
    metrics.record_exchange_duration("test", Duration::from_millis(100));
    metrics.increment_errors("test", "error");
    metrics.increment_exchanges("test");
    metrics.set_queue_depth("test", 5);
    metrics.record_circuit_breaker_change("test", "closed", "open");

    // If we get here without panicking, the default metrics work
}

#[test]
fn test_context_with_custom_metrics() {
    struct TestMetrics {
        exchange_count: AtomicU64,
    }

    impl TestMetrics {
        fn new() -> Self {
            Self {
                exchange_count: AtomicU64::new(0),
            }
        }

        fn get_exchange_count(&self) -> u64 {
            self.exchange_count.load(Ordering::Relaxed)
        }
    }

    impl MetricsCollector for TestMetrics {
        fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {
            // Implementation would record duration
        }

        fn increment_errors(&self, _route_id: &str, _error_type: &str) {
            // Implementation would increment error counter
        }

        fn increment_exchanges(&self, _route_id: &str) {
            self.exchange_count.fetch_add(1, Ordering::Relaxed);
        }

        fn set_queue_depth(&self, _route_id: &str, _depth: usize) {
            // Implementation would update gauge
        }

        fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {
            // Implementation would record state change
        }
    }

    let test_metrics = Arc::new(TestMetrics::new());
    let context =
        CamelContext::with_metrics(Arc::clone(&test_metrics) as Arc<dyn MetricsCollector>);

    let metrics = context.metrics();

    // Call the increment method
    metrics.increment_exchanges("test");

    // Verify that our custom metrics collector was called
    assert_eq!(test_metrics.get_exchange_count(), 1);
}
