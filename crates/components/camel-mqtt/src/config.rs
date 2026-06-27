// MQTT component configuration: broker endpoints, credentials, TLS settings.

use std::collections::HashMap;

use camel_api::CamelError;
use camel_component_api::NetworkRetryPolicy;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AckMode {
    #[default]
    Auto,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Copy, Default)]
pub enum QosLevel {
    AtMostOnce = 0,
    #[default]
    AtLeastOnce = 1,
    ExactlyOnce = 2,
}

impl QosLevel {
    pub fn to_rumqttc(&self) -> rumqttc::QoS {
        match self {
            QosLevel::AtMostOnce => rumqttc::QoS::AtMostOnce,
            QosLevel::AtLeastOnce => rumqttc::QoS::AtLeastOnce,
            QosLevel::ExactlyOnce => rumqttc::QoS::ExactlyOnce,
        }
    }
}

// NOTE: Debug is implemented manually below (redacts password), so it is NOT derived here.
#[derive(Clone, Deserialize)]
pub struct MqttBrokerConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub tls_ca_cert: Option<String>,
}

impl MqttBrokerConfig {
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.url.trim().is_empty() {
            return Err(CamelError::Config(
                "MqttBrokerConfig.url must not be empty".into(),
            ));
        }
        if !self.url.starts_with("mqtt://") && !self.url.starts_with("mqtts://") {
            return Err(CamelError::Config(format!(
                "MqttBrokerConfig.url must start with mqtt:// or mqtts://, got: {}",
                self.url
            )));
        }
        Ok(())
    }

    pub fn is_tls(&self) -> bool {
        self.url.starts_with("mqtts://")
    }

    /// Parse host and port from url, e.g. "mqtt://broker.example.com:1883".
    /// If no port is given, defaults to 1883 (mqtt://) or 8883 (mqtts://).
    pub fn host_port(&self) -> Result<(String, u16), CamelError> {
        let without_scheme = self
            .url
            .trim_start_matches("mqtt://")
            .trim_start_matches("mqtts://");
        let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
        let (host, port) = match authority.split_once(':') {
            Some((h, p)) => {
                let port = p.parse::<u16>().map_err(|_| {
                    CamelError::Config(format!("MqttBrokerConfig.url invalid port: {}", p))
                })?;
                (h.to_string(), port)
            }
            None => {
                let default_port = if self.is_tls() { 8883 } else { 1883 };
                (authority.to_string(), default_port)
            }
        };
        if host.is_empty() {
            return Err(CamelError::Config(format!(
                "MqttBrokerConfig.url missing host: {}",
                self.url
            )));
        }
        Ok((host, port))
    }
}

impl std::fmt::Debug for MqttBrokerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MqttBrokerConfig")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("tls_ca_cert", &self.tls_ca_cert)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MqttConfig {
    pub brokers: HashMap<String, MqttBrokerConfig>,
    pub client_id_prefix: String,
    pub reconnect: NetworkRetryPolicy,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            brokers: HashMap::new(),
            client_id_prefix: "camel".to_string(),
            reconnect: NetworkRetryPolicy::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MqttEndpointConfig {
    pub broker_name: String,
    pub subscriptions: Vec<String>,
    pub publish_topic: Option<String>,
    pub qos: QosLevel,
    pub ack_mode: AckMode,
    pub clean_session: bool,
    pub retain: bool,
    pub keep_alive_secs: u64,
    pub max_payload_bytes: usize,
    pub client_id_override: Option<String>,
    /// Per-endpoint override for the reconnect backoff policy.
    /// When `None`, the component-level `MqttConfig.reconnect` is used.
    pub reconnect: Option<NetworkRetryPolicy>,
}

impl Default for MqttEndpointConfig {
    fn default() -> Self {
        Self {
            broker_name: String::new(),
            subscriptions: vec![],
            publish_topic: None,
            qos: QosLevel::AtLeastOnce,
            ack_mode: AckMode::Auto,
            clean_session: true,
            retain: false,
            keep_alive_secs: 60,
            max_payload_bytes: 256 * 1024,
            client_id_override: None,
            reconnect: None,
        }
    }
}

