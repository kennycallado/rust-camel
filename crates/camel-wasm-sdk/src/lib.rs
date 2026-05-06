//! # camel-wasm-sdk
//!
//! WASM SDK for building Camel guest plugins. Provides types and utilities
//! to write processors that run inside the Camel WASM runtime.
//!
//! ## Quick Start
//!
//! ```ignore
//! use camel_wasm_sdk::{export, Plugin, WasmExchange, WasmBody, WasmError};
//!
//! struct MyProcessor;
//!
//! impl Plugin for MyProcessor {
//!     fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
//!         let transformed = match &exchange.input.body {
//!             WasmBody::Text(s) => WasmBody::Text(s.to_uppercase()),
//!             other => other.clone(),
//!         };
//!         exchange.input.body = transformed;
//!         Ok(exchange)
//!     }
//! }
//!
//! export!(MyProcessor);
//! ```
//!
//! ## Plugin with Init Hook
//!
//! ```ignore
//! use camel_wasm_sdk::{export, Plugin, WasmExchange, WasmError};
//!
//! struct MyProcessor;
//!
//! impl Plugin for MyProcessor {
//!     fn init() -> Result<(), String> {
//!         // one-time setup
//!         Ok(())
//!     }
//!
//!     fn process(exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
//!         Ok(exchange)
//!     }
//! }
//!
//! export!(MyProcessor);
//! ```
//!
//! ## Plugin Example with State
//!
//! ```ignore
//! use camel_wasm_sdk::{export, Plugin, GuestState, WasmExchange, WasmBody, WasmError};
//!
//! struct Config {
//!     prefix: String,
//! }
//!
//! static STATE: GuestState<Config> = GuestState::new();
//!
//! struct StatefulProcessor;
//!
//! impl Plugin for StatefulProcessor {
//!     fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
//!         let config = STATE.get_or_init(|| Config {
//!             prefix: "[camel] ".to_string(),
//!         });
//!         let transformed = match &exchange.input.body {
//!             WasmBody::Text(s) => WasmBody::Text(format!("{}{}", config.prefix, s)),
//!             other => other.clone(),
//!         };
//!         exchange.input.body = transformed;
//!         Ok(exchange)
//!     }
//! }
//!
//! export!(StatefulProcessor);
//! ```

#[allow(clippy::too_many_arguments)]
mod bindings {
    wit_bindgen::generate!({
        world: "plugin",
        path: "../../wit",
        pub_export_macro: true,
    });
}

mod plugin;
mod state_helpers;
mod types_ext;

pub use bindings::Guest;
pub use bindings::camel::plugin::host;
pub use bindings::camel::plugin::types::{
    WasmBody, WasmError, WasmExchange, WasmMessage, WasmPattern,
};
pub use bindings::export;
pub use plugin::Plugin;
pub use state::GuestState;
pub use state_helpers::{load, load_json, store, store_json};

pub mod state;
