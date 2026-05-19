//! Bean component for rust-camel — register and invoke named beans as route processors.
//!
//! Main types: `BeanRegistry`, `BeanProcessor`. Main modules: `error`, `processor`, `registry`.

mod error;
mod processor;
mod registry;

pub use error::BeanError;
pub use processor::BeanProcessor;
pub use registry::BeanRegistry;

// Re-export for macro users
pub use async_trait::async_trait;
pub use camel_bean_macros::{Bean, bean_impl, handler};
