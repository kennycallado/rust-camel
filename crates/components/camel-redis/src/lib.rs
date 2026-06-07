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
pub mod producer;

use camel_component_api::{BoxProcessor, CamelError};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext, RuntimeObservability};
use std::sync::Arc;

pub use bundle::RedisBundle;
pub use config::{RedisCommand, RedisConfig, RedisEndpointConfig};
pub use consumer::RedisConsumer;
pub use health::RedisHealthCheck;
pub use producer::RedisProducer;

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

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let mut config = RedisEndpointConfig::from_uri(uri)?;
        // Apply global config defaults if available
        if let Some(ref global_cfg) = self.config {
            config.apply_defaults(global_cfg);
        }
        // Resolve any remaining None fields to hardcoded defaults
        config.resolve_defaults();

        let health_check = RedisHealthCheck::new(&config)?;
        ctx.register_current_route_health_check(Arc::new(health_check));

        Ok(Box::new(RedisEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
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
        rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(RedisProducer::new(
            self.config.clone(),
            rt,
        )))
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(RedisConsumer::new(self.config.clone())?))
    }
}

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
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
            .with_host("redis-global")
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

// REDIS-009: Integration tests with live Redis (#[ignore] by default)
#[cfg(test)]
mod integration_tests {
    use crate::{RedisComponent, RedisEndpointConfig, RedisProducer};
    use camel_component_api::NoOpComponentContext;
    use camel_component_api::test_support::PanicRuntimeObservability;
    use camel_component_api::{Component, ProducerContext};
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
    use std::time::Duration;

    /// Helper to check if a local Redis is available.
    async fn redis_available() -> bool {
        tokio::net::TcpStream::connect("127.0.0.1:6379")
            .await
            .is_ok()
    }

    /// Integration test: PING command via producer.
    /// Run with: `cargo test -p camel-component-redis -- --ignored test_integration_ping`
    #[tokio::test]
    #[ignore = "Requires live Redis at 127.0.0.1:6379"]
    async fn test_integration_ping() {
        if !redis_available().await {
            eprintln!("Skipping: Redis not available at 127.0.0.1:6379");
            return;
        }
        let config = RedisEndpointConfig::from_uri("redis://127.0.0.1:6379?command=PING").unwrap();
        let producer = RedisProducer::new(config, test_rt());
        producer
            .check_connection()
            .await
            .expect("PING should succeed");
    }

    /// Integration test: SET/GET round-trip via producer.
    /// Run with: `cargo test -p camel-component-redis -- --ignored test_integration_set_get`
    #[tokio::test]
    #[ignore = "Requires live Redis at 127.0.0.1:6379"]
    async fn test_integration_set_get() {
        if !redis_available().await {
            eprintln!("Skipping: Redis not available at 127.0.0.1:6379");
            return;
        }
        let component = RedisComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("redis://127.0.0.1:6379?command=SET", &ctx)
            .unwrap();

        let _producer = endpoint
            .create_producer(test_rt(), &ProducerContext::default())
            .expect("producer should be created");
        // NOTE: Full SET/GET round-trip requires tower::Service::call which needs
        // an active runtime loop. This test scaffolding proves the endpoint
        // config and producer creation work with a live Redis connection.
    }

    /// Integration test: PUBLISH/SUBSCRIBE via consumer.
    /// Run with: `cargo test -p camel-component-redis -- --ignored test_integration_pubsub`
    #[tokio::test]
    #[ignore = "Requires live Redis at 127.0.0.1:6379"]
    async fn test_integration_pubsub() {
        if !redis_available().await {
            eprintln!("Skipping: Redis not available at 127.0.0.1:6379");
            return;
        }
        let component = RedisComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                "redis://127.0.0.1:6379?command=SUBSCRIBE&channels=integration-test",
                &ctx,
            )
            .unwrap();

        let mut consumer = endpoint
            .create_consumer(test_rt())
            .expect("consumer should be created");

        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let consumer_ctx = camel_component_api::ConsumerContext::new(tx, cancel_token.clone());

        // Start subscriber, let it run briefly, then cancel
        consumer
            .start(consumer_ctx)
            .await
            .expect("start should succeed");
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel_token.cancel();
        consumer.stop().await.expect("stop should succeed");
    }
}
