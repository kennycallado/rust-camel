//! Redis-backed service repositories for rust-camel.
//!
//! This crate provides Redis implementations of the `CacheRepository` and
//! `IdempotentRepository` abstractions from `camel-api`, reusing the
//! connection and topology management of `camel-component-redis`.
//!
//! Sentinel deployments are supported unconditionally (no feature flag);
//! TLS is opt-in via the `tls` feature.
//!
//! Endpoint configuration types are re-exported so downstream crates do not
//! need to depend on `camel-component-redis` directly.

pub(crate) mod cache_repo;
pub(crate) mod connection;
pub mod error;
pub(crate) mod executor;
pub(crate) mod idempotent_repo;
pub mod keyspace;

pub use cache_repo::RedisCacheRepository;
// Re-export for internal use only: the executor and idempotent repository
// consume it via `crate::is_transient_redis_error`; it is not part of the
// crate's public surface.
pub(crate) use camel_component_redis::config::is_transient_redis_error;
pub use camel_component_redis::{RedisEndpointConfig, SentinelConfig, TopologyKind};
pub(crate) use error::to_camel_error;
pub use idempotent_repo::RedisIdempotentRepository;
pub(crate) use keyspace::namespaced;
pub(crate) use keyspace::validate_namespace_token;
