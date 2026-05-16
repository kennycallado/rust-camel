//! Kafka component for rust-camel — Apache Camel–inspired integration with Apache Kafka.
//!
//! Provides producer and consumer endpoints backed by `rdkafka`, with Tower-native
//! async pipelines, EIP patterns, and full security (SASL, SSL) support.
//!
//! # Migration from v0.x (Breaking API Change)
//!
//! `KafkaProducer::new`, `KafkaConsumer::new`, and `apply_security_config` now require
//! a `ResolvedKafkaEndpointConfig` instead of `KafkaEndpointConfig`. This eliminates
//! production panics from missing config fields by enforcing resolution at compile time.
//!
//! **Before:**
//! ```ignore
//! let config = KafkaEndpointConfig::from_uri("kafka:topic?brokers=localhost:9092&groupId=g")?;
//! let producer = KafkaProducer::new(config)?;  // could panic on missing fields
//! ```
//!
//! **After:**
//! ```ignore
//! let config = KafkaEndpointConfig::from_uri("kafka:topic?brokers=localhost:9092&groupId=g")?;
//! let resolved = config.resolve()?;  // applies defaults, validates, returns ResolvedKafkaEndpointConfig
//! let producer = KafkaProducer::new(resolved)?;  // guaranteed safe
//! ```
//!
//! When using the `KafkaComponent` trait implementation, resolution happens automatically
//! in `create_endpoint`, so no changes are needed for component-based usage.

pub mod bundle;
pub mod config;
pub mod consumer;
pub mod manual_commit;
pub mod producer;

pub use bundle::KafkaBundle;
pub use config::{KafkaConfig, KafkaEndpointConfig, ResolvedKafkaEndpointConfig};
pub use consumer::KafkaConsumer;
pub use manual_commit::KafkaManualCommit;
pub use producer::KafkaProducer;

use camel_component_api::{BoxProcessor, CamelError};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext};

pub struct KafkaComponent {
    config: Option<KafkaConfig>,
}

impl KafkaComponent {
    /// Create a new KafkaComponent without global config defaults.
    /// Endpoint configs will fall back to hardcoded defaults via `resolve_defaults()`.
    pub fn new() -> Self {
        Self { config: None }
    }

    /// Create a KafkaComponent with global config defaults.
    /// These will be applied to endpoint configs before `resolve_defaults()`.
    pub fn with_config(config: KafkaConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    /// Create a KafkaComponent with optional global config defaults.
    /// If `None`, behaves like `new()` (uses hardcoded defaults only).
    pub fn with_optional_config(config: Option<KafkaConfig>) -> Self {
        Self { config }
    }
}

impl Default for KafkaComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for KafkaComponent {
    fn scheme(&self) -> &str {
        "kafka"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let mut config = KafkaEndpointConfig::from_uri(uri)?;
        // Apply global config defaults if available
        if let Some(ref global_cfg) = self.config {
            config.apply_defaults(global_cfg);
        }
        // Resolve all fields and validate — returns ResolvedKafkaEndpointConfig
        let resolved = config.resolve()?;
        Ok(Box::new(KafkaEndpoint {
            uri: uri.to_string(),
            config: resolved,
        }))
    }
}

struct KafkaEndpoint {
    uri: String,
    config: ResolvedKafkaEndpointConfig,
}

impl Endpoint for KafkaEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(KafkaProducer::new(self.config.clone())?))
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(KafkaConsumer::new(self.config.clone())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::NoOpComponentContext;

    #[test]
    fn test_component_scheme() {
        let component = KafkaComponent::new();
        assert_eq!(component.scheme(), "kafka");
    }

    #[test]
    fn test_component_creates_endpoint_with_defaults() {
        let component = KafkaComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                "kafka:orders?brokers=localhost:9092&groupId=test-group",
                &ctx,
            )
            .expect("endpoint should be created");
        assert_eq!(
            endpoint.uri(),
            "kafka:orders?brokers=localhost:9092&groupId=test-group"
        );
    }

    #[test]
    fn test_component_rejects_wrong_scheme() {
        let component = KafkaComponent::new();
        let ctx = NoOpComponentContext;
        let result =
            component.create_endpoint("sql:select 1?db_url=postgres://localhost/test", &ctx);
        assert!(result.is_err(), "wrong scheme should fail");
        let err = result.err().expect("error must exist");
        assert!(err.to_string().contains("expected scheme 'kafka'"));
    }

    #[test]
    fn test_component_rejects_empty_brokers() {
        let component = KafkaComponent::new();
        let ctx = NoOpComponentContext;
        // Empty brokers string should be rejected at resolve time
        let result = component.create_endpoint("kafka:orders?brokers=", &ctx);
        assert!(result.is_err(), "empty brokers should fail");
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(
                msg.contains("brokers"),
                "error should mention brokers: {msg}"
            );
        }
    }

    #[test]
    fn test_component_rejects_empty_group_id() {
        let component = KafkaComponent::new();
        let ctx = NoOpComponentContext;
        let result =
            component.create_endpoint("kafka:orders?brokers=localhost:9092&groupId=", &ctx);
        assert!(result.is_err(), "empty group_id should fail");
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(
                msg.contains("group_id"),
                "error should mention group_id: {msg}"
            );
        }
    }

    #[test]
    fn test_component_applies_global_defaults_when_missing_in_uri() {
        let global = KafkaConfig::default()
            .with_brokers("broker-1:9092")
            .with_group_id("global-group");
        let component = KafkaComponent::with_config(global);
        let ctx = NoOpComponentContext;

        let endpoint = component
            .create_endpoint("kafka:orders", &ctx)
            .expect("endpoint should be created with global defaults");

        let producer = endpoint
            .create_producer(&ProducerContext::default())
            .expect("producer should be created from endpoint");
        drop(producer);
    }
}
