use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::task::JoinHandle;
use camel_api::{CamelError, Lifecycle, MetricsCollector};
use crate::PrometheusMetrics;

pub struct PrometheusService {
    addr: SocketAddr,
    metrics: Arc<PrometheusMetrics>,
    server_handle: Option<JoinHandle<()>>,
}

impl PrometheusService {
    pub fn new(port: u16) -> Self {
        Self {
            addr: format!("0.0.0.0:{}", port).parse().unwrap(),
            metrics: Arc::new(PrometheusMetrics::new()),
            server_handle: None,
        }
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
        let addr = self.addr;
        let metrics = Arc::clone(&self.metrics);
        
        let handle = tokio::spawn(async move {
            crate::MetricsServer::run(addr, metrics).await;
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
