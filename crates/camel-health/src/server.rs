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

pub struct HealthServer {
    addr: SocketAddr,
    server_handle: Option<JoinHandle<()>>,
    bound_port: Arc<AtomicU16>,
    status: Arc<AtomicU8>,
    health_source: Option<Arc<dyn HealthSource>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    handler_timeout: Duration,
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
            handler_timeout: crate::DEFAULT_HANDLER_TIMEOUT,
        }
    }

    pub fn set_health_source(&mut self, source: Arc<dyn HealthSource>) {
        self.health_source = Some(source);
    }

    /// R4-L11: set the handler-level probe timeout. Must be > 0.
    pub fn set_handler_timeout(&mut self, timeout: Duration) {
        assert!(!timeout.is_zero(), "health handler timeout must be > 0");
        self.handler_timeout = timeout;
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
        let handler_timeout = self.handler_timeout;

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            let source =
                source.unwrap_or_else(|| Arc::new(DefaultHealthSource) as Arc<dyn HealthSource>);
            let app = crate::health_router_with_timeout(source, handler_timeout);
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
        let shutdown_timeout = self.handler_timeout + Duration::from_secs(2);
        if let Some(handle) = self.server_handle.take() {
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }

            tokio::pin!(handle);
            match tokio::time::timeout(shutdown_timeout, &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(join_err)) => {
                    // log-policy: system-broken
                    tracing::error!("Health server task failed during shutdown: {}", join_err);
                }
                Err(_) => {
                    tracing::warn!(
                        "Health server did not shut down within {:?}, aborting",
                        shutdown_timeout
                    );
                    handle.abort();
                    let _ = handle.await;
                }
            }
        }
        self.status.store(STATUS_STOPPED, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
impl HealthServer {
    /// Replace the server handle with one that has already panicked,
    /// testing the `Ok(Err(JoinError))` arm in `stop()`.
    pub fn inject_panicked_handle(&mut self) {
        let handle = tokio::spawn(async {
            panic!("injected panic for JoinError test");
        });
        let _ = self.server_handle.take();
        self.server_handle = Some(handle);
    }

    /// Remove the shutdown sender so `stop()` never signals graceful shutdown.
    /// The abort path fires when `stop()` times out waiting for a server that
    /// never receives a shutdown signal.
    pub fn strip_shutdown_signal(&mut self) {
        self.shutdown_tx = None;
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

    #[tokio::test]
    async fn test_stop_aborts_on_timeout() {
        let addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let mut server = HealthServer::new(addr);
        // Small handler_timeout: shutdown_timeout = 100ms + 2s = 2.1s
        server.set_handler_timeout(Duration::from_millis(100));
        server.start().await.unwrap();
        let port = server.port();
        assert!(port > 0);

        // Strip shutdown signal — server never receives graceful shutdown,
        // so abort path fires after shutdown_timeout expires.
        server.strip_shutdown_signal();

        let start = std::time::Instant::now();
        server.stop().await.unwrap();
        let elapsed = start.elapsed();

        // Old implementation: 5s const timeout → elapsed ~5s
        // New implementation: 2.1s timeout → elapsed ~2.1s
        // Bound at 4s to discriminate between old (5s) and new (2.1s)
        assert!(
            elapsed < Duration::from_secs(4),
            "stop() should abort promptly (2.1s timeout), took {:?}",
            elapsed
        );
        assert_eq!(server.status(), ServiceStatus::Stopped);

        // Port must be bindable — proves task was aborted and awaited,
        // not detached with the old drop-on-timeout behavior.
        let mut server2 = HealthServer::new(SocketAddr::from(([127, 0, 0, 1], port)));
        server2.start().await.unwrap();
        assert_eq!(server2.port(), port);
        server2.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_shutdown_timeout_derives_from_handler_timeout() {
        let addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let mut server = HealthServer::new(addr);
        // shutdown_timeout = handler_timeout + 2s
        server.set_handler_timeout(Duration::from_millis(50));
        server.start().await.unwrap();

        let start = std::time::Instant::now();
        server.stop().await.unwrap();
        let elapsed = start.elapsed();

        // With handler_timeout=50ms, shutdown_timeout=2.05s.
        // Old code: 5s const → elapsed could reach 5s.
        // New code: 2.05s → bound at 4s to discriminate.
        assert!(
            elapsed < Duration::from_secs(4),
            "stop() should use derived timeout (50ms+2s), took {:?}",
            elapsed
        );
        assert_eq!(server.status(), ServiceStatus::Stopped);
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn test_stop_logs_panic_on_join_error() {
        let addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let mut server = HealthServer::new(addr);
        server.start().await.unwrap();

        // Replace the handle with a panicking one to exercise Ok(Err(JoinError))
        server.inject_panicked_handle();

        let stop_result = server.stop().await;
        assert!(
            stop_result.is_ok(),
            "stop() should return Ok even on JoinError"
        );
        assert_eq!(server.status(), ServiceStatus::Stopped);

        // Verify the error! log was captured
        assert!(
            logs_contain("Health server task failed during shutdown"),
            "expected error-level log for JoinError, but none captured"
        );
    }
}
