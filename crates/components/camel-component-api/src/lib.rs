pub mod bundle;
pub mod component;
pub mod component_context;
pub mod consumer;
pub mod endpoint;
pub mod producer;
pub mod registrar;

pub use bundle::ComponentBundle;
pub use component::Component;
pub use component_context::{ComponentContext, NoOpComponentContext};
pub use consumer::{ConcurrencyModel, Consumer, ConsumerContext, ExchangeEnvelope};
pub use endpoint::Endpoint;
pub use producer::ProducerContext;
pub use registrar::ComponentRegistrar;

// Re-export camel-api types for component convenience
pub use camel_api::{
    Body, BodyType, BoxProcessor, CamelError, Exchange, Message, RouteAction, RouteStatus,
    RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult, RuntimeHandle, RuntimeQuery,
    RuntimeQueryBus, RuntimeQueryResult, StreamBody, StreamMetadata, Value,
};

// Re-export camel-endpoint types for component convenience
pub use camel_endpoint::{UriComponents, UriConfig, parse_uri};
