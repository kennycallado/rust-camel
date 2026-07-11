//! ExecBundle — ComponentBundle for `exec:`. Owns `[components.exec]`.
//!
//! Provides fail-fast `from_toml` deserialization + validation (ADR-0033) and
//! registers the ExecComponent via the real `ComponentRegistrar` trait.

use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::ExecComponent;
use crate::config::ExecGlobalConfig;

/// Bundle for the `exec:` component scheme.
#[derive(Debug)]
pub struct ExecBundle {
    pub(crate) config: ExecGlobalConfig,
}

impl ExecBundle {
    /// Access the validated global config (public for integration tests).
    pub fn config(&self) -> &ExecGlobalConfig {
        &self.config
    }
}

impl ComponentBundle for ExecBundle {
    fn config_key() -> &'static str {
        "exec"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let mut config: ExecGlobalConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        // Fail-fast validation + canonical pinning, against CWD as workspace base.
        config.validate(&std::env::current_dir()?)?;
        Ok(Self { config })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        let component = ExecComponent::new(self.config);
        ctx.register_component_dyn(Arc::new(component));
    }
}

#[cfg(test)]
mod tests {
    use super::ExecBundle;
    use camel_component_api::ComponentBundle;

    #[test]
    fn from_toml_fails_when_no_profiles() {
        let v: toml::Value = toml::from_str("workspace_root = \".\"\n").unwrap();
        let err = ExecBundle::from_toml(v).unwrap_err();
        assert!(err.to_string().contains("no profiles"));
    }

    #[test]
    fn config_key_is_exec() {
        assert_eq!(ExecBundle::config_key(), "exec");
    }
}
