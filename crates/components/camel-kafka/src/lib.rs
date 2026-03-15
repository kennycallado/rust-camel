pub mod config;
pub mod consumer;
pub mod manual_commit;
pub mod producer;

pub use config::{KafkaConfig, KafkaEndpointConfig};
pub use consumer::KafkaConsumer;
pub use manual_commit::KafkaManualCommit;
pub use producer::KafkaProducer;

use camel_api::{BoxProcessor, CamelError};
use camel_component::{Component, Consumer, Endpoint, ProducerContext};

pub struct KafkaComponent {
    config: Option<KafkaConfig>,
}

impl KafkaComponent {
    /// Create a new KafkaComponent without global config defaults.
    /// Endpoint configs will fall back to hardcoded defaults via `resolve_defaults()`.
    pub fn new() -> Self {
        Self { config: None }
    }

    /// Create a KafkaComponent with global config defaults.
    /// These will be applied to endpoint configs before `resolve_defaults()`.
    pub fn with_config(config: KafkaConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    /// Create a KafkaComponent with optional global config defaults.
    /// If `None`, behaves like `new()` (uses hardcoded defaults only).
    pub fn with_optional_config(config: Option<KafkaConfig>) -> Self {
        Self { config }
    }
}

impl Default for KafkaComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for KafkaComponent {
    fn scheme(&self) -> &str {
        "kafka"
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let mut config = KafkaEndpointConfig::from_uri(uri)?;
        // Apply global config defaults if available
        if let Some(ref global_cfg) = self.config {
            config.apply_defaults(global_cfg);
        }
        // Resolve any remaining None fields to hardcoded defaults
        config.resolve_defaults();
        Ok(Box::new(KafkaEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

struct KafkaEndpoint {
    uri: String,
    config: KafkaEndpointConfig,
}

impl Endpoint for KafkaEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(KafkaProducer::new(self.config.clone())?))
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(KafkaConsumer::new(self.config.clone())))
    }
}
