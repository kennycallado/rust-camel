//! camel-function — function registry and lifecycle management for Camel Rust.
//!
//! Allows registering named functions (WASM or container-based) that can be invoked
//! from route definitions and expressions. Functions are pooled, health-checked, and
//! hot-reloadable.
//!
//! Main types: `FunctionConfig`, `FunctionRuntimeService`, `RunnerHandle`, `ContainerProvider`.
//! Main modules: `protocol`, `provider`.

pub use camel_api::function::*;

mod config;
mod invoker;
mod pool;
pub mod protocol;
pub mod provider;
mod service;

pub use config::FunctionConfig;
pub use pool::{RunnerHandle, RunnerState};
pub use provider::HealthReport;
pub use provider::container::{ContainerProvider, ContainerProviderBuilder, PullPolicy};
pub use service::FunctionRuntimeService;
