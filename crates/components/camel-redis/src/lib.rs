pub mod bundle;
pub mod commands;
pub mod config;
pub mod consumer;
pub mod producer;

use camel_component_api::{BoxProcessor, CamelError};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext};

pub use bundle::RedisBundle;
pub use config::{RedisCommand, RedisConfig, RedisEndpointConfig};
pub use consumer::RedisConsumer;
pub use producer::RedisProducer;

pub struct RedisComponent {
    config: Option<RedisConfig>,
}

impl RedisComponent {
    /// Create a new RedisComponent without global config defaults.
    /// Endpoint configs will fall back to hardcoded defaults via `resolve_defaults()`.
    pub fn new() -> Self {
        Self { config: None }
    }

    /// Create a RedisComponent with global config defaults.
    /// These will be applied to endpoint configs before `resolve_defaults()`.
    pub fn with_config(config: RedisConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    /// Create a RedisComponent with optional global config defaults.
    /// If `None`, behaves like `new()` (uses hardcoded defaults only).
    pub fn with_optional_config(config: Option<RedisConfig>) -> Self {
        Self { config }
    }
}

impl Default for RedisComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for RedisComponent {
    fn scheme(&self) -> &str {
        "redis"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let mut config = RedisEndpointConfig::from_uri(uri)?;
        // Apply global config defaults if available
        if let Some(ref global_cfg) = self.config {
            config.apply_defaults(global_cfg);
        }
        // Resolve any remaining None fields to hardcoded defaults
        config.resolve_defaults();
        Ok(Box::new(RedisEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

struct RedisEndpoint {
    uri: String,
    config: RedisEndpointConfig,
}

impl Endpoint for RedisEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(RedisProducer::new(self.config.clone())))
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(RedisConsumer::new(self.config.clone())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::NoOpComponentContext;

    #[test]
    fn test_component_scheme() {
        let component = RedisComponent::new();
        assert_eq!(component.scheme(), "redis");
    }

    #[test]
    fn test_component_creates_endpoint() {
        let component = RedisComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("redis://localhost:6379?command=GET", &ctx)
            .expect("endpoint should be created");
        assert_eq!(endpoint.uri(), "redis://localhost:6379?command=GET");
    }

    #[test]
    fn test_component_rejects_wrong_scheme() {
        let component = RedisComponent::new();
        let ctx = NoOpComponentContext;
        let result = component.create_endpoint("kafka:topic?brokers=localhost:9092", &ctx);
        assert!(result.is_err(), "wrong scheme should fail");
        let err = result.err().expect("error must exist");
        assert!(err.to_string().contains("expected scheme 'redis'"));
    }

    #[test]
    fn test_component_applies_global_defaults() {
        let global = RedisConfig::default()
            .with_host("redis-global")
            .with_port(6380);
        let component = RedisComponent::with_config(global);
        let ctx = NoOpComponentContext;

        let endpoint = component
            .create_endpoint("redis://?command=GET", &ctx)
            .expect("endpoint should be created with defaults");

        let _producer = endpoint
            .create_producer(&ProducerContext::default())
            .expect("producer should be created");
        let _consumer = endpoint
            .create_consumer()
            .expect("consumer should be created");
    }
}
