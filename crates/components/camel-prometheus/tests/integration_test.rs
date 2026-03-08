use std::sync::Arc;
use std::time::Duration;

use camel_api::metrics::MetricsCollector;
use camel_prometheus::PrometheusMetrics;

#[tokio::test]
async fn test_full_integration() {
    let metrics = Arc::new(PrometheusMetrics::new());
    
    metrics.increment_exchanges("route-1");
    metrics.increment_exchanges("route-1");
    metrics.increment_exchanges("route-2");
    
    metrics.increment_errors("route-1", "timeout");
    metrics.increment_errors("route-1", "connection_refused");
    
    metrics.record_exchange_duration("route-1", Duration::from_millis(100));
    metrics.record_exchange_duration("route-1", Duration::from_millis(200));
    
    metrics.set_queue_depth("route-1", 5);
    
    metrics.record_circuit_breaker_change("route-1", "closed", "open");
    metrics.record_circuit_breaker_change("route-2", "open", "half_open");
    
    let output = metrics.gather();
    
    assert!(output.contains("camel_exchanges_total{route=\"route-1\"} 2"));
    assert!(output.contains("camel_exchanges_total{route=\"route-2\"} 1"));
    assert!(output.contains("camel_errors_total{error_type=\"timeout\",route=\"route-1\"} 1"));
    assert!(output.contains("camel_errors_total{error_type=\"connection_refused\",route=\"route-1\"} 1"));
    assert!(output.contains("camel_exchange_duration_seconds_count{route=\"route-1\"} 2"));
    assert!(output.contains("camel_queue_depth{route=\"route-1\"} 5"));
    assert!(output.contains("camel_circuit_breaker_state{route=\"route-1\"} 1"));
    assert!(output.contains("camel_circuit_breaker_state{route=\"route-2\"} 2"));
}

#[tokio::test]
async fn test_concurrent_access() {
    let metrics = Arc::new(PrometheusMetrics::new());
    let mut handles = vec![];
    
    for i in 0..10 {
        let metrics = Arc::clone(&metrics);
        let handle = tokio::spawn(async move {
            for _ in 0..100 {
                metrics.increment_exchanges(&format!("route-{}", i % 3));
                metrics.record_exchange_duration(&format!("route-{}", i % 3), Duration::from_millis(10));
            }
        });
        handles.push(handle);
    }
    
    for handle in handles {
        handle.await.unwrap();
    }
    
    let output = metrics.gather();
    assert!(output.contains("camel_exchanges_total"));
}