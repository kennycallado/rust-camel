//! JavaScript language plugin for Apache Camel Rust.
//!
//! Provides [`JsLanguage`] — a [`Language`](camel_language_api::Language) implementation
//! backed by the [Boa](https://boajs.dev) JavaScript engine.
//!
//! # Resource Limits
//!
//! Scripts are bounded by configurable limits sourced from `[languages.js.limits]`
//! in `Camel.toml`. When absent, rust-camel runtime defaults apply:
//!
//! | Limit | Default | Source |
//! |---|---|---|
//! | `execution-timeout-ms` | 5,000 | `tokio::time::timeout` + `spawn_blocking` |
//! | `max-loop-iterations` | 100,000 | Boa `RuntimeLimits::set_loop_iteration_limit` |
//! | `max-recursion-depth` | 512 | Boa `RuntimeLimits::set_recursion_limit` |
//! | `max-stack-size` | 10,240 slots | Boa `RuntimeLimits::set_stack_size_limit` |
//!
//! ## Covered threats
//!
//! - Infinite loops (`while (true) {}`) — trip `max-loop-iterations`.
//! - Deep recursion — trips `max-recursion-depth`.
//! - Stack exhaustion — trips `max-stack-size`.
//! - Wall-clock stalls — bounded by `execution-timeout-ms`.
//!
//! ## NOT covered
//!
//! **Heap cap** — `boa_engine` 0.21 does not expose a heap-size limit. A script
//! that allocates many small objects can exhaust process memory before any other
//! limit trips. Route authors writing JS that allocates heavily should validate
//! their scripts carefully; untrusted JS should use `function:` (ADR-0005,
//! out-of-process) instead of `js:`/`javascript:`.
//!
//! ## Timeout caveat
//!
//! Same as Rhai: when `execution-timeout-ms` fires, the route future resolves
//! to an error, but the blocking thread may continue executing until a
//! `RuntimeLimits` bound trips or the script finishes.

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
