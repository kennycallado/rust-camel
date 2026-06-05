use std::sync::Arc;

use camel_component_api::NetworkRetryPolicy;
use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};
use serde::Deserialize;

use crate::{MasterComponent, config::MasterComponentConfig};

pub struct MasterBundle {
    config: MasterComponentConfig,
}

impl ComponentBundle for MasterBundle {
    fn config_key() -> &'static str {
        "master"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let drain_timeout_ms = value
            .get("drain_timeout_ms")
            .and_then(|v| v.as_integer())
            .unwrap_or(5000) as u64;

        let delegate_retry_max_attempts = value
            .get("delegate_retry_max_attempts")
            .and_then(|v| v.as_integer())
            .and_then(|v| u32::try_from(v).ok())
            .map(|v| if v == 0 { None } else { Some(v) })
            .unwrap_or(None);

        // Parse structured reconnect policy if present in TOML.
        let reconnect = value
            .get("reconnect")
            .cloned()
            .and_then(|v| NetworkRetryPolicy::deserialize(v).ok())
            .unwrap_or_else(|| {
                NetworkRetryPolicy {
                    max_attempts: 0, // unlimited by default
                    ..NetworkRetryPolicy::default()
                }
            });

        Ok(Self {
            config: MasterComponentConfig {
                drain_timeout_ms,
                reconnect,
                delegate_retry_max_attempts,
            },
        })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(MasterComponent::new(self.config)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_toml_uses_defaults() {
        let bundle = MasterBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap();
        assert_eq!(bundle.config.drain_timeout_ms, 5000);
        assert_eq!(bundle.config.reconnect.max_attempts, 0);
        assert!(bundle.config.reconnect.enabled);
        assert_eq!(bundle.config.delegate_retry_max_attempts, None);
    }

    #[test]
    fn from_toml_parses_delegate_retry_max_attempts() {
        let mut table = toml::map::Map::new();
        table.insert(
            "delegate_retry_max_attempts".to_string(),
            toml::Value::Integer(7),
        );
        let bundle = MasterBundle::from_toml(toml::Value::Table(table)).unwrap();
        // delegate_retry_max_attempts=7, but reconnect is still at default (unlimited)
        // since from_toml doesn't bridge; bridging happens in MasterComponent::new()
        assert_eq!(bundle.config.delegate_retry_max_attempts, Some(7));
        assert_eq!(bundle.config.reconnect.max_attempts, 0);
    }

    #[test]
    fn from_toml_zero_means_unlimited_retries() {
        let mut table = toml::map::Map::new();
        table.insert(
            "delegate_retry_max_attempts".to_string(),
            toml::Value::Integer(0),
        );
        let bundle = MasterBundle::from_toml(toml::Value::Table(table)).unwrap();
        assert_eq!(bundle.config.delegate_retry_max_attempts, None);
    }

    #[test]
    fn from_toml_parses_reconnect_section() {
        let mut reconnect_table = toml::map::Map::new();
        reconnect_table.insert("max_attempts".to_string(), toml::Value::Integer(10));
        reconnect_table.insert("enabled".to_string(), toml::Value::Boolean(false));

        let mut table = toml::map::Map::new();
        table.insert("reconnect".to_string(), toml::Value::Table(reconnect_table));

        let bundle = MasterBundle::from_toml(toml::Value::Table(table)).unwrap();
        assert_eq!(bundle.config.reconnect.max_attempts, 10);
        assert!(!bundle.config.reconnect.enabled);
    }

    #[test]
    fn from_toml_reconnect_wins_over_delegate_retry() {
        // When both reconnect and delegate_retry_max_attempts are set,
        // MasterComponent::new() prefers the explicit reconnect.
        let mut reconnect_table = toml::map::Map::new();
        reconnect_table.insert("max_attempts".to_string(), toml::Value::Integer(5));

        let mut table = toml::map::Map::new();
        table.insert("reconnect".to_string(), toml::Value::Table(reconnect_table));
        table.insert(
            "delegate_retry_max_attempts".to_string(),
            toml::Value::Integer(20),
        );

        let bundle = MasterBundle::from_toml(toml::Value::Table(table)).unwrap();
        assert_eq!(bundle.config.reconnect.max_attempts, 5); // reconnect wins
        assert_eq!(bundle.config.delegate_retry_max_attempts, Some(20));
    }
}
