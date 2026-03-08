use std::time::Duration;
use camel_prometheus::metrics::PrometheusMetrics;

fn main() {
    let metrics = PrometheusMetrics::new();
    
    // Record some test metrics
    metrics.increment_exchanges("test-route");
    metrics.increment_errors("test-route", "timeout");
    metrics.record_exchange_duration("test-route", Duration::from_millis(100));
    metrics.set_queue_depth("test-route", 5);
    metrics.record_circuit_breaker_change("test-route", "closed", "open");
    
    // Gather and display the output
    let output = metrics.gather();
    println!("=== Prometheus Metrics Output ===");
    println!("{}", output);
    
    // Check for correct label names
    if output.contains("route=\"test-route\"") {
        println!("\n✓ Correct label 'route' found in output");
    } else if output.contains("route_id=\"test-route\"") {
        println!("\n✗ Incorrect label 'route_id' still found in output");
    } else {
        println!("\n? No route labels found in output");
    }
}
