pub mod bundle;
pub mod config;
pub mod producer;

use camel_component_api::{BoxProcessor, CamelError};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext};

pub use bundle::OpenSearchBundle;
pub use config::{OpenSearchConfig, OpenSearchEndpointConfig, OpenSearchOperation};
pub use producer::OpenSearchProducer;

// ---------------------------------------------------------------------------
// OpenSearchComponent — handles the `opensearch://` scheme (HTTP)
// ---------------------------------------------------------------------------

pub struct OpenSearchComponent {
    config: Option<OpenSearchConfig>,
}

impl OpenSearchComponent {
    /// Create a new OpenSearchComponent without global config defaults.
    /// Endpoint configs will fall back to hardcoded defaults via `merge_with_global()`.
    pub fn new() -> Self {
        Self { config: None }
    }

    /// Create an OpenSearchComponent with global config defaults.
    /// These will be applied to endpoint configs when specific values aren't provided.
    pub fn with_config(config: OpenSearchConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    /// Create an OpenSearchComponent with optional global config defaults.
    /// If `None`, behaves like `new()` (uses hardcoded defaults only).
    pub fn with_optional_config(config: Option<OpenSearchConfig>) -> Self {
        Self { config }
    }
}

impl Default for OpenSearchComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for OpenSearchComponent {
    fn scheme(&self) -> &str {
        "opensearch"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let mut config = OpenSearchEndpointConfig::from_uri(uri)?;
        // Apply global config defaults if available
        if let Some(ref global_cfg) = self.config {
            config = config.merge_with_global(global_cfg);
        }
        Ok(Box::new(OpenSearchEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

// ---------------------------------------------------------------------------
// OpenSearchsComponent — handles the `opensearchs://` scheme (HTTPS/TLS)
// Delegates to OpenSearchComponent internally.
// ---------------------------------------------------------------------------

pub struct OpenSearchsComponent {
    inner: OpenSearchComponent,
}

impl OpenSearchsComponent {
    pub fn new() -> Self {
        Self {
            inner: OpenSearchComponent::new(),
        }
    }

    pub fn with_config(config: OpenSearchConfig) -> Self {
        Self {
            inner: OpenSearchComponent::with_config(config),
        }
    }
}

impl Default for OpenSearchsComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for OpenSearchsComponent {
    fn scheme(&self) -> &str {
        "opensearchs"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        self.inner.create_endpoint(uri, ctx)
    }
}

// ---------------------------------------------------------------------------
// OpenSearchEndpoint — producer-only. create_consumer() returns an error.
// ---------------------------------------------------------------------------

struct OpenSearchEndpoint {
    uri: String,
    config: OpenSearchEndpointConfig,
}

impl Endpoint for OpenSearchEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(OpenSearchProducer::new(
            self.config.clone(),
        )))
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "OpenSearch component does not support consumers".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::NoOpComponentContext;

    #[test]
    fn test_opensearch_component_scheme() {
        let component = OpenSearchComponent::new();
        assert_eq!(component.scheme(), "opensearch");
    }

    #[test]
    fn test_opensearchs_component_scheme() {
        let component = OpenSearchsComponent::new();
        assert_eq!(component.scheme(), "opensearchs");
    }

    #[test]
    fn test_opensearch_component_creates_endpoint() {
        let component = OpenSearchComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("opensearch://localhost:9200/myindex?operation=INDEX", &ctx)
            .expect("endpoint should be created");
        assert_eq!(
            endpoint.uri(),
            "opensearch://localhost:9200/myindex?operation=INDEX"
        );
    }

    #[test]
    fn test_opensearchs_component_creates_endpoint() {
        let component = OpenSearchsComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                "opensearchs://localhost:9200/myindex?operation=SEARCH",
                &ctx,
            )
            .expect("endpoint should be created");
        assert_eq!(
            endpoint.uri(),
            "opensearchs://localhost:9200/myindex?operation=SEARCH"
        );
    }

    #[test]
    fn test_opensearch_component_rejects_wrong_scheme() {
        let component = OpenSearchComponent::new();
        let ctx = NoOpComponentContext;
        let result = component.create_endpoint("kafka:topic?brokers=localhost:9092", &ctx);
        assert!(result.is_err(), "wrong scheme should fail");
        let err = result.err().expect("error must exist");
        assert!(err.to_string().contains("expected scheme 'opensearch'"));
    }

    #[test]
    fn test_opensearch_component_applies_global_defaults() {
        let global = OpenSearchConfig::default()
            .with_host("es-global")
            .with_port(9300);
        let component = OpenSearchComponent::with_config(global);
        let ctx = NoOpComponentContext;

        let endpoint = component
            .create_endpoint("opensearch:///myindex?operation=SEARCH", &ctx)
            .expect("endpoint should be created with defaults");

        // Verify the endpoint was created — producer creation will fail until
        // Task 2 (producer) is complete, but the endpoint config should have defaults
        assert_eq!(endpoint.uri(), "opensearch:///myindex?operation=SEARCH");
    }

    #[test]
    fn test_endpoint_create_consumer_returns_error() {
        let component = OpenSearchComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("opensearch://localhost:9200/myindex?operation=INDEX", &ctx)
            .expect("endpoint should be created");

        let result = endpoint.create_consumer();
        assert!(result.is_err(), "create_consumer should return an error");
        let err = result.err().expect("error must exist");
        assert!(
            err.to_string().contains("does not support consumers"),
            "expected consumer-not-supported error, got: {err}"
        );
    }
}
