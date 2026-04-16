use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU16, Ordering};

use async_trait::async_trait;
use camel_api::{CamelError, HealthChecker, Lifecycle, ServiceStatus};
use tokio::task::JoinHandle;
use tracing::info;

const STATUS_STOPPED: u8 = 0;
const STATUS_STARTED: u8 = 1;
const STATUS_FAILED: u8 = 2;

pub struct HealthServer {
    addr: SocketAddr,
    server_handle: Option<JoinHandle<()>>,
    bound_port: Arc<AtomicU16>,
    status: Arc<AtomicU8>,
    health_checker: Option<HealthChecker>,
    liveness_checker: Option<HealthChecker>,
    startup_checker: Option<HealthChecker>,
}

impl HealthServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            server_handle: None,
            bound_port: Arc::new(AtomicU16::new(0)),
            status: Arc::new(AtomicU8::new(STATUS_STOPPED)),
            health_checker: None,
            liveness_checker: None,
            startup_checker: None,
        }
    }

    pub fn new_with_checker(addr: SocketAddr, checker: Option<HealthChecker>) -> Self {
        Self {
            addr,
            server_handle: None,
            bound_port: Arc::new(AtomicU16::new(0)),
            status: Arc::new(AtomicU8::new(STATUS_STOPPED)),
            health_checker: checker,
            liveness_checker: None,
            startup_checker: None,
        }
    }

    pub fn set_health_checker(&mut self, checker: HealthChecker) {
        self.health_checker = Some(checker);
    }

    pub fn set_liveness_checker(&mut self, checker: HealthChecker) {
        self.liveness_checker = Some(checker);
    }

    pub fn set_startup_checker(&mut self, checker: HealthChecker) {
        self.startup_checker = Some(checker);
    }

    pub fn port(&self) -> u16 {
        self.bound_port.load(Ordering::SeqCst)
    }

    pub fn status_arc(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.status)
    }
}

impl camel_api::HealthSource for HealthServer {
    fn liveness(&self) -> camel_api::HealthStatus {
        self.liveness_checker
            .as_ref()
            .map(|c| c().status)
            .unwrap_or(camel_api::HealthStatus::Healthy)
    }

    fn readiness(&self) -> camel_api::HealthStatus {
        self.health_checker
            .as_ref()
            .map(|c| c().status)
            .unwrap_or(camel_api::HealthStatus::Healthy)
    }

    fn startup(&self) -> camel_api::HealthStatus {
        self.startup_checker
            .as_ref()
            .map(|c| c().status)
            .unwrap_or(camel_api::HealthStatus::Healthy)
    }
}

#[async_trait]
impl Lifecycle for HealthServer {
    fn name(&self) -> &str {
        "health"
    }

    fn status(&self) -> ServiceStatus {
        match self.status.load(Ordering::SeqCst) {
            STATUS_STOPPED => ServiceStatus::Stopped,
            STATUS_STARTED => ServiceStatus::Started,
            _ => ServiceStatus::Failed,
        }
    }

    async fn start(&mut self) -> Result<(), CamelError> {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind(self.addr).await.map_err(|e| {
            self.status.store(STATUS_FAILED, Ordering::SeqCst);
            CamelError::Io(e.to_string())
        })?;

        let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        self.bound_port.store(actual_port, Ordering::SeqCst);

        let checker = self.health_checker.take();
        let liveness = self.liveness_checker.take();
        let startup = self.startup_checker.take();
        let port = actual_port;

        let handle = tokio::spawn(async move {
            let app = crate::health_router(checker, liveness, startup);
            info!("Health server listening on port {}", port);
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("Health server error: {}", e);
            }
        });

        self.server_handle = Some(handle);
        self.status.store(STATUS_STARTED, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
        self.status.store(STATUS_STOPPED, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
                        return Err(format!("Health server on port {} did not start", port));
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    }

    #[tokio::test]
    async fn test_health_server_start_stop() {
        let addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let mut server = HealthServer::new(addr);
        assert_eq!(server.status(), ServiceStatus::Stopped);

        server.start().await.unwrap();
        assert_eq!(server.status(), ServiceStatus::Started);
        let port = server.port();
        assert!(port > 0);

        wait_for_server(port, 2000).await.unwrap();

        server.stop().await.unwrap();
        assert_eq!(server.status(), ServiceStatus::Stopped);
    }

    #[tokio::test]
    async fn test_health_server_endpoints() {
        let addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let mut server = HealthServer::new(addr);
        server.start().await.unwrap();
        let port = server.port();

        wait_for_server(port, 2000).await.unwrap();

        let healthz = reqwest::get(format!("http://127.0.0.1:{}/healthz", port))
            .await
            .unwrap();
        assert_eq!(healthz.status(), 200);

        let readyz = reqwest::get(format!("http://127.0.0.1:{}/readyz", port))
            .await
            .unwrap();
        assert_eq!(readyz.status(), 200);

        let health = reqwest::get(format!("http://127.0.0.1:{}/health", port))
            .await
            .unwrap();
        assert_eq!(health.status(), 200);

        server.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_status_arc_reflects_state() {
        let addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let mut server = HealthServer::new(addr);
        let status_arc = server.status_arc();

        assert_eq!(status_arc.load(Ordering::SeqCst), STATUS_STOPPED);
        server.start().await.unwrap();
        assert_eq!(status_arc.load(Ordering::SeqCst), STATUS_STARTED);
        server.stop().await.unwrap();
        assert_eq!(status_arc.load(Ordering::SeqCst), STATUS_STOPPED);
    }
}

#[cfg(test)]
mod server_tests {
    use super::*;
    use camel_api::{HealthReport, HealthSource, HealthStatus};

    fn make_server() -> HealthServer {
        HealthServer::new("127.0.0.1:0".parse().unwrap())
    }

    #[test]
    fn test_health_source_liveness_no_checker_returns_healthy() {
        let server = make_server();
        assert_eq!(server.liveness(), HealthStatus::Healthy);
    }

    #[test]
    fn test_health_source_readiness_no_checker_returns_healthy() {
        let server = make_server();
        assert_eq!(server.readiness(), HealthStatus::Healthy);
    }

    #[test]
    fn test_health_source_startup_no_checker_returns_healthy() {
        let server = make_server();
        assert_eq!(server.startup(), HealthStatus::Healthy);
    }

    #[test]
    fn test_health_source_liveness_with_unhealthy_checker() {
        let mut server = make_server();
        let checker = Arc::new(|| HealthReport {
            status: HealthStatus::Unhealthy,
            ..Default::default()
        }) as camel_api::HealthChecker;
        server.set_liveness_checker(checker);
        assert_eq!(server.liveness(), HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_source_startup_with_unhealthy_checker() {
        let mut server = make_server();
        let checker = Arc::new(|| HealthReport {
            status: HealthStatus::Unhealthy,
            ..Default::default()
        }) as camel_api::HealthChecker;
        server.set_startup_checker(checker);
        assert_eq!(server.startup(), HealthStatus::Unhealthy);
    }
}
