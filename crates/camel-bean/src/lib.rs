mod error;
mod processor;
mod registry;

pub use error::BeanError;
pub use processor::BeanProcessor;
pub use registry::BeanRegistry;

// Re-export for macro users
pub use async_trait::async_trait;
pub use camel_bean_macros::{Bean, bean_impl, handler};
