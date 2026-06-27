pub mod client_id;
pub mod config;
pub mod consumer;
pub mod endpoint;
pub mod headers;
pub mod producer;
#[cfg(feature = "tls")]
pub mod tls;
pub mod uri;

use std::sync::Arc;

use camel_api::CamelError;
use camel_component_api::{
    BoxProcessor, Component, ComponentBundle, ComponentContext, ComponentRegistrar, Consumer,
    Endpoint, ProducerContext, RuntimeObservability,
};
use config::{MqttBrokerConfig, MqttConfig, MqttEndpointConfig};
use consumer::MqttConsumer;
use producer::MqttProducer;
use uri::parse_mqtt_uri;

// ─── Component ─────────────────────────────────────────────────────────────

pub struct MqttComponent {
    config: MqttConfig,
}

impl MqttComponent {
    pub fn new() -> Self {
        Self {
            config: MqttConfig::default(),
        }
    }

    pub fn with_config(config: MqttConfig) -> Result<Self, CamelError> {
        for (name, broker) in &config.brokers {
            broker
                .validate()
                .map_err(|e| CamelError::Config(format!("mqtt broker '{name}': {e}")))?;
        }
        Ok(Self { config })
    }

    fn resolve_broker(&self, broker_name: &str) -> Result<MqttBrokerConfig, CamelError> {
        self.config
            .brokers
            .get(broker_name)
            .cloned()
            .ok_or_else(|| {
                CamelError::Config(format!(
                    "mqtt: broker '{broker_name}' not found in Camel.toml [components.mqtt.brokers]"
                ))
            })
    }
}

impl Default for MqttComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for MqttComponent {
    fn scheme(&self) -> &str {
        "mqtt"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let ep_config = parse_mqtt_uri(uri)?;
        ep_config.validate()?;
        let broker = self.resolve_broker(&ep_config.broker_name)?;
        Ok(Box::new(MqttEndpoint {
            uri: uri.to_string(),
            config: ep_config,
            broker,
            client_id_prefix: self.config.client_id_prefix.clone(),
            fallback_reconnect: self.config.reconnect.clone(),
        }))
    }
}

// ─── Endpoint ──────────────────────────────────────────────────────────────

pub struct MqttEndpoint {
    uri: String,
    config: MqttEndpointConfig,
    broker: MqttBrokerConfig,
    client_id_prefix: String,
    fallback_reconnect: camel_component_api::NetworkRetryPolicy,
}

impl Endpoint for MqttEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        consumer::validate_for_consumer(&self.config)?;
        Ok(Box::new(MqttConsumer::new(
            self.config.clone(),
            rt,
            self.broker.clone(),
            self.client_id_prefix.clone(),
            self.fallback_reconnect.clone(),
        )))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let producer = MqttProducer::new(
            self.config.clone(),
            self.broker.clone(),
            &self.uri,
            &self.client_id_prefix,
            ctx.route_id(),
            self.fallback_reconnect.clone(),
        )?;
        Ok(BoxProcessor::new(producer))
    }
}

// ─── Bundle ────────────────────────────────────────────────────────────────

pub struct MqttBundle {
    config: MqttConfig,
}

impl ComponentBundle for MqttBundle {
    fn config_key() -> &'static str {
        "mqtt"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: MqttConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self { config })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        match MqttComponent::with_config(self.config) {
            Ok(comp) => ctx.register_component_dyn(Arc::new(comp)),
            Err(e) => {
                // log-policy: system-broken
                tracing::error!("MqttComponent registration failed: {e}");
            }
        }
    }
}
