pub mod commands;
pub mod config;
pub mod consumer;
pub mod producer;

use camel_api::{BoxProcessor, CamelError};
use camel_component::{Component, Consumer, Endpoint, ProducerContext};

pub use config::{RedisCommand, RedisConfig};
pub use consumer::RedisConsumer;
pub use producer::RedisProducer;

pub struct RedisComponent;

impl RedisComponent {
    pub fn new() -> Self {
        Self
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
        let config = RedisConfig::from_uri(uri)?;
        Ok(Box::new(RedisEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

struct RedisEndpoint {
    uri: String,
    config: RedisConfig,
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
