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

        Ok(Self {
            config: MasterComponentConfig { drain_timeout_ms },
        })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(MasterComponent::new(self.config)));
    }
}
