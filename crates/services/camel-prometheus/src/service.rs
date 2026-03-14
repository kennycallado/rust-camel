use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU16, Ordering};

use crate::PrometheusMetrics;
use async_trait::async_trait;
use camel_api::{CamelError, HealthReport, Lifecycle, MetricsCollector, ServiceStatus};
use tokio::task::JoinHandle;

type HealthChecker = Arc<dyn Fn() -> HealthReport + Send + Sync>;

pub struct PrometheusService {
    addr: SocketAddr,
    metrics: Arc<PrometheusMetrics>,
    server_handle: Option<JoinHandle<()>>,
    /// The actual bound port (set after start(), 0 before)
    bound_port: Arc<AtomicU16>,
    /// Current service status (Stopped=0, Started=1, Failed=2)
    status: Arc<AtomicU8>,
    health_checker: Option<HealthChecker>,
}

impl PrometheusService {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            metrics: Arc::new(PrometheusMetrics::new()),
            server_handle: None,
            bound_port: Arc::new(AtomicU16::new(0)),
            status: Arc::new(AtomicU8::new(0)),
            health_checker: None,
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

        let listener = TcpListener::bind(self.addr).await.map_err(|e| {
            self.status.store(2, Ordering::SeqCst);
            CamelError::Io(e.to_string())
        })?;

        let actual_port = listener.local_addr().map(|addr| addr.port()).map_err(|e| {
            self.status.store(2, Ordering::SeqCst);
            CamelError::Io(e.to_string())
        })?;

        self.bound_port.store(actual_port, Ordering::SeqCst);

        let metrics = Arc::clone(&self.metrics);
        let health_checker = self.health_checker.clone();

        let handle = tokio::spawn(async move {
            match health_checker {
                Some(checker) => {
                    crate::MetricsServer::run_with_listener_and_health_checker(
                        listener, metrics, checker,
                    )
                    .await;
                }
                None => {
                    crate::MetricsServer::run_with_listener(listener, metrics).await;
                }
            }
        });

        self.server_handle = Some(handle);
        self.status.store(1, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
        self.status.store(0, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_prometheus_service() {
        let service = PrometheusService::new(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            9090,
        ));
        assert_eq!(service.name(), "prometheus");
    }

    #[tokio::test]
    async fn test_prometheus_service_status_transitions() {
        let mut service = PrometheusService::new(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            0,
        ));

        assert_eq!(service.status(), ServiceStatus::Stopped);

        service.start().await.unwrap();
        assert_eq!(service.status(), ServiceStatus::Started);

        service.stop().await.unwrap();
        assert_eq!(service.status(), ServiceStatus::Stopped);
    }

    #[test]
    fn test_health_checker_injection() {
        use camel_api::HealthStatus;

        let mut service = PrometheusService::new(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            9090,
        ));

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
        use std::net::{IpAddr, Ipv4Addr};
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9091);
        let service = PrometheusService::new(addr);
        assert_eq!(service.name(), "prometheus");
    }
}
