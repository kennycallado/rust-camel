pub mod commands;
pub mod config;
pub mod consumer;
pub mod producer;

use camel_api::{BoxProcessor, CamelError};
use camel_component::{Component, Consumer, Endpoint, ProducerContext};

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

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
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
