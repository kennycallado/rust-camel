use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

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
            .unwrap_or(Some(30));

        Ok(Self {
            config: MasterComponentConfig {
                drain_timeout_ms,
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
        assert_eq!(bundle.config.delegate_retry_max_attempts, Some(30));
    }

    #[test]
    fn from_toml_parses_delegate_retry_max_attempts() {
        let mut table = toml::map::Map::new();
        table.insert(
            "delegate_retry_max_attempts".to_string(),
            toml::Value::Integer(7),
        );
        let bundle = MasterBundle::from_toml(toml::Value::Table(table)).unwrap();
        assert_eq!(bundle.config.delegate_retry_max_attempts, Some(7));
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
}
