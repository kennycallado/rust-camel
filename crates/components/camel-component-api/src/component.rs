use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::component_metadata::ComponentMetadata;

use crate::component_context::ComponentContext;
use crate::endpoint::Endpoint;

/// A Component is a factory for Endpoints.
///
/// Each component handles a specific URI scheme (e.g. "timer", "log", "direct").
#[async_trait]
pub trait Component: Send + Sync {
    /// The URI scheme this component handles (e.g., "timer", "log").
    fn scheme(&self) -> &str;

    /// Declare this component's metadata (URI options, capabilities, version).
    ///
    /// Called once at registration time to harvest metadata into the catalog.
    /// Override to provide rich metadata; the default returns a minimal
    /// entry with just the scheme name.
    fn metadata(&self) -> ComponentMetadata {
        ComponentMetadata::minimal(self.scheme())
    }

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

    #[test]
    fn test_component_default_metadata() {
        let c = DummyComponent;
        let meta = c.metadata();
        assert_eq!(meta.scheme, "dummy");
        assert_eq!(meta.schema_version, ComponentMetadata::SCHEMA_VERSION);
        assert!(meta.uri_options.is_empty());
    }

    #[tokio::test]
    async fn test_component_default_lifecycle() {
        let c = DummyComponent;
        assert!(c.start().await.is_ok());
        assert!(c.stop().await.is_ok());
    }
}
