use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use camel_api::Lifecycle;
use camel_prometheus::PrometheusService;

async fn wait_for_server(port: u16, timeout_ms: u64) -> Result<(), String> {
    let start = std::time::Instant::now();
    let client = reqwest::Client::new();

    loop {
        match client
            .get(format!("http://127.0.0.1:{}/healthz", port))
            .timeout(Duration::from_millis(100))
            .send()
            .await
        {
            Ok(_) => return Ok(()),
            Err(_) => {
                if start.elapsed().as_millis() > timeout_ms as u128 {
                    return Err(format!("Server on port {} did not start", port));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

#[tokio::test]
async fn test_healthz_endpoint() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
    let mut service = PrometheusService::new(addr);
    service.start().await.unwrap();
    let port = service.port();

    wait_for_server(port, 2000).await.unwrap();

    let response = reqwest::get(format!("http://127.0.0.1:{}/healthz", port))
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    service.stop().await.unwrap();
}

#[tokio::test]
async fn test_readyz_endpoint_healthy() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
    let mut service = PrometheusService::new(addr);
    service.start().await.unwrap();
    let port = service.port();

    wait_for_server(port, 2000).await.unwrap();

    let response = reqwest::get(format!("http://127.0.0.1:{}/readyz", port))
        .await
        .unwrap();

    // Without health checker, should return 200 with empty healthy report
    assert_eq!(response.status(), 200);

    let body = response.text().await.unwrap();
    assert!(body.contains("Healthy"));

    service.stop().await.unwrap();
}

#[tokio::test]
async fn test_health_endpoint() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
    let mut service = PrometheusService::new(addr);
    service.start().await.unwrap();
    let port = service.port();

    wait_for_server(port, 2000).await.unwrap();

    let response = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body = response.text().await.unwrap();
    assert!(body.contains("Healthy"));

    service.stop().await.unwrap();
}
