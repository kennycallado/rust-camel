use std::collections::HashMap;
use std::path::PathBuf;

use crate::BrokerType;

pub fn default_bridge_cache_dir() -> PathBuf {
    camel_bridge::download::default_cache_dir()
}

fn default_max_bridges() -> usize {
    8
}

fn default_bridge_start_timeout_ms() -> u64 {
    30_000
}

fn default_broker_reconnect_interval_ms() -> u64 {
    5_000
}

fn default_health_check_interval_ms() -> u64 {
    5_000
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
    pub broker_name: Option<String>,
}

impl JmsEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, camel_component_api::CamelError> {
        let (scheme, rest) = if let Some(r) = uri.strip_prefix("jms:") {
            ("jms", r)
        } else if let Some(r) = uri.strip_prefix("activemq:") {
            ("activemq", r)
        } else if let Some(r) = uri.strip_prefix("artemis:") {
            ("artemis", r)
        } else {
            return Err(camel_component_api::CamelError::ProcessorError(
                "expected scheme 'jms', 'activemq', or 'artemis'".to_string(),
            ));
        };

        let (path, query) = match rest.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (rest, None),
        };

        let (destination_type, destination_name) =
            match path.splitn(2, ':').collect::<Vec<_>>().as_slice() {
                // Shorthand (no prefix): only allowed for activemq/artemis, NOT jms
                [name] if !name.is_empty() && scheme != "jms" => {
                    (DestinationType::Queue, name.to_string())
                }
                // Explicit queue: or topic:
                [prefix, name]
                    if (*prefix == "queue" || *prefix == "topic") && !name.is_empty() =>
                {
                    let dt = if *prefix == "queue" {
                        DestinationType::Queue
                    } else {
                        DestinationType::Topic
                    };
                    (dt, name.to_string())
                }
                // jms: shorthand (rejected)
                [name] if !name.is_empty() && scheme == "jms" => {
                    return Err(camel_component_api::CamelError::ProcessorError(format!(
                        "URI 'jms:{}' is ambiguous — use 'jms:queue:{}' or 'jms:topic:{}'",
                        name, name, name
                    )));
                }
                _ => {
                    return Err(camel_component_api::CamelError::ProcessorError(
                        "destination must be 'queue:<name>' or 'topic:<name>'".to_string(),
                    ));
                }
            };

        // Parse broker= query param
        let broker_name = query.and_then(|q| {
            q.split('&').find_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                if k == "broker" {
                    if v.is_empty() {
                        None
                    } else {
                        Some(v.to_string())
                    }
                } else {
                    None
                }
            })
        });

        Ok(JmsEndpointConfig {
            destination_type,
            destination_name,
            broker_name,
        })
    }
}

#[derive(Clone, PartialEq, serde::Deserialize)]
pub struct BrokerConfig {
    pub broker_url: String,
    pub broker_type: BrokerType,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl std::fmt::Debug for BrokerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokerConfig")
            .field("broker_url", &self.broker_url)
            .field("broker_type", &self.broker_type)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct JmsPoolConfig {
    pub brokers: HashMap<String, BrokerConfig>,
    pub default_broker: String,
    #[serde(default = "default_max_bridges")]
    pub max_bridges: usize,
    #[serde(default = "default_bridge_start_timeout_ms")]
    pub bridge_start_timeout_ms: u64,
    #[serde(default = "default_broker_reconnect_interval_ms")]
    pub broker_reconnect_interval_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
    #[serde(default = "default_bridge_cache_dir")]
    pub bridge_cache_dir: PathBuf,
}

impl Default for JmsPoolConfig {
    fn default() -> Self {
        Self {
            brokers: HashMap::new(),
            default_broker: String::new(),
            max_bridges: default_max_bridges(),
            bridge_start_timeout_ms: default_bridge_start_timeout_ms(),
            broker_reconnect_interval_ms: default_broker_reconnect_interval_ms(),
            health_check_interval_ms: default_health_check_interval_ms(),
            bridge_cache_dir: default_bridge_cache_dir(),
        }
    }
}

impl JmsPoolConfig {
    /// Convenience constructor for single-broker scenarios (tests, simple examples).
    /// Creates a pool with one broker named "default".
    pub fn single_broker(broker_url: impl Into<String>, broker_type: BrokerType) -> Self {
        let url = broker_url.into();
        let mut brokers = HashMap::new();
        brokers.insert(
            "default".to_string(),
            BrokerConfig {
                broker_url: url,
                broker_type,
                username: None,
                password: None,
            },
        );
        Self {
            brokers,
            default_broker: "default".to_string(),
            max_bridges: 1,
            ..Self::default()
        }
    }

