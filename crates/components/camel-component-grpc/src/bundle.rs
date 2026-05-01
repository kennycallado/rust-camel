use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::component::GrpcComponent;

pub struct GrpcBundle;

impl ComponentBundle for GrpcBundle {
    fn config_key() -> &'static str {
        "grpc"
    }

    fn from_toml(_value: toml::Value) -> Result<Self, CamelError> {
        Ok(Self)
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(GrpcComponent::new()));
    }
}
