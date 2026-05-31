use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::static_config::HttpStaticConfig;
use crate::static_endpoint::HttpStaticComponent;
use crate::{HttpComponent, HttpsComponent, config::HttpConfig};

pub struct HttpBundle {
    config: HttpConfig,
}

impl ComponentBundle for HttpBundle {
    fn config_key() -> &'static str {
        "http"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: HttpConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self { config })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(HttpComponent::with_config(self.config.clone())));
        ctx.register_component_dyn(Arc::new(HttpsComponent::with_config(self.config)));
    }
}

// ---------------------------------------------------------------------------
// HttpStaticBundle
// ---------------------------------------------------------------------------

/// Component bundle for the `http-static` scheme.
///
/// Loads configuration from `[default.components.http-static]` in Camel.toml
/// and registers [`HttpStaticComponent`].
#[derive(Debug)]
pub struct HttpStaticBundle {
    config: HttpStaticConfig,
}

impl ComponentBundle for HttpStaticBundle {
    fn config_key() -> &'static str {
        "http-static"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        // Empty TOML section → use default config (dir will be validated at consumer start).
        // This allows the bundle to register even when no [components.http-static] exists.
        if let toml::Value::Table(ref table) = value
            && table.is_empty()
        {
            return Ok(Self {
                config: HttpStaticConfig::default(),
            });
        }
        let config: HttpStaticConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self { config })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(HttpStaticComponent::with_config(self.config)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRegistrar {
        schemes: Vec<String>,
    }

    impl camel_component_api::ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(
            &mut self,
            component: std::sync::Arc<dyn camel_component_api::Component>,
        ) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    #[test]
    fn http_bundle_from_toml_empty_uses_defaults() {
        let value: toml::Value = toml::from_str("").unwrap();
        let result = HttpBundle::from_toml(value);
        assert!(
            result.is_ok(),
            "empty HTTP toml must use defaults: {:?}",
            result.err()
        );
    }

    #[test]
    fn http_bundle_from_toml_custom_timeout() {
        let value: toml::Value = toml::from_str("connect_timeout_ms = 1000").unwrap();
        let bundle = HttpBundle::from_toml(value).unwrap();
        assert_eq!(bundle.config.connect_timeout_ms, 1000);
    }

    #[test]
    fn http_bundle_from_toml_returns_error_on_invalid_config() {
        let mut table = toml::map::Map::new();
        table.insert(
            "connect_timeout_ms".to_string(),
            toml::Value::String("not-a-number".to_string()),
        );

        let result = HttpBundle::from_toml(toml::Value::Table(table));
        assert!(result.is_err(), "expected Err on malformed config");
        let err_msg = match result {
            Err(err) => err.to_string(),
            Ok(_) => panic!("expected Err on malformed config"),
        };
        assert!(
            err_msg.contains("Configuration error"),
            "expected CamelError::Config, got: {err_msg}"
        );
    }

    #[test]
    fn http_bundle_registers_expected_schemes() {
        let bundle = HttpBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap();
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["http", "https"]);
    }

    // HttpStaticBundle tests

    #[test]
    fn http_static_bundle_config_key() {
        assert_eq!(HttpStaticBundle::config_key(), "http-static");
    }

    #[test]
    fn http_static_bundle_from_toml_empty_uses_defaults() {
        let value: toml::Value = toml::from_str("").unwrap();
        let result = HttpStaticBundle::from_toml(value);
        assert!(
            result.is_ok(),
            "empty http-static toml must use defaults: {:?}",
            result.err()
        );
        let bundle = result.unwrap();
        assert!(bundle.config.dir.as_os_str().is_empty());
        assert_eq!(bundle.config.port, 8080);
    }

    #[test]
    fn http_static_bundle_from_toml_with_config() {
        let toml_str = r#"
            dir = "/app/spa/dist"
            port = 3000
            spaFallback = true
            cacheControl = "public, max-age=3600"
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let bundle = HttpStaticBundle::from_toml(value).unwrap();
        assert_eq!(bundle.config.dir, std::path::PathBuf::from("/app/spa/dist"));
        assert_eq!(bundle.config.port, 3000);
        assert!(bundle.config.spa_fallback);
        assert_eq!(bundle.config.cache_control, "public, max-age=3600");
    }

    #[test]
    fn http_static_bundle_from_toml_returns_error_on_invalid_config() {
        let mut table = toml::map::Map::new();
        table.insert(
            "port".to_string(),
            toml::Value::String("not-a-number".to_string()),
        );

        let result = HttpStaticBundle::from_toml(toml::Value::Table(table));
        assert!(result.is_err(), "expected Err on malformed config");
        let err_msg = match result {
            Err(err) => err.to_string(),
            Ok(_) => panic!("expected Err on malformed config"),
        };
        assert!(
            err_msg.contains("Configuration error"),
            "expected CamelError::Config, got: {err_msg}"
        );
    }

    #[test]
    fn http_static_bundle_registers_expected_scheme() {
        let toml_str = r#"dir = "/tmp/static""#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let bundle = HttpStaticBundle::from_toml(value).unwrap();
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["http-static"]);
    }
}
