use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use crate::PrometheusMetrics;
use async_trait::async_trait;
use camel_api::{CamelError, Lifecycle, MetricsCollector};
use tokio::task::JoinHandle;

pub struct PrometheusService {
    addr: SocketAddr,
    metrics: Arc<PrometheusMetrics>,
    server_handle: Option<JoinHandle<()>>,
    /// The actual bound port (set after start(), 0 before)
    bound_port: Arc<AtomicU16>,
}

impl PrometheusService {
    pub fn new(port: u16) -> Self {
        Self {
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port),
            metrics: Arc::new(PrometheusMetrics::new()),
            server_handle: None,
            bound_port: Arc::new(AtomicU16::new(0)),
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
}

#[async_trait]
impl Lifecycle for PrometheusService {
    fn name(&self) -> &str {
        "prometheus"
    }

    fn as_metrics_collector(&self) -> Option<Arc<dyn MetricsCollector>> {
        Some(Arc::clone(&self.metrics) as Arc<dyn MetricsCollector>)
    }

    async fn start(&mut self) -> Result<(), CamelError> {
        use tokio::net::TcpListener;

        // Bind listener BEFORE spawning to detect errors early
        let listener = TcpListener::bind(self.addr)
            .await
            .map_err(|e| CamelError::Io(e.to_string()))?;

        // Store actual port (useful when binding to port 0)
        let actual_port = listener
            .local_addr()
            .map(|addr| addr.port())
            .map_err(|e| CamelError::Io(e.to_string()))?;
        self.bound_port.store(actual_port, Ordering::SeqCst);

        let metrics = Arc::clone(&self.metrics);

        let handle = tokio::spawn(async move {
            crate::MetricsServer::run_with_listener(listener, metrics).await;
        });

        self.server_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_prometheus_service() {
        let service = PrometheusService::new(9090);
        assert_eq!(service.name(), "prometheus");
    }
}
