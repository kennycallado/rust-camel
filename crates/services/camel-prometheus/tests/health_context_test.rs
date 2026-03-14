use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use camel_core::context::CamelContext;
use camel_prometheus::PrometheusService;

fn bind_addr(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port)
}

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
async fn test_health_with_context() {
    let prometheus = PrometheusService::new(bind_addr(0));
    let port_accessor = prometheus.port_accessor();

    let mut ctx = CamelContext::new().with_lifecycle(prometheus);

    ctx.start().await.unwrap();

    let port = port_accessor.load(std::sync::atomic::Ordering::SeqCst);
    wait_for_server(port, 2000).await.unwrap();

    // Test /healthz (always 200)
    let response = reqwest::get(format!("http://127.0.0.1:{}/healthz", port))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // Test /health (detailed)
    let response = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let body = response.text().await.unwrap();
    assert!(body.contains("Healthy"));

    ctx.stop().await.unwrap();
}
