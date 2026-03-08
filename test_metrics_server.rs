use std::net::SocketAddr;
use std::sync::Arc;
use camel_prometheus::{MetricsServer, PrometheusMetrics};
use camel_api::metrics::MetricsCollector;

#[tokio::main]
async fn main() {
    // Create metrics instance
    let metrics = Arc::new(PrometheusMetrics::new());
    
    // Add some test metrics
    metrics.increment_exchanges("test-route");
    metrics.record_exchange_duration("test-route", 100);
    metrics.set_queue_depth("test-queue", 5);
    
    // Start server
    let addr: SocketAddr = "127.0.0.1:9090".parse().unwrap();
    println!("Starting metrics server on {}", addr);
    
    // Run the server (this will block indefinitely)
    MetricsServer::run(addr, metrics).await;
}