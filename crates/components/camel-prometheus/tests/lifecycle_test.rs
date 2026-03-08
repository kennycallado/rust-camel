use std::time::Duration;

use camel_api::Lifecycle;
use camel_prometheus::PrometheusService;

#[tokio::test]
async fn test_prometheus_service_lifecycle() {
    let mut service = PrometheusService::new(9091);
    
    assert_eq!(service.name(), "prometheus");
    
    // Verify MetricsCollector is available
    let collector = service.as_metrics_collector();
    assert!(collector.is_some());
    
    // Record some metrics so they appear in output
    let collector = collector.unwrap();
    collector.increment_exchanges("test-route");
    
    // Start service
    service.start().await.unwrap();
    
    // Wait for server to start
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Test HTTP endpoint
    let response = reqwest::get("http://127.0.0.1:9091/metrics")
        .await
        .expect("Failed to fetch metrics");
    
    assert!(response.status().is_success());
    
    let body = response.text().await.unwrap();
    assert!(body.contains("camel_exchanges_total"));
    
    // Stop service
    service.stop().await.unwrap();
}

#[tokio::test]
async fn test_prometheus_service_with_context() {
    use camel_core::context::CamelContext;
    
    let prometheus = PrometheusService::new(9092);
    let metrics = prometheus.as_metrics_collector().unwrap();
    
    let mut ctx = CamelContext::with_metrics(metrics)
        .with_lifecycle(prometheus);
    
    ctx.start().await.unwrap();
    
    // Wait for server to start
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Test HTTP endpoint
    let response = reqwest::get("http://127.0.0.1:9092/metrics")
        .await
        .expect("Failed to fetch metrics");
    
    assert!(response.status().is_success());
    
    ctx.stop().await.unwrap();
}
