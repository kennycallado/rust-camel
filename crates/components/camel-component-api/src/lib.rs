pub mod component;
pub mod consumer;
pub mod endpoint;
pub mod producer;

pub use component::Component;
pub use consumer::{ConcurrencyModel, Consumer, ConsumerContext, ExchangeEnvelope};
pub use endpoint::Endpoint;
pub use producer::ProducerContext;

// Re-export camel-api types for component convenience
pub use camel_api::{Body, BodyType, BoxProcessor, CamelError, Exchange, Message, Value};

// Re-export camel-endpoint types for component convenience
pub use camel_endpoint::{UriComponents, UriConfig, parse_uri};
