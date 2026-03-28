//! JavaScript language plugin for Apache Camel Rust.
//!
//! Provides [`JsLanguage`] — a [`Language`](camel_language_api::Language) implementation
//! backed by the [Boa](https://boajs.dev) JavaScript engine.
//!
//! # Usage
//!
//! ```no_run
//! use camel_language_js::JsLanguage;
//! use camel_language_api::Language;
//!
//! let lang = JsLanguage::new();
//! let expr = lang.create_expression("camel.headers.get('foo')").unwrap();
//! ```

pub mod engines;
pub mod error;
pub mod value;

mod bindings;
mod engine;
mod expression;
mod language;

pub use engine::{JsEngine, JsEvalResult, JsExchange};
pub use engines::BoaEngine;
pub use error::JsLanguageError;
pub use language::JsLanguage;
