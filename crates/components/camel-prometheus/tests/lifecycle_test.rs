use std::time::{Duration, Instant};

use camel_api::Lifecycle;
use camel_prometheus::PrometheusService;

/// Wait for the Prometheus server to become available with retry mechanism.
/// 
/// Polls the /metrics endpoint until it responds or timeout is reached.
/// This is more reliable than fixed sleep durations.
async fn wait_for_server(port: u16, timeout_ms: u64) -> Result<(), String> {
    let start = Instant::now();
    let client = reqwest::Client::new();
    
    loop {
        match client
            .get(&format!("http://127.0.0.1:{}/metrics", port))
            .timeout(Duration::from_millis(100))
            .send()
            .await
        {
            Ok(_) => return Ok(()),
            Err(_) => {
                if start.elapsed().as_millis() > timeout_ms as u128 {
                    return Err(format!(
                        "Server on port {} did not start within {}ms",
                        port, timeout_ms
                    ));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

#[tokio::test]
async fn test_prometheus_service_lifecycle() {
    // Use port 0 to let OS assign an available port (avoids port conflicts)
    let mut service = PrometheusService::new(0);
    
    assert_eq!(service.name(), "prometheus");
    
    // Verify MetricsCollector is available
    let collector = service.as_metrics_collector();
    assert!(collector.is_some());
    
    // Record metrics before starting (multiple types)
    let collector = collector.unwrap();
    collector.increment_exchanges("test-route");
    collector.increment_errors("test-route", "SomeError");
    collector.set_queue_depth("test-route", 5);
    
    // Start service
    service.start().await.expect("Service should start successfully");
    
    // Get actual port assigned by OS
    let port = service.port();
    assert!(port > 0, "Port should be assigned after start");
    
    // Wait for server with retry mechanism (not fixed sleep)
    wait_for_server(port, 2000)
        .await
        .expect("Server should start within timeout");
    
    // Test HTTP endpoint
    let response = reqwest::get(&format!("http://127.0.0.1:{}/metrics", port))
        .await
        .expect("Failed to fetch metrics");
    
    assert!(response.status().is_success());
    
    let body = response.text().await.expect("Failed to read response body");
    
    // Verify multiple metric types are present
    assert!(
        body.contains("camel_exchanges_total"),
        "Response should contain exchanges metric"
    );
    assert!(
        body.contains("camel_errors_total"),
        "Response should contain errors metric"
    );
    assert!(
        body.contains("camel_queue_depth"),
        "Response should contain queue depth metric"
    );
    assert!(
        body.contains("test-route"),
        "Response should contain route label"
    );
    
    // Stop service
    service.stop().await.expect("Service should stop successfully");
}

#[tokio::test]
async fn test_prometheus_service_bind_error() {
    // Start first service on port 0 (OS assigns available port)
    let mut service1 = PrometheusService::new(0);
    service1
        .start()
        .await
        .expect("First service should start successfully");
    let port = service1.port();
    
    // Wait for first server to be ready
    wait_for_server(port, 2000)
        .await
        .expect("First server should start");
    
    // Try to start second service on the same port - should fail
    let mut service2 = PrometheusService::new(port);
    let result = service2.start().await;
    
    assert!(
        result.is_err(),
        "Should fail to bind to already-used port {}",
        port
    );
    
    // Verify error message mentions the issue
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("AddrInUse") 
            || error_msg.contains("Address already in use")
            || error_msg.contains("address already in use"),
        "Error should indicate address in use: {}",
        error_msg
    );
    
    // Cleanup: stop first service
    service1.stop().await.expect("First service should stop");
}

#[tokio::test]
async fn test_prometheus_service_with_context() {
    use camel_core::context::CamelContext;
    
    // Create service and get port accessor before moving service to context
    let prometheus = PrometheusService::new(0);
    let port_accessor = prometheus.port_accessor();
    let metrics = prometheus.as_metrics_collector().unwrap();
    
    let mut ctx = CamelContext::with_metrics(metrics).with_lifecycle(prometheus);
    
    // Start context (which starts the prometheus service)
    ctx.start().await.expect("Context should start successfully");
    
    // Get the actual port from the accessor
    let port = port_accessor.load(std::sync::atomic::Ordering::SeqCst);
    assert!(port > 0, "Port should be assigned after context start");
    
    // Wait for server with retry mechanism
    wait_for_server(port, 2000)
        .await
        .expect("Server should start within timeout");
    
    // Test HTTP endpoint
    let response = reqwest::get(&format!("http://127.0.0.1:{}/metrics", port))
        .await
        .expect("Failed to fetch metrics");
    
    assert!(
        response.status().is_success(),
        "Metrics endpoint should return success"
    );
    
    // Stop context (which stops the prometheus service)
    ctx.stop().await.expect("Context should stop successfully");
}

#[tokio::test]
async fn test_prometheus_service_multiple_start_stop_cycles() {
    // Verify that we can start/stop multiple times with different ports
    for i in 0..3 {
        let mut service = PrometheusService::new(0);
        service.start().await.expect(&format!("Cycle {}: start should succeed", i));
        
        let port = service.port();
        wait_for_server(port, 2000)
            .await
            .expect(&format!("Cycle {}: server should start", i));
        
        service.stop().await.expect(&format!("Cycle {}: stop should succeed", i));
    }
}
