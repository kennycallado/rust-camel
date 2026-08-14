//! Redis component for rust-camel.
//!
//! Provides producer and consumer implementations for Redis, supporting:
//! - String, Hash, List, Set, Sorted Set operations
//! - Pub/Sub (SUBSCRIBE, PSUBSCRIBE, PUBLISH)
//! - Queue operations (BLPOP, BRPOP)
//! - Key management (EXPIRE, TTL, DEL, etc.)
//!
//! # Breaking Changes in v0.10.0
//!
//! - **`RedisConsumer::new()`** now takes `RedisEndpointConfig` directly instead of
//!   separate `(config, mode)` parameters. The mode is inferred from the command type
//!   in the config (SUBSCRIBE/PSUBSCRIBE → PubSub, BLPOP/BRPOP → Queue).
//! - **`resolve_zstore_keys()`** signature changed to accept `&[String]` instead of
//!   a single `&str` for ZUNIONSTORE/ZINTERSTORE key resolution.
//! - Invalid consumer commands (e.g. SET, GET) now return an error instead of silently
//!   falling back to BLPOP (REDIS-003).
//!
//! # Example
//!
//! ```no_run
//! use camel_component_redis::{RedisComponent, RedisEndpointConfig};
//!
//! let config = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET").unwrap();
//! let component = RedisComponent::new();
//! ```

pub mod bundle;
pub mod commands;
pub mod config;
pub mod consumer;
pub mod executor;
pub mod health;
pub(crate) mod metadata;
pub mod producer;
pub(crate) mod pubsub;
pub(crate) mod queue;
pub(crate) mod retry;
pub mod sentinel_component;
pub mod sentinel_config;
pub mod topology;

use camel_component_api::{BoxProcessor, CamelError, ComponentMetadata};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext, RuntimeObservability};
use metadata::RedisMetadataDescriptor;
use std::sync::Arc;

pub use bundle::RedisBundle;
pub use config::{RedisCommand, RedisConfig, RedisEndpointConfig};
pub use consumer::RedisConsumer;
pub use executor::MultiplexedExecutor;
pub use health::RedisHealthCheck;
pub use producer::RedisProducer;
pub use sentinel_component::{RedisSentinelComponent, RedissSentinelComponent};
pub use sentinel_config::{SentinelConfig, TopologyKind};
#[cfg(test)]
pub use topology::FakeTopology;
pub use topology::{RedisTopology, ServerKind, StandaloneTopology};

pub struct RedisComponent {
    config: Option<RedisConfig>,
}

impl RedisComponent {
    /// Create a new RedisComponent without global config defaults.
    /// Endpoint configs will fall back to hardcoded defaults via `resolve_defaults()`.
    pub fn new() -> Self {
        Self { config: None }
    }

    /// Create a RedisComponent with global config defaults.
    /// These will be applied to endpoint configs before `resolve_defaults()`.
    pub fn with_config(config: RedisConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    /// Create a RedisComponent with optional global config defaults.
    /// If `None`, behaves like `new()` (uses hardcoded defaults only).
    pub fn with_optional_config(config: Option<RedisConfig>) -> Self {
        Self { config }
    }
}

impl Default for RedisComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for RedisComponent {
    fn scheme(&self) -> &str {
        "redis"
    }

