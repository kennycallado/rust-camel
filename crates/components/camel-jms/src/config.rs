use std::path::PathBuf;

use crate::{BRIDGE_VERSION, BrokerType};

/// Per-endpoint connection overrides parsed from the URI query string.
/// These are resolved at `create_endpoint()` time and never reach producer/consumer.
#[derive(Default, Clone)]
pub struct JmsUriOverrides {
    pub broker_url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl std::fmt::Debug for JmsUriOverrides {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JmsUriOverrides")
            .field("broker_url", &self.broker_url)
            .field("username", &self.username)
            .field(
                "password",
                &self.password.as_ref().map(|_| "[REDACTED]".to_string()),
            )
            .finish()
    }
}

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
    /// Connection overrides from URI query string. Resolved in create_endpoint(),
    /// not used by producer or consumer directly.
    pub(crate) uri_overrides: JmsUriOverrides,
}

impl JmsEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, camel_component_api::CamelError> {
        // 1. Strip known scheme prefix
        let (scheme, rest) = if let Some(r) = uri.strip_prefix("jms:") {
            ("jms", r)
        } else if let Some(r) = uri.strip_prefix("activemq:") {
            ("activemq", r)
        } else if let Some(r) = uri.strip_prefix("artemis:") {
            ("artemis", r)
        } else {
            return Err(camel_component_api::CamelError::ProcessorError(format!(
                "expected scheme 'jms', 'activemq', or 'artemis', got: {uri}"
            )));
        };

