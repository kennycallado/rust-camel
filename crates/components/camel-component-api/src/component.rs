use async_trait::async_trait;
use camel_api::CamelError;

use crate::component_context::ComponentContext;
use crate::endpoint::Endpoint;

/// A Component is a factory for Endpoints.
///
/// Each component handles a specific URI scheme (e.g. "timer", "log", "direct").
#[async_trait]
pub trait Component: Send + Sync {
    /// The URI scheme this component handles (e.g., "timer", "log").
    fn scheme(&self) -> &str;

    /// Create an endpoint from a URI string.
    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError>;

    /// Start the component (e.g. connect to external systems).
    ///
    /// Default: no-op, returns `Ok(())`.
    async fn start(&self) -> Result<(), CamelError> {
        Ok(())
    }

    /// Stop the component (e.g. release resources, close connections).
    ///
    /// Default: no-op, returns `Ok(())`.
    async fn stop(&self) -> Result<(), CamelError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyComponent;

    #[async_trait]
    impl Component for DummyComponent {
        fn scheme(&self) -> &str {
            "dummy"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Err(CamelError::EndpointCreationFailed("not implemented".into()))
        }
    }

    #[tokio::test]
    async fn test_component_default_lifecycle() {
        let c = DummyComponent;
        assert!(c.start().await.is_ok());
        assert!(c.stop().await.is_ok());
    }
}
