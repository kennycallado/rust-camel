//! Serverless function lifecycle management — WASM and container function runtime for rust-camel.
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
