use async_trait::async_trait;
use crate::{CamelError, MetricsCollector};
use std::sync::Arc;

/// Lifecycle trait for background services.
/// 
/// This trait follows Apache Camel's Service pattern but uses a different name
/// to avoid confusion with tower::Service which is the core of rust-camel's
/// request processing.
#[async_trait]
pub trait Lifecycle: Send + Sync {
    /// Service name for logging
    fn name(&self) -> &str;
    
    /// Start service (called during CamelContext.start())
    async fn start(&mut self) -> Result<(), CamelError>;
    
    /// Stop service (called during CamelContext.stop())
    async fn stop(&mut self) -> Result<(), CamelError>;
    
    /// Optional: expose MetricsCollector for auto-registration
    fn as_metrics_collector(&self) -> Option<Arc<dyn MetricsCollector>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestService;
    
    #[async_trait]
    impl Lifecycle for TestService {
        fn name(&self) -> &str {
            "test"
        }
        
        async fn start(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
        
        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_lifecycle_trait() {
        let mut service = TestService;
        assert_eq!(service.name(), "test");
        service.start().await.unwrap();
        service.stop().await.unwrap();
    }
}
