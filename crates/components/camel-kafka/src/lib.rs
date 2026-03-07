pub mod config;
pub mod consumer;
pub mod producer;

pub use config::KafkaConfig;
pub use consumer::KafkaConsumer;
pub use producer::KafkaProducer;

use camel_api::{BoxProcessor, CamelError};
use camel_component::{Component, Consumer, Endpoint, ProducerContext};

pub struct KafkaComponent;

impl KafkaComponent {
    pub fn new() -> Self {
        Self
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
        let config = KafkaConfig::from_uri(uri)?;
        Ok(Box::new(KafkaEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

struct KafkaEndpoint {
    uri: String,
    config: KafkaConfig,
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
