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
// `Bean` is a non-functional derive stub (emits a compile_error pointing at
// `bean_impl`). Hidden so it does not pollute the public docs surface (rc-x2gy).
#[doc(hidden)]
pub use camel_bean_macros::Bean;
pub use camel_bean_macros::{bean_impl, handler};
