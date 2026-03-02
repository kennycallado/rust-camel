use crate::{MetricsCollector, NoOpMetrics};
use std::sync::Arc;
use std::time::Duration;

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