    fn metadata(&self) -> ComponentMetadata {
        RedisMetadataDescriptor::metadata()
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        create_redis_endpoint(uri, self.config.as_ref(), ctx)
    }
}

/// Shared endpoint-creation logic for every redis-scheme component
/// (`redis`, `redis-sentinel`, `rediss-sentinel`).
///
/// The config parser derives the topology from the URI scheme; global config
/// defaults (including the `[components.redis.sentinel]` block) are applied
/// when provided. Fail-closed checks run here so all schemes behave
/// identically.
pub(crate) fn create_redis_endpoint(
    uri: &str,
    global_config: Option<&RedisConfig>,
    ctx: &dyn camel_component_api::ComponentContext,
) -> Result<Box<dyn Endpoint>, CamelError> {
    let mut config = RedisEndpointConfig::from_uri(uri)?;
    // Apply global config defaults if available
    if let Some(global_cfg) = global_config {
        config.apply_defaults(global_cfg);
        // Apply sentinel topology from the TOML block (fail-closed, ADR-0033).
        config.apply_topology_defaults(global_cfg)?;
    }
    // Resolve any remaining None fields to hardcoded defaults
    config.resolve_defaults();

    // Fail-closed when a sentinel config is present but the feature is off
    if cfg!(not(feature = "sentinel")) && matches!(config.topology_kind, TopologyKind::Sentinel(_))
    {
        return Err(CamelError::Config(
            "redis-sentinel requires the 'sentinel' cargo feature".into(),
        ));
    }

    // Validate topology (mutual exclusion, non-empty master_name/nodes)
    #[cfg(feature = "cluster")]
    let cluster_nodes_present = cluster_nodes_in_global_config(global_config);
    #[cfg(not(feature = "cluster"))]
    let cluster_nodes_present = false;
    crate::sentinel_config::validate_topology(&config.topology_kind, cluster_nodes_present)?;

    let health_check = RedisHealthCheck::new(&config)?;
    ctx.register_current_route_health_check(Arc::new(health_check));

    Ok(Box::new(RedisEndpoint {
        uri: uri.to_string(),
        config,
    }))
}

#[cfg(feature = "cluster")]
fn cluster_nodes_in_global_config(global_config: Option<&RedisConfig>) -> bool {
    global_config
        .map(|c| !c.cluster_nodes.is_empty())
        .unwrap_or(false)
}

pub struct RedisEndpoint {
    uri: String,
    config: RedisEndpointConfig,
}

impl Endpoint for RedisEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(RedisProducer::new(self.config.clone())?))
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(RedisConsumer::new(self.config.clone(), rt)?))
    }
}

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

    use super::*;
    use camel_component_api::NoOpComponentContext;

    #[test]
    fn test_component_scheme() {
        let component = RedisComponent::new();
        assert_eq!(component.scheme(), "redis");
    }

    #[test]
    fn test_component_creates_endpoint() {
        let component = RedisComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("redis://localhost:6379?command=GET", &ctx)
            .expect("endpoint should be created");
        assert_eq!(endpoint.uri(), "redis://localhost:6379?command=GET");
    }

    #[test]
    fn test_component_rejects_wrong_scheme() {
        let component = RedisComponent::new();
        let ctx = NoOpComponentContext;
        let result = component.create_endpoint("kafka:topic?brokers=localhost:9092", &ctx);
        assert!(result.is_err(), "wrong scheme should fail");
        let err = result.err().expect("error must exist");
        assert!(err.to_string().contains("expected scheme 'redis'"));
    }

    #[test]
    fn test_component_applies_global_defaults() {
        let global = RedisConfig::default()
            .with_host("localhost")
            .with_port(6380);
        let component = RedisComponent::with_config(global);
        let ctx = NoOpComponentContext;

        let endpoint = component
            .create_endpoint("redis://?command=GET", &ctx)
            .expect("endpoint should be created with defaults");

        let _producer = endpoint
            .create_producer(rt(), &ProducerContext::default())
            .expect("producer should be created");
        // GET is not a valid consumer command (REDIS-003), so use BLPOP for consumer test
        let endpoint2 = component
            .create_endpoint("redis://?command=BLPOP&key=test", &ctx)
            .expect("endpoint should be created");
        let _consumer = endpoint2
            .create_consumer(rt())
            .expect("consumer should be created");
    }

    // REDIS-011: RedisEndpoint is now pub
    #[test]
    fn test_redis_endpoint_is_pub_accessible() {
        let component = RedisComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("redis://localhost:6379?command=GET", &ctx)
            .unwrap();
        // Verify we can access the endpoint's URI
        assert_eq!(endpoint.uri(), "redis://localhost:6379?command=GET");
    }
}

// Integration tests for the Redis component live in `crates/camel-test/tests/redis_test.rs`
// (testcontainers-backed). The `mod integration_tests` block that used to live here — three
// `#[ignore]` tests against `127.0.0.1:6379` — was a redundant duplicate of that coverage and
// has been removed.
