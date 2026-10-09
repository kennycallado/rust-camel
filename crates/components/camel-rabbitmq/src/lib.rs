//! RabbitMQ component for rust-camel — AMQP 0-9-1 messaging.
//!
//! Credentials never reach logs or error messages in the clear: broker
//! passwords render as `<redacted>` and URL userinfo is masked through the
//! canonical [`camel_api::redact::redact_url`] helper (ADR-0076).

pub mod bundle;
pub mod component;
pub mod config;
pub mod connection;
pub mod consumer;
pub mod error;
pub(crate) mod headers;
pub mod health;
pub(crate) mod metadata;
pub(crate) mod producer;
pub(crate) mod reply;
pub(crate) mod topology;

pub use bundle::RabbitMqBundle;
pub use component::RabbitMqComponent;
pub use connection::{
    ConnStatus, ConnectFn, ConnectFuture, PUBLISH_DISCONNECTED_BOUND, RabbitConnectionManager,
};
pub use consumer::RabbitConsumer;
pub use error::RabbitError;
pub use health::RabbitHealthCheck;
