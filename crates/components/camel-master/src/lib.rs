//! Master/leader-election component — wraps a delegate consumer with distributed lock-based leadership coordination.
//!
//! Main types: `MasterComponent`, `MasterBundle`.
//! Main modules: `bundle`, `config`.

pub mod bundle;
pub mod component;
pub mod config;
pub mod consumer;
pub mod endpoint;
pub mod leadership;
pub mod supervision;

pub use bundle::MasterBundle;
pub use component::MasterComponent;

// Re-exports so the test module's `use super::*;` glob import continues to resolve.
// Production code uses explicit crate::module::* paths.
#[cfg(test)]
pub(crate) use crate::config::MasterUriConfig;
#[cfg(test)]
pub(crate) use consumer::MasterConsumer;

// These imports exist solely for the test module's `use super::*;` glob import.
// Production code uses explicit paths.
#[cfg(test)]
use async_trait::async_trait;
#[cfg(test)]
use camel_api::{CamelError, MetricsCollector};
#[cfg(test)]
use camel_component_api::{
    BoxProcessor, Component, ComponentContext, Consumer, ConsumerContext, Endpoint,
    NetworkRetryPolicy, ProducerContext,
};
#[cfg(test)]
use camel_language_api::Language;
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
mod tests;
