use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};

use crate::PrometheusMetrics;
use async_trait::async_trait;
use camel_api::{CamelError, HealthChecker, Lifecycle, MetricsCollector, ServiceStatus};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};
use tracing::{debug, info, warn};

pub struct PrometheusService {
    addr: SocketAddr,
    metrics: Arc<PrometheusMetrics>,
    server_handle: Option<JoinHandle<()>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// The actual bound port (set after start(), 0 before)
    bound_port: Arc<AtomicU16>,
    /// Current service status (Stopped=0, Started=1, Failed=2)
    status: Arc<AtomicU8>,
    health_checker: Option<HealthChecker>,
    /// Guard to prevent double-start
    started: AtomicBool,
}

impl PrometheusService {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            metrics: Arc::new(PrometheusMetrics::new()),
            server_handle: None,
            shutdown_tx: None,
            bound_port: Arc::new(AtomicU16::new(0)),
            status: Arc::new(AtomicU8::new(0)),
            health_checker: None,
            started: AtomicBool::new(false),
        }
    }

    /// Returns the actual port the server is listening on (after start())
    ///
    /// Returns 0 if the service hasn't been started yet.
    pub fn port(&self) -> u16 {
        self.bound_port.load(Ordering::SeqCst)
    }

    /// Returns a cloneable accessor for the bound port.
    ///
    /// Use this when you need to read the port after the service has been
    /// moved into a CamelContext or other container.
    pub fn port_accessor(&self) -> Arc<AtomicU16> {
        Arc::clone(&self.bound_port)
    }

    pub fn status_arc(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.status)
    }

    /// Set the health checker closure that will be called to get system health.
    ///
    /// The closure should capture a reference to CamelContext's health_check method.
    pub fn set_health_checker(&mut self, checker: HealthChecker) {
        self.health_checker = Some(checker);
    }

    /// Get a clone of the health checker if one is set.
    pub fn health_checker(&self) -> Option<HealthChecker> {
        self.health_checker.clone()
    }
}

#[async_trait]
impl Lifecycle for PrometheusService {
    fn name(&self) -> &str {
        "prometheus"
    }

    fn as_metrics_collector(&self) -> Option<Arc<dyn MetricsCollector>> {
        Some(Arc::clone(&self.metrics) as Arc<dyn MetricsCollector>)
    }

    fn status(&self) -> ServiceStatus {
        match self.status.load(Ordering::SeqCst) {
            0 => ServiceStatus::Stopped,
            1 => ServiceStatus::Started,
            2 => ServiceStatus::Failed,
            _ => ServiceStatus::Failed,
        }
    }

    async fn start(&mut self) -> Result<(), CamelError> {
        use tokio::net::TcpListener;

        if self
            .started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(CamelError::Config(
                "PrometheusService already started".to_string(),
            ));
        }

        let listener = TcpListener::bind(self.addr).await.map_err(|e| {
            self.status.store(2, Ordering::SeqCst);
            self.started.store(false, Ordering::SeqCst);
            CamelError::Io(e.to_string())
        })?;

        let actual_port = listener.local_addr().map(|addr| addr.port()).map_err(|e| {
            self.status.store(2, Ordering::SeqCst);
            self.started.store(false, Ordering::SeqCst);
            CamelError::Io(e.to_string())
        })?;

        self.bound_port.store(actual_port, Ordering::SeqCst);

        let metrics = Arc::clone(&self.metrics);
        let health_checker = self.health_checker.clone();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let handle = tokio::spawn(async move {
            if let Err(err) =
                crate::MetricsServer::run_with_listener_and_health_checker_with_shutdown(
                    listener,
                    metrics,
                    health_checker,
                    shutdown_rx,
                )
                .await
            {
                warn!("prometheus metrics server exited with error: {err}");
            }
        });

