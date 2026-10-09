//! `RabbitMqComponent`: scheme `rabbitmq` with one connection manager per
//! named broker, created once and cached for the component's lifetime.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, ComponentMetadata, Consumer, Endpoint,
    NetworkRetryPolicy, ProducerContext, RuntimeObservability,
};

use crate::config::{
    RabbitBrokerConfig, RabbitComponentConfig, RabbitEndpointConfig, rabbitmq_reconnect_default,
    resolve_broker_name,
};
use crate::connection::RabbitConnectionManager;
use crate::consumer::RabbitConsumer;
use crate::health::RabbitHealthCheck;
use crate::producer::RabbitProducer;

/// Component serving the `rabbitmq` URI scheme.
pub struct RabbitMqComponent {
    brokers: HashMap<String, RabbitBrokerConfig>,
    retry: NetworkRetryPolicy,
    /// Once-created manager per broker name. `create_endpoint` takes `&self`,
    /// so the cache needs interior synchronization; the critical section is
    /// short and holds no `.await`.
    managers: Mutex<HashMap<String, Arc<RabbitConnectionManager>>>,
    /// Registration-time lifecycle supplier threaded into every manager so
    /// reconnects observe the CURRENT runtime shutdown token. Unbound
    /// (`create_endpoint`'s `ControllerComponentContext` keeps `None`) unless
    /// the bundle installs a slot-bound context via
    /// [`with_lifecycle_context`](Self::with_lifecycle_context).
    lifecycle_context: Option<Arc<dyn ComponentContext>>,
}

impl RabbitMqComponent {
    /// Build from a `[components.rabbitmq]` section.
    pub fn new(config: RabbitComponentConfig) -> Self {
        let retry = config.reconnect.unwrap_or_else(rabbitmq_reconnect_default);
        Self {
            brokers: config.brokers,
            retry,
            managers: Mutex::new(HashMap::new()),
            lifecycle_context: None,
        }
    }

    /// Bind the registration-time lifecycle context.
    ///
    /// The bundle passes a slot-bound `RegistryComponentContext`
    /// (`with_shutdown_slot`) so `shutdown_token()` resolves the current
    /// runtime token per call and a runtime stop cancels pending reconnects.
    /// Never a per-endpoint adapter context: those keep the `None` default to
    /// avoid a stale-across-stop/start lineage.
    pub fn with_lifecycle_context(mut self, context: Arc<dyn ComponentContext>) -> Self {
        self.lifecycle_context = Some(context);
        self
    }

    /// Return the manager for `name`, creating it on first use.
    fn manager_for(&self, name: &str) -> Result<Arc<RabbitConnectionManager>, CamelError> {
        let mut managers = self.managers.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(manager) = managers.get(name) {
            return Ok(Arc::clone(manager));
        }
        let broker = self
            .brokers
            .get(name)
            .ok_or_else(|| CamelError::Config(format!("Unknown RabbitMQ broker '{name}'")))?;
        let mut manager = RabbitConnectionManager::from_broker_config(broker, self.retry.clone());
        if let Some(context) = &self.lifecycle_context {
            manager = manager.with_lifecycle_context(Arc::clone(context));
        }
        let manager = Arc::new(manager);
        managers.insert(name.to_string(), Arc::clone(&manager));
        Ok(manager)
    }
}

impl Component for RabbitMqComponent {
    fn scheme(&self) -> &str {
        "rabbitmq"
    }

    fn metadata(&self) -> ComponentMetadata {
        // The P1 descriptor advertises the producer capability only; task 2.1
        // adds the consumer flag alongside the consumer implementation.
        crate::metadata::RabbitMqMetadataDescriptor::metadata()
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = RabbitEndpointConfig::from_uri(uri)?;
        let broker_name = resolve_broker_name(&self.brokers, config.broker.as_deref())?;
        let manager = self.manager_for(&broker_name)?;
        ctx.register_current_route_health_check(Arc::new(RabbitHealthCheck::new(Arc::clone(
            &manager,
        ))));
        Ok(Box::new(RabbitEndpoint {
            uri: uri.to_string(),
            config,
            manager,
        }))
    }
}

struct RabbitEndpoint {
    uri: String,
    config: RabbitEndpointConfig,
    manager: Arc<RabbitConnectionManager>,
}

impl Endpoint for RabbitEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        // A consumer needs an explicit queue to bind to; `routingKey` alone is
        // a producer-target concept.
        if self.config.queue.is_none() {
            return Err(CamelError::Config(
                "rabbitmq consumer requires the 'queue' parameter".to_string(),
            ));
        }
        Ok(Box::new(RabbitConsumer::new(
            self.config.clone(),
            Arc::clone(&self.manager),
            rt,
        )))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(RabbitProducer::new(
            self.config.clone(),
            Arc::clone(&self.manager),
            rt,
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use camel_component_api::NoOpComponentContext;
    use camel_component_api::test_support::PanicRuntimeObservability;

    use super::*;
    use crate::config::{RabbitBrokerConfig, RabbitComponentConfig};

    fn component() -> RabbitMqComponent {
        let mut brokers = HashMap::new();
        brokers.insert(
            "main".to_string(),
            RabbitBrokerConfig {
                url: "amqp://localhost:5672".to_string(),
                username: None,
                password: None,
                vhost: None,
            },
        );
        RabbitMqComponent::new(RabbitComponentConfig {
            brokers,
            reconnect: None,
        })
    }

    #[test]
    fn create_consumer_without_queue_errors() {
        let component = component();
        let endpoint = component
            .create_endpoint("rabbitmq:ex?routingKey=rk", &NoOpComponentContext)
            .expect("endpoint must resolve");

        let rt: Arc<dyn RuntimeObservability> = Arc::new(PanicRuntimeObservability);
        let error = endpoint
            .create_consumer(rt)
            .err()
            .expect("a consumer without a queue must be rejected");

        assert!(
            error.to_string().contains("queue"),
            "the error must name the missing 'queue' parameter, got: {error}"
        );
    }
}