        // 2. Split query string
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));

        // 3. Parse destination type and name
        let (destination_type, destination_name) =
            if let Some((dtype_str, name)) = path.split_once(':') {
                // explicit type prefix: queue:name or topic:name
                if name.is_empty() {
                    return Err(camel_component_api::CamelError::ProcessorError(
                        "destination name cannot be empty".to_string(),
                    ));
                }
                let dtype = match dtype_str.to_lowercase().as_str() {
                    "queue" => DestinationType::Queue,
                    "topic" => DestinationType::Topic,
                    other => {
                        return Err(camel_component_api::CamelError::ProcessorError(format!(
                            "JMS destination type must be 'queue' or 'topic', got: {other}"
                        )));
                    }
                };
                (dtype, name.to_string())
            } else {
                // no colon in path — shorthand form (only allowed for activemq: and artemis:)
                if scheme == "jms" {
                    return Err(camel_component_api::CamelError::ProcessorError(
                        "JMS URI must be jms:queue:name or jms:topic:name".to_string(),
                    ));
                }
                if path.is_empty() {
                    return Err(camel_component_api::CamelError::ProcessorError(
                        "destination name cannot be empty".to_string(),
                    ));
                }
                (DestinationType::Queue, path.to_string())
            };

        // 4. Parse query params
        let uri_overrides = parse_uri_overrides(query);

        Ok(Self {
            destination_type,
            destination_name,
            uri_overrides,
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

fn parse_uri_overrides(query: &str) -> JmsUriOverrides {
    let mut overrides = JmsUriOverrides::default();
    if query.is_empty() {
        return overrides;
    }
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            if value.is_empty() {
                continue; // empty value = no override
            }
            match key {
                "brokerUrl" => overrides.broker_url = Some(value.to_string()),
                "username" => overrides.username = Some(value.to_string()),
                "password" => overrides.password = Some(value.to_string()),
                _ => {} // unknown params silently ignored
            }
        }
    }
    overrides
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

impl JmsConfig {
    /// Merge URI overrides into this config. Override values win over self.
    /// `broker_type` is NOT merged — it is always set by the scheme, not by URI overrides.
    pub fn merge_overrides(&self, overrides: &JmsUriOverrides) -> Self {
        Self {
            broker_url: overrides
                .broker_url
                .clone()
                .unwrap_or_else(|| self.broker_url.clone()),
            broker_type: self.broker_type.clone(),
            username: overrides.username.clone().or_else(|| self.username.clone()),
            password: overrides.password.clone().or_else(|| self.password.clone()),
            bridge_version: self.bridge_version.clone(),
            bridge_cache_dir: self.bridge_cache_dir.clone(),
            bridge_start_timeout_ms: self.bridge_start_timeout_ms,
            broker_reconnect_interval_ms: self.broker_reconnect_interval_ms,
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
    fn parse_activemq_queue_explicit() {
        let cfg = JmsEndpointConfig::from_uri("activemq:queue:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
        assert!(cfg.uri_overrides.broker_url.is_none());
    }

    #[test]
    fn parse_activemq_topic_explicit() {
        let cfg = JmsEndpointConfig::from_uri("activemq:topic:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Topic);
        assert_eq!(cfg.destination_name, "events");
    }

    #[test]
    fn parse_artemis_queue_explicit() {
        let cfg = JmsEndpointConfig::from_uri("artemis:queue:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn parse_activemq_shorthand_defaults_to_queue() {
        let cfg = JmsEndpointConfig::from_uri("activemq:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn parse_artemis_shorthand_defaults_to_queue() {
        let cfg = JmsEndpointConfig::from_uri("artemis:my-queue").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "my-queue");
    }

    #[test]
    fn parse_query_broker_url_override() {
        let cfg = JmsEndpointConfig::from_uri("activemq:queue:orders?brokerUrl=tcp://host:61617")
            .unwrap();
        assert_eq!(cfg.destination_name, "orders");
        assert_eq!(
            cfg.uri_overrides.broker_url.as_deref(),
            Some("tcp://host:61617")
        );
        assert!(cfg.uri_overrides.username.is_none());
    }

    #[test]
    fn parse_query_all_overrides() {
        let cfg = JmsEndpointConfig::from_uri(
            "jms:queue:orders?brokerUrl=tcp://h:1&username=admin&password=secret",
        )
        .unwrap();
        assert_eq!(cfg.uri_overrides.broker_url.as_deref(), Some("tcp://h:1"));
        assert_eq!(cfg.uri_overrides.username.as_deref(), Some("admin"));
        assert_eq!(cfg.uri_overrides.password.as_deref(), Some("secret"));
    }

    #[test]
    fn parse_query_unknown_params_ignored() {
        let cfg = JmsEndpointConfig::from_uri(
            "activemq:queue:orders?unknownParam=foo&brokerUrl=tcp://h:1",
        )
        .unwrap();
        assert_eq!(cfg.uri_overrides.broker_url.as_deref(), Some("tcp://h:1"));
    }

    #[test]
    fn parse_query_empty_value_treated_as_no_override() {
        let cfg = JmsEndpointConfig::from_uri("activemq:queue:orders?brokerUrl=").unwrap();
        assert!(cfg.uri_overrides.broker_url.is_none());
    }

    #[test]
    fn parse_activemq_empty_destination_errors() {
        assert!(JmsEndpointConfig::from_uri("activemq:queue:").is_err());
        assert!(JmsEndpointConfig::from_uri("activemq:").is_err());
    }

    #[test]
    fn parse_jms_shorthand_still_errors() {
        // jms: without explicit queue:/topic: is unchanged — still an error
        let err = JmsEndpointConfig::from_uri("jms:orders").unwrap_err();
        assert!(
            err.to_string().contains("jms:queue:name")
                || err.to_string().contains("jms:topic:name")
        );
    }

    #[test]
    fn parse_wrong_scheme_new_message() {
        let err = JmsEndpointConfig::from_uri("kafka:orders").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("activemq") && msg.contains("artemis") && msg.contains("jms"),
            "error should mention all valid schemes, got: {msg}"
        );
    }

    #[test]
    fn uri_overrides_debug_redacts_password() {
        let o = JmsUriOverrides {
            broker_url: Some("tcp://host:61616".to_string()),
            username: Some("admin".to_string()),
            password: Some("secret".to_string()),
        };
        let s = format!("{:?}", o);
        assert!(
            s.contains("[REDACTED]"),
            "password must be redacted, got: {s}"
        );
        assert!(
            !s.contains("secret"),
            "raw password must not appear, got: {s}"
        );
    }

    #[test]
    fn uri_overrides_default_is_all_none() {
        let o = JmsUriOverrides::default();
        assert!(o.broker_url.is_none());
        assert!(o.username.is_none());
        assert!(o.password.is_none());
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

    #[test]
    fn merge_overrides_uri_wins_for_broker_url() {
        let base = JmsConfig::default(); // broker_url = "tcp://localhost:61616"
        let overrides = JmsUriOverrides {
            broker_url: Some("tcp://override:9999".to_string()),
            username: None,
            password: None,
        };
        let merged = base.merge_overrides(&overrides);
        assert_eq!(merged.broker_url, "tcp://override:9999");
        assert_eq!(merged.broker_type, BrokerType::ActiveMq); // unchanged
        assert!(merged.username.is_none()); // from base
    }

    #[test]
    fn merge_overrides_base_used_when_no_override() {
        let base = JmsConfig {
            username: Some("base_user".to_string()),
            ..JmsConfig::default()
        };
        let overrides = JmsUriOverrides::default(); // all None
        let merged = base.merge_overrides(&overrides);
        assert_eq!(merged.broker_url, "tcp://localhost:61616");
        assert_eq!(merged.username.as_deref(), Some("base_user"));
    }

    #[test]
    fn merge_overrides_broker_type_never_overridden() {
        let base = JmsConfig {
            broker_type: BrokerType::Artemis,
            ..JmsConfig::default()
        };
        let overrides = JmsUriOverrides::default();
        let merged = base.merge_overrides(&overrides);
        assert_eq!(merged.broker_type, BrokerType::Artemis);
    }
}