        self.shutdown_tx = Some(shutdown_tx);
        self.server_handle = Some(handle);
        self.status.store(1, Ordering::SeqCst);
        info!(port = %actual_port, "prometheus metrics service started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        if let Some(handle) = self.server_handle.take() {
            let mut handle = handle;
            match timeout(Duration::from_secs(5), &mut handle).await {
                Ok(join_result) => {
                    if let Err(e) = join_result {
                        return Err(CamelError::Io(format!(
                            "prometheus server task join failed: {e}"
                        )));
                    }
                }
                Err(_) => {
                    debug!("prometheus server shutdown timed out; aborting task");
                    handle.abort();
                    let _ = handle.await;
                }
            }
        }
        self.status.store(0, Ordering::SeqCst);
        self.started.store(false, Ordering::SeqCst);
        debug!("prometheus metrics service stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::HealthReport;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::Ordering;

    #[test]
    fn test_create_prometheus_service() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 9090);
        let service = PrometheusService::new(addr);
        assert_eq!(service.name(), "prometheus");
    }

    #[tokio::test]
    async fn test_prometheus_service_status_transitions() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
        let mut service = PrometheusService::new(addr);

        assert_eq!(service.status(), ServiceStatus::Stopped);

        service.start().await.unwrap();
        assert_eq!(service.status(), ServiceStatus::Started);

        service.stop().await.unwrap();
        assert_eq!(service.status(), ServiceStatus::Stopped);
    }

    #[tokio::test]
    async fn test_stop_uses_graceful_shutdown_signal() {
        use std::sync::atomic::Ordering as AtomicOrdering;

        crate::server::test_reset_graceful_shutdown_observability();

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let mut service = PrometheusService::new(addr);
        service.start().await.unwrap();

        service.stop().await.unwrap();

        let signal_count = crate::server::test_graceful_shutdown_signal_count();
        assert!(
            signal_count.load(AtomicOrdering::SeqCst) >= 1,
            "expected at least one graceful shutdown signal"
        );
    }

    #[tokio::test]
    async fn test_port_and_port_accessor_after_start() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let mut service = PrometheusService::new(addr);

        assert_eq!(service.port(), 0);
        let accessor = service.port_accessor();
        assert_eq!(accessor.load(Ordering::SeqCst), 0);

        service.start().await.unwrap();
        let port = service.port();
        assert!(port > 0);
        assert_eq!(accessor.load(Ordering::SeqCst), port);

        service.stop().await.unwrap();
    }

    #[test]
    fn test_as_metrics_collector_returns_metrics_instance() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9090);
        let service = PrometheusService::new(addr);
        let collector = service.as_metrics_collector().unwrap();
        collector.increment_exchanges("route-a");
        let output = service.metrics.gather();
        assert!(output.contains("camel_exchanges_total"));
        assert!(output.contains("route-a"));
    }

    #[test]
    fn test_unknown_internal_status_maps_to_failed() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9090);
        let service = PrometheusService::new(addr);
        service.status_arc().store(9, Ordering::SeqCst);
        assert_eq!(service.status(), ServiceStatus::Failed);
    }

    #[test]
    fn test_health_checker_injection() {
        use camel_api::HealthStatus;

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 9090);
        let mut service = PrometheusService::new(addr);

        // Initially no health checker
        assert!(service.health_checker().is_none());

        // Inject health checker
        let checker = Arc::new(|| HealthReport {
            status: HealthStatus::Healthy,
            services: vec![],
            ..Default::default()
        });

        service.set_health_checker(checker);
        assert!(service.health_checker().is_some());

        // Call the checker
        let report = service.health_checker().unwrap()();
        assert_eq!(report.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_prometheus_service_with_socket_addr() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9091);
        let service = PrometheusService::new(addr);
        assert_eq!(service.name(), "prometheus");
    }

    #[tokio::test]
    async fn test_double_start_returns_error() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let mut service = PrometheusService::new(addr);

        service.start().await.unwrap();
        assert_eq!(service.status(), ServiceStatus::Started);

        let result = service.start().await;
        assert!(result.is_err(), "second start() should return an error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("already started"),
            "error should mention 'already started', got: {err_msg}"
        );

        service.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_start_allowed_again_after_stop() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let mut service = PrometheusService::new(addr);

        service.start().await.unwrap();
        service.stop().await.unwrap();

        // After stop, start should succeed again
        service.start().await.unwrap();
        assert_eq!(service.status(), ServiceStatus::Started);

        service.stop().await.unwrap();
    }
}
