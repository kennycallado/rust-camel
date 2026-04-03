use std::path::PathBuf;

use crate::{BrokerType, BRIDGE_VERSION};

pub fn default_bridge_cache_dir() -> PathBuf {
    camel_bridge::download::default_cache_dir()
}

#[derive(Debug, Clone, PartialEq)]
pub enum DestinationType {
    Queue,
    Topic,
}

#[derive(Debug, Clone)]
pub struct JmsEndpointConfig {
    pub destination_type: DestinationType,
    pub destination_name: String,
}

impl JmsEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, camel_api::CamelError> {
        let rest = uri.strip_prefix("jms:").ok_or_else(|| {
            camel_api::CamelError::ProcessorError(format!("expected scheme 'jms', got: {uri}"))
        })?;
        let (dtype, name) = rest.split_once(':').ok_or_else(|| {
            camel_api::CamelError::ProcessorError(
                "JMS URI must be jms:queue:name or jms:topic:name".to_string(),
            )
        })?;
        let destination_type = match dtype.to_lowercase().as_str() {
            "queue" => DestinationType::Queue,
            "topic" => DestinationType::Topic,
            other => {
                return Err(camel_api::CamelError::ProcessorError(format!(
                    "JMS destination type must be 'queue' or 'topic', got: {other}"
                )));
            }
        };
        Ok(JmsEndpointConfig {
            destination_type,
            destination_name: name.to_string(),
        })
    }

    pub fn destination(&self) -> String {
        let prefix = match self.destination_type {
            DestinationType::Queue => "queue",
            DestinationType::Topic => "topic",
        };
        format!("{prefix}:{}", self.destination_name)
    }
}

#[derive(Clone)]
pub struct JmsConfig {
    pub broker_url: String,
    pub broker_type: BrokerType,
    pub username: Option<String>,
    pub password: Option<String>,
    pub bridge_version: String,
    pub bridge_cache_dir: PathBuf,
    pub bridge_start_timeout_ms: u64,
    pub broker_reconnect_interval_ms: u64,
}

impl std::fmt::Debug for JmsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JmsConfig")
            .field("broker_url", &self.broker_url)
            .field("broker_type", &self.broker_type)
            .field("username", &self.username)
            .field(
                "password",
                &self.password.as_ref().map(|_| "[REDACTED]".to_string()),
            )
            .field("bridge_version", &self.bridge_version)
            .field("bridge_cache_dir", &self.bridge_cache_dir)
            .field("bridge_start_timeout_ms", &self.bridge_start_timeout_ms)
            .field(
                "broker_reconnect_interval_ms",
                &self.broker_reconnect_interval_ms,
            )
            .finish()
    }
}

impl Default for JmsConfig {
    fn default() -> Self {
        Self {
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            username: None,
            password: None,
            bridge_version: BRIDGE_VERSION.to_string(),
            bridge_cache_dir: default_bridge_cache_dir(),
            bridge_start_timeout_ms: 30_000,
            broker_reconnect_interval_ms: 5_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_queue_uri() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
        assert_eq!(cfg.destination(), "queue:orders");
    }

    #[test]
    fn parse_topic_uri() {
        let cfg = JmsEndpointConfig::from_uri("jms:topic:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Topic);
        assert_eq!(cfg.destination_name, "events");
        assert_eq!(cfg.destination(), "topic:events");
    }

    #[test]
    fn wrong_scheme_returns_error() {
        let err = JmsEndpointConfig::from_uri("kafka:orders").unwrap_err();
        assert!(err.to_string().contains("expected scheme 'jms'"));
    }

    #[test]
    fn missing_destination_name_returns_error() {
        let err = JmsEndpointConfig::from_uri("jms:queue").unwrap_err();
        assert!(err.to_string().contains("jms:queue:name"));
    }

    #[test]
    fn invalid_destination_type_returns_error() {
        let err = JmsEndpointConfig::from_uri("jms:inbox:orders").unwrap_err();
        assert!(err.to_string().contains("'queue' or 'topic'"));
    }

    #[test]
    fn default_config_has_activemq() {
        let cfg = JmsConfig::default();
        assert_eq!(cfg.broker_type, BrokerType::ActiveMq);
        assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
    }

    #[test]
    fn debug_redacts_password() {
        let cfg = JmsConfig {
            password: Some("secret".to_string()),
            ..JmsConfig::default()
        };
        let dbg = format!("{cfg:?}");
        assert!(dbg.contains("[REDACTED]"));
        assert!(!dbg.contains("secret"));
    }
}