    /// Validates the config: all brokers must have non-empty URLs, default_broker must exist.
    pub fn validate(&self) -> Result<(), camel_component_api::CamelError> {
        for (name, bc) in &self.brokers {
            if bc.broker_url.is_empty() {
                return Err(camel_component_api::CamelError::ProcessorError(format!(
                    "broker '{}' has an empty broker_url",
                    name
                )));
            }
        }
        if self.brokers.is_empty() {
            // valid at config time; usage may fail later
        } else if self.default_broker.is_empty() {
            return Err(camel_component_api::CamelError::ProcessorError(
                "jms: default_broker must be set when brokers map is non-empty".to_string(),
            ));
        } else if !self.brokers.contains_key(&self.default_broker) {
            return Err(camel_component_api::CamelError::ProcessorError(format!(
                "jms: default_broker '{}' is not declared in brokers map",
                self.default_broker
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrokerType;

    #[test]
    fn parse_jms_queue_explicit() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
        assert_eq!(cfg.broker_name, None);
    }

    #[test]
    fn parse_jms_topic_explicit() {
        let cfg = JmsEndpointConfig::from_uri("jms:topic:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Topic);
        assert_eq!(cfg.destination_name, "events");
    }

    #[test]
    fn parse_jms_with_broker_param() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?broker=primary").unwrap();
        assert_eq!(cfg.broker_name, Some("primary".to_string()));
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn jms_shorthand_rejected() {
        let err = JmsEndpointConfig::from_uri("jms:orders").unwrap_err();
        assert!(err.to_string().contains("ambiguous"), "got: {}", err);
    }

    #[test]
    fn invalid_destination_type_returns_error() {
        let err = JmsEndpointConfig::from_uri("jms:inbox:orders").unwrap_err();
        assert!(
            err.to_string().contains("'queue' or 'topic'")
                || err.to_string().contains("queue:<name>"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_activemq_queue_explicit() {
        let cfg = JmsEndpointConfig::from_uri("activemq:queue:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn parse_activemq_shorthand() {
        let cfg = JmsEndpointConfig::from_uri("activemq:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn parse_artemis_shorthand_defaults_to_queue() {
        let cfg = JmsEndpointConfig::from_uri("artemis:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "events");
    }

    #[test]
    fn parse_jms_with_empty_broker_param_treated_as_none() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?broker=").unwrap();
        assert_eq!(cfg.broker_name, None);
    }

    #[test]
    fn parse_activemq_topic() {
        let cfg = JmsEndpointConfig::from_uri("activemq:topic:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Topic);
        assert_eq!(cfg.destination_name, "events");
    }

    #[test]
    fn single_broker_convenience() {
        let cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        assert_eq!(cfg.max_bridges, 1);
        assert_eq!(cfg.default_broker, "default");
        assert!(cfg.brokers.contains_key("default"));
        let bc = &cfg.brokers["default"];
        assert_eq!(bc.broker_url, "tcp://localhost:61616");
        assert_eq!(bc.broker_type, BrokerType::ActiveMq);
    }

    #[test]
    fn default_pool_config() {
        let cfg = JmsPoolConfig::default();
        assert_eq!(cfg.max_bridges, 8);
        assert!(cfg.default_broker.is_empty());
        assert!(cfg.brokers.is_empty());
        assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
        assert_eq!(cfg.broker_reconnect_interval_ms, 5_000);
        assert_eq!(cfg.health_check_interval_ms, 5_000);
    }

    #[test]
    fn validate_empty_broker_url() {
        let cfg = JmsPoolConfig::single_broker("", BrokerType::ActiveMq);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("empty broker_url"), "got: {}", err);
    }

    #[test]
    fn validate_missing_default_broker() {
        let mut cfg = JmsPoolConfig::default();
        cfg.brokers.insert(
            "declared".to_string(),
            BrokerConfig {
                broker_url: "tcp://localhost:61616".to_string(),
                broker_type: BrokerType::ActiveMq,
                username: None,
                password: None,
            },
        );
        cfg.default_broker = "missing".to_string();
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("not declared"), "got: {}", err);
    }

    #[test]
    fn validate_non_empty_brokers_requires_default_broker() {
        let mut cfg = JmsPoolConfig::default();
        cfg.brokers.insert(
            "mybroker".to_string(),
            BrokerConfig {
                broker_url: "tcp://localhost:61616".to_string(),
                broker_type: BrokerType::ActiveMq,
                username: None,
                password: None,
            },
        );
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("default_broker"), "got: {}", err);
    }

    #[test]
    fn broker_config_debug_redacts_password() {
        let bc = BrokerConfig {
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            username: Some("admin".to_string()),
            password: Some("secret".to_string()),
        };
        let s = format!("{bc:?}");
        assert!(s.contains("<redacted>"), "got: {s}");
        assert!(!s.contains("secret"), "got: {s}");
    }

    #[test]
    fn validate_ok() {
        let cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.validate().unwrap();
    }
}