impl MqttEndpointConfig {
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.broker_name.trim().is_empty() {
            return Err(CamelError::Config(
                "mqtt: broker_name must not be empty".into(),
            ));
        }
        if self.ack_mode == AckMode::Manual
            && self.qos != QosLevel::AtMostOnce
            && self.clean_session
        {
            return Err(CamelError::Config(
                "mqtt: ackMode=manual with QoS 1/2 requires cleanSession=false \
                 (with cleanSession=true, broker may discard unacked messages on reconnect)"
                    .into(),
            ));
        }
        if self.keep_alive_secs > u64::from(u16::MAX) {
            return Err(CamelError::Config(format!(
                "mqtt: keepAliveSecs must be <= 65535, got {}",
                self.keep_alive_secs
            )));
        }
        Ok(())
    }

    /// Resolve the effective reconnect policy, preferring the endpoint override.
    pub fn effective_reconnect(&self, fallback: &NetworkRetryPolicy) -> NetworkRetryPolicy {
        self.reconnect.clone().unwrap_or_else(|| fallback.clone())
    }
}

/// Enforce a payload size limit. Returns `StreamLimitExceeded(max)` when `len > max`.
pub(crate) fn check_payload_len(len: usize, max: usize) -> Result<(), CamelError> {
    if len > max {
        Err(CamelError::StreamLimitExceeded(max))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mqtt_config_defaults() {
        let config = MqttConfig::default();
        assert_eq!(config.client_id_prefix, "camel");
        assert!(config.brokers.is_empty());
        assert_eq!(
            config.reconnect.max_delay,
            NetworkRetryPolicy::default().max_delay
        );
    }

    #[test]
    fn broker_config_requires_url() {
        let broker = MqttBrokerConfig {
            url: "mqtt://localhost:1883".to_string(),
            username: None,
            password: None,
            tls_ca_cert: None,
        };
        assert!(broker.validate().is_ok());
    }

    #[test]
    fn broker_config_rejects_empty_url() {
        let broker = MqttBrokerConfig {
            url: "".to_string(),
            username: None,
            password: None,
            tls_ca_cert: None,
        };
        assert!(broker.validate().is_err());
    }

    #[test]
    fn broker_config_defaults_port_when_missing() {
        let broker = MqttBrokerConfig {
            url: "mqtt://localhost".to_string(),
            username: None,
            password: None,
            tls_ca_cert: None,
        };
        assert_eq!(broker.host_port().unwrap(), ("localhost".to_string(), 1883));
        let tls_broker = MqttBrokerConfig {
            url: "mqtts://localhost".to_string(),
            username: None,
            password: None,
            tls_ca_cert: None,
        };
        assert_eq!(
            tls_broker.host_port().unwrap(),
            ("localhost".to_string(), 8883)
        );
    }

    #[test]
    fn broker_config_rejects_invalid_port() {
        let broker = MqttBrokerConfig {
            url: "mqtt://localhost:notaport".to_string(),
            username: None,
            password: None,
            tls_ca_cert: None,
        };
        assert!(broker.host_port().is_err());
    }

    #[test]
    fn broker_config_rejects_missing_host() {
        let broker = MqttBrokerConfig {
            url: "mqtt://:1883".to_string(),
            username: None,
            password: None,
            tls_ca_cert: None,
        };
        assert!(broker.host_port().is_err());
    }

    #[test]
    fn endpoint_config_rejects_manual_ack_qos1_clean_session() {
        let mut cfg = MqttEndpointConfig::default();
        cfg.broker_name = "primary".to_string();
        cfg.ack_mode = AckMode::Manual;
        cfg.qos = QosLevel::AtLeastOnce;
        cfg.clean_session = true;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn endpoint_config_allows_manual_ack_qos1_no_clean_session() {
        let mut cfg = MqttEndpointConfig::default();
        cfg.broker_name = "primary".to_string();
        cfg.ack_mode = AckMode::Manual;
        cfg.qos = QosLevel::AtLeastOnce;
        cfg.clean_session = false;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn endpoint_config_rejects_keep_alive_exceeds_u16() {
        let mut cfg = MqttEndpointConfig::default();
        cfg.broker_name = "primary".to_string();
        cfg.keep_alive_secs = 100_000;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("keepAliveSecs must be <= 65535"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn endpoint_config_accepts_normal_keep_alive_range() {
        let mut cfg = MqttEndpointConfig::default();
        cfg.broker_name = "primary".to_string();
        cfg.keep_alive_secs = 3600;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn broker_config_debug_redacts_password_but_shows_ca_cert_path() {
        let broker = MqttBrokerConfig {
            url: "mqtt://localhost:1883".to_string(),
            username: Some("u".to_string()),
            password: Some("secret".to_string()),
            tls_ca_cert: Some("/etc/ssl/ca.pem".to_string()),
        };
        let s = format!("{broker:?}");
        assert!(!s.contains("secret"), "password leaked: {s}");
        assert!(s.contains("/etc/ssl/ca.pem"), "ca_cert path hidden: {s}");
    }
}
