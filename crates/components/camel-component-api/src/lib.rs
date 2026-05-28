//! camel-component-api — traits and types for building Camel components.
//!
//! Provides the core abstractions (`Component`, `Endpoint`, `Consumer`, `Producer`)
//! that every rust-camel component implements. Also re-exports commonly-used types
//! from `camel-api` and `camel-endpoint` for component convenience.
//!
//! Main types: `Component`, `Endpoint`, `Consumer`, `ProducerContext`, `ExchangeEnvelope`, `ComponentBundle`, `PollingConsumer`.
//! Main modules: `component`, `endpoint`, `consumer`, `producer`, `registrar`.

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
pub use endpoint::{Endpoint, PollingConsumer};
pub use producer::ProducerContext;
pub use registrar::ComponentRegistrar;

// Re-export camel-api types for component convenience
pub use camel_api::{
    AsyncHealthCheck, Body, BodyType, BoxProcessor, CamelError, CheckResult, Exchange,
    HealthStatus, Message, RouteAction, RouteStatus, RuntimeCommand, RuntimeCommandBus,
    RuntimeCommandResult, RuntimeHandle, RuntimeQuery, RuntimeQueryBus, RuntimeQueryResult,
    StreamBody, StreamMetadata, Value,
};

// Re-export camel-endpoint types for component convenience
pub use camel_endpoint::{UriComponents, UriConfig, parse_uri};
// Expose a `uri` sub-module so crates using `#[uri_config(crate = "camel_component_api")]`
// can resolve `camel_component_api::uri::parse_bool_param` etc.
pub mod uri {
    /// Parse a boolean URI parameter value (case-insensitive).
    /// Accepts: "true"/"1"/"yes" → true; "false"/"0"/"no" → false.
    pub fn parse_bool_param(s: &str) -> Result<bool, String> {
        match s.to_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            _ => Err(format!("invalid boolean value: '{}'", s)),
        }
    }
}
