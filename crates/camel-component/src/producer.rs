//! Producer context for dependency injection.
//!
//! Re-exports [`ProducerContext`] from `camel-api` for convenience.
//! This type lives in `camel-api` to avoid cyclic dependencies between
//! `camel-core` and `camel-component`.

pub use camel_api::ProducerContext;
