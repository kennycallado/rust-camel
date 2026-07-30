//! Component bundle for the `template:` scheme (ADR-0047 Stage 2, Phase 4 /
//! Task 4.5).
//!
//! [`TemplateBundle`] owns the `[components.template]` TOML key. It folds the
//! two operator-facing limit layers — acquisition `limits` and render
//! `render-limits` — and registers a single [`TemplateComponent`] into the
//! context. Both layers default when absent, so an empty `[template]` block
//! (or no block at all) yields the ADR-0047 defaults. This makes both limit
//! layers operator-configurable (design "Two limit layers").
//!
//! Mirrors the shape of `camel-file`'s `FileBundle`: deserialize the raw block
//! into a config struct, then construct the component and register it.

use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};
use camel_language_api::MinijinjaLimitsConfig;
use serde::Deserialize;

use crate::component::TemplateComponent;
use crate::config::ExternalTemplateLimitsConfig;

/// Operator-facing `[components.template]` block: two independent limit layers.
///
/// - `limits`        → [`ExternalTemplateLimitsConfig`] (acquisition bounds:
///   closure size, include count/depth, template size, reload timeout).
/// - `render-limits` → [`MinijinjaLimitsConfig`] (render-time bounds: source,
///   context, output size, fuel, recursion, execution timeout).
///
/// Both are `#[serde(default)]`, so a missing sub-table falls back to the
/// per-struct defaults (ADR-0047 §4.1).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateBundleConfig {
    /// Acquisition limits applied while building the template closure.
    #[serde(default)]
    pub limits: ExternalTemplateLimitsConfig,

    /// Render limits applied per render inside the MiniJinja environment.
    #[serde(default)]
    pub render_limits: MinijinjaLimitsConfig,
}

/// Bundle that registers the `template:` scheme from the `[template]` config
/// block.
///
/// Constructed by `camel-cli` via `register_bundle!(camel_template::TemplateBundle)`
/// — always-on (no feature gate), like the other built-in transform components.
pub struct TemplateBundle {
    config: TemplateBundleConfig,
}

impl ComponentBundle for TemplateBundle {
    fn config_key() -> &'static str {
        "template"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: TemplateBundleConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self { config })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        let component = TemplateComponent::new(self.config.limits, self.config.render_limits);
        ctx.register_component_dyn(Arc::new(component));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRegistrar {
        schemes: Vec<String>,
    }

    impl ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(&mut self, component: Arc<dyn camel_component_api::Component>) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    #[test]
    fn config_key_is_template() {
        assert_eq!(TemplateBundle::config_key(), "template");
    }

    #[test]
    fn from_toml_empty_uses_defaults() {
        let value: toml::Value = toml::from_str("").unwrap();
        let bundle = TemplateBundle::from_toml(value);
        assert!(bundle.is_ok(), "empty TOML must use defaults");
    }

    #[test]
    fn from_toml_parses_both_limit_layers() {
        let raw = r#"
limits.max-include-count = 8
render-limits.fuel = 1234
"#;
        let value: toml::Value = toml::from_str(raw).unwrap();
        let bundle = TemplateBundle::from_toml(value).expect("both limit layers must deserialize");
        assert_eq!(bundle.config.limits.max_include_count, Some(8));
        assert_eq!(bundle.config.render_limits.fuel, Some(1234));
    }

    #[test]
    fn from_toml_rejects_unknown_field() {
        let raw = "bogus = 1\n";
        let value: toml::Value = toml::from_str(raw).unwrap();
        let result = TemplateBundle::from_toml(value);
        assert!(result.is_err(), "deny_unknown_fields must reject `bogus`");
    }

    #[test]
    fn register_all_registers_template_scheme() {
        let bundle = TemplateBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap();
        let mut registrar = TestRegistrar { schemes: vec![] };
        bundle.register_all(&mut registrar);
        assert_eq!(registrar.schemes, vec!["template"]);
    }
}
