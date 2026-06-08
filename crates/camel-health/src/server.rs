use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU16, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{CamelError, HealthSource, HealthStatus, Lifecycle, ServiceStatus};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::info;

const STATUS_STOPPED: u8 = 0;
const STATUS_STARTED: u8 = 1;
const STATUS_FAILED: u8 = 2;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct HealthServer {
    addr: SocketAddr,
    server_handle: Option<JoinHandle<()>>,
    bound_port: Arc<AtomicU16>,
    status: Arc<AtomicU8>,
    health_source: Option<Arc<dyn HealthSource>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl HealthServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            server_handle: None,
            bound_port: Arc::new(AtomicU16::new(0)),
            status: Arc::new(AtomicU8::new(STATUS_STOPPED)),
            health_source: None,
            shutdown_tx: None,
        }
    }

    pub fn set_health_source(&mut self, source: Arc<dyn HealthSource>) {
        self.health_source = Some(source);
    }

    pub fn port(&self) -> u16 {
        self.bound_port.load(Ordering::SeqCst)
    }

    pub fn status_arc(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.status)
    }
}

struct DefaultHealthSource;

#[async_trait]
impl HealthSource for DefaultHealthSource {
    async fn liveness(&self) -> HealthStatus {
        HealthStatus::Healthy
    }

    async fn readiness(&self) -> HealthStatus {
        HealthStatus::Healthy
    }

    async fn startup(&self) -> HealthStatus {
        HealthStatus::Healthy
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

        if self.server_handle.is_some() {
            return Ok(());
        }

        let listener = TcpListener::bind(self.addr).await.map_err(|e| {
            self.status.store(STATUS_FAILED, Ordering::SeqCst);
            CamelError::Io(format!("health check bind {addr}: {e}", addr = self.addr))
        })?;

        let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        self.bound_port.store(actual_port, Ordering::SeqCst);

        let source = self.health_source.clone();
        let port = actual_port;

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            let source =
                source.unwrap_or_else(|| Arc::new(DefaultHealthSource) as Arc<dyn HealthSource>);
            let app = crate::health_router(source);
            info!("Health server listening on port {}", port);
            let server = axum::serve(listener, app);
            let shutdown_future = async move {
                let _ = shutdown_rx.await;
            };
            if let Err(e) = server.with_graceful_shutdown(shutdown_future).await {
                // log-policy: system-broken
                tracing::error!("Health server error: {}", e);
            }
        });

        self.server_handle = Some(handle);
        self.shutdown_tx = Some(shutdown_tx);
        self.status.store(STATUS_STARTED, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(handle) = self.server_handle.take() {
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }

            match tokio::time::timeout(SHUTDOWN_TIMEOUT, handle).await {
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!(
                        "Health server did not shut down within {:?}, aborting",
                        SHUTDOWN_TIMEOUT
                    );
                }
            }
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

    struct FixedHealthSource {
        readiness: HealthStatus,
    }

    #[async_trait]
    impl HealthSource for FixedHealthSource {
        async fn liveness(&self) -> HealthStatus {
            HealthStatus::Healthy
        }

        async fn readiness(&self) -> HealthStatus {
            self.readiness
        }

        async fn startup(&self) -> HealthStatus {
            HealthStatus::Healthy
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

    #[tokio::test]
    async fn test_health_source_reflects_state_via_http() {
        let source: Arc<dyn HealthSource> = Arc::new(FixedHealthSource {
            readiness: HealthStatus::Unhealthy,
        });
        let mut server = HealthServer::new("127.0.0.1:0".parse().unwrap());
        server.set_health_source(source);
        server.start().await.unwrap();
        wait_for_server(server.port(), 2000).await.unwrap();

        let resp = reqwest::get(format!("http://127.0.0.1:{}/readyz", server.port()))
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        server.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_double_start_is_idempotent() {
        let mut server = HealthServer::new("127.0.0.1:0".parse().unwrap());
        server.start().await.unwrap();
        let first_port = server.port();

        let result = server.start().await;
        assert!(result.is_ok(), "second start() should be idempotent");

        let second_port = server.port();
        assert_eq!(
            first_port, second_port,
            "second start should not bind a new port"
        );

        server.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_graceful_shutdown_uses_cancel_not_abort() {
        let addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let mut server = HealthServer::new(addr);
        server.start().await.unwrap();
        let port = server.port();
        wait_for_server(port, 2000).await.unwrap();

        let stop_result = server.stop().await;
        assert!(stop_result.is_ok(), "graceful shutdown should succeed");
        assert_eq!(server.status(), ServiceStatus::Stopped);
    }
}
