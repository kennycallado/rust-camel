//! camel-component-surrealdb — SurrealDB multi-model database component for rust-camel.
//!
//! Provides document, graph, and vector operations through the surrealdb crate.
//! Integrates with the datasource domain via PoolFactory.

pub mod bundle;
pub mod config;
pub mod consumer;
pub mod endpoint;
pub mod error;
pub mod headers;
pub(crate) mod metadata;
pub mod polling;
pub mod pool_factory;
pub mod producer;
pub mod query;
pub mod vector;

use std::sync::Arc;

use camel_api::datasource::{DatasourceCatalog, DatasourceConfig};
use camel_component_api::{CamelError, Component, ComponentContext, ComponentMetadata, Endpoint};
use metadata::SurrealDbMetadataDescriptor;
use pool_factory::SurrealDbTimeouts;

pub use bundle::SurrealDbBundle;
pub use config::{SurrealDbEndpointConfig, SurrealDbOperation, VectorMetric};
pub use error::SurrealDbError;
pub use pool_factory::redact_db_url;

/// Applies the datasource-scoped deadline extras to an endpoint config.
///
/// The returned endpoint is a `Box<dyn Endpoint>` with no config access, so
/// this seam is the testable surface for the route-creation-time propagation:
/// [`crate::SurrealDbComponent::create_endpoint`] calls it once
/// `ds_config` is resolved, and the updated config rides into the endpoint,
/// producer, consumer, and polling consumer via the existing `config.clone()`
/// threading.
///
/// Endpoints without a datasource never reach this function and keep the
/// [`SurrealDbEndpointConfig::default`] values.
pub(crate) fn apply_datasource_timeouts(
    mut config: SurrealDbEndpointConfig,
    ds_config: &DatasourceConfig,
) -> Result<SurrealDbEndpointConfig, CamelError> {
    let timeouts = SurrealDbTimeouts::from_extra(&ds_config.extra)?;
    config.connect_timeout = timeouts.connect;
    config.operation_timeout = timeouts.operation;
    Ok(config)
}

/// SurrealDB component — factory for `surrealdb:` endpoints.
///
/// Supports document, graph, vector, and live query operations. Each
/// endpoint references a named datasource from the catalog whose connection
/// string is resolved at endpoint creation time.
///
/// When a `DatasourceCatalog` is present, the component resolves datasource
/// names into connection URLs. The component also validates that `Live`
/// operations use a WebSocket protocol (ws/wss), since SurrealDB's live
/// queries require persistent connections.
pub struct SurrealDbComponent {
    catalog: Option<Arc<dyn DatasourceCatalog>>,
}

impl SurrealDbComponent {
    /// Create a new component without a datasource catalog.
    ///
    /// Use this when endpoints reference their datasource directly via
    /// `db_url` in the config, or when testing.
    pub fn new() -> Self {
        Self { catalog: None }
    }

    /// Attach a datasource catalog for named datasource resolution.
    pub fn with_catalog(catalog: Arc<dyn DatasourceCatalog>) -> Self {
        Self {
            catalog: Some(catalog),
        }
    }

    /// Access the optional datasource catalog.
    pub fn catalog(&self) -> Option<&Arc<dyn DatasourceCatalog>> {
        self.catalog.as_ref()
    }
}

impl Default for SurrealDbComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for SurrealDbComponent {
    fn scheme(&self) -> &str {
        "surrealdb"
    }

    fn metadata(&self) -> ComponentMetadata {
        SurrealDbMetadataDescriptor::metadata()
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        // Step 1: Parse URI into SurrealDbEndpointConfig (validates scheme + operation + params)
        let config = SurrealDbEndpointConfig::from_uri(uri)?;

        // Step 2: Validate operation-specific required params (fail-fast)
        config.validate().map_err(|e| {
            CamelError::EndpointCreationFailed(format!("config validation failed: {e}"))
        })?;

        // Step 3: If the endpoint references a datasource name, resolve it from catalog
        if !config.datasource.is_empty() {
            let catalog = self.catalog.as_ref().ok_or_else(|| {
                CamelError::EndpointCreationFailed(
                    "datasource requires catalog — no datasource catalog configured".into(),
                )
            })?;

            let ds_config = catalog.get_config(&config.datasource).ok_or_else(|| {
                CamelError::EndpointCreationFailed(format!(
                    "datasource '{}' not found in catalog",
                    config.datasource
                ))
            })?;

            // Step 3b: Fail fast on invalid deadline extras and propagate the
            // configured bounds through the endpoint config.
            let config = apply_datasource_timeouts(config, &ds_config)
                .map_err(|err| CamelError::EndpointCreationFailed(err.to_string()))?;

            // Step 4: LIVE operation requires WebSocket protocol (ws/wss)
            if config.operation == crate::config::SurrealDbOperation::Live {
                let url = &ds_config.db_url;
                if !url.starts_with("ws://") && !url.starts_with("wss://") {
                    return Err(CamelError::from(SurrealDbError::LiveRequiresWebSocket(
                        redact_db_url(url),
                    )));
                }
            }

            // Step 5: Return the real endpoint
            return Ok(Box::new(endpoint::SurrealDbEndpoint::new(
                uri.to_string(),
                config,
                self.catalog.clone(),
            )));
        }

        // No datasource name: return endpoint without catalog resolution
        Ok(Box::new(endpoint::SurrealDbEndpoint::new(
            uri.to_string(),
            config,
            self.catalog.clone(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::NoOpComponentContext;

    #[test]
    fn component_scheme_is_surrealdb() {
        let c = SurrealDbComponent::new();
        assert_eq!(c.scheme(), "surrealdb");
    }

    #[test]
    fn component_default_is_new() {
        let c1 = SurrealDbComponent::default();
        let c2 = SurrealDbComponent::new();
        assert!(c1.catalog.is_none());
        assert!(c2.catalog.is_none());
    }

    #[test]
    fn component_rejects_invalid_uri() {
        let c = SurrealDbComponent::new();
        let ctx = NoOpComponentContext;
        let result = c.create_endpoint("invalid", &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn component_rejects_missing_datasource_in_uri() {
        let c = SurrealDbComponent::new();
        let ctx = NoOpComponentContext;
        let result = c.create_endpoint("surrealdb:query", &ctx);
        // from_uri() returns InvalidUri when datasource is missing
        assert!(result.is_err());
    }

    #[test]
    fn component_create_endpoint_succeeds_with_catalog() {
        // Valid URI + catalog with config → should return Ok with a real endpoint.
        use camel_api::datasource::DatasourceConfig;

        struct TestCatalog;
        impl camel_api::datasource::DatasourceCatalog for TestCatalog {
            fn get_config(&self, name: &str) -> Option<DatasourceConfig> {
                if name == "main" {
                    Some(DatasourceConfig {
                        db_url: "ws://localhost:8000".into(),
                        provider: Some("surrealdb".into()),
                        max_connections: None,
                        min_connections: None,
                        idle_timeout_secs: None,
                        max_lifetime_secs: None,
                        ssl_mode: None,
                        ssl_root_cert: None,
                        ssl_cert: None,
                        ssl_key: None,
                        extra: std::collections::HashMap::new(),
                    })
                } else {
                    None
                }
            }
            fn get_pool<'a>(&'a self, _name: &'a str) -> camel_api::datasource::GetPoolFuture<'a> {
                Box::pin(async { Err(CamelError::Config("no pool".into())) })
            }
            fn register_factory(
                &self,
                _kind: &str,
                _factory: Arc<dyn camel_api::datasource::PoolFactory>,
            ) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let c = SurrealDbComponent::with_catalog(Arc::new(TestCatalog));
        let ctx = NoOpComponentContext;
        let result = c.create_endpoint("surrealdb:query?datasource=main", &ctx);
        assert!(result.is_ok());
        let endpoint = result.unwrap();
        assert_eq!(endpoint.uri(), "surrealdb:query?datasource=main");
    }

    #[test]
    fn component_with_catalog_survives_construction() {
        use camel_api::datasource::DatasourceConfig;

        struct TestCatalog;
        impl camel_api::datasource::DatasourceCatalog for TestCatalog {
            fn get_config(&self, _name: &str) -> Option<DatasourceConfig> {
                None
            }
            fn get_pool<'a>(&'a self, _name: &'a str) -> camel_api::datasource::GetPoolFuture<'a> {
                Box::pin(async { Err(CamelError::Config("no pool".into())) })
            }
            fn register_factory(
                &self,
                _kind: &str,
                _factory: Arc<dyn camel_api::datasource::PoolFactory>,
            ) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let c = SurrealDbComponent::with_catalog(Arc::new(TestCatalog));
        assert!(c.catalog().is_some());
    }

    // --- datasource timeout propagation (design: create_endpoint fail-fast) ---

    /// Builds a `DatasourceConfig` over `ws://localhost:8000` carrying the
    /// given extras; the URL is never dialled by these tests.
    fn datasource_with_extras(
        items: Vec<(&str, toml::Value)>,
    ) -> camel_api::datasource::DatasourceConfig {
        camel_api::datasource::DatasourceConfig {
            db_url: "ws://localhost:8000".into(),
            provider: Some("surrealdb".into()),
            max_connections: None,
            min_connections: None,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra: items.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    #[test]
    fn apply_timeouts_override_sets_configured_values() {
        let ds = datasource_with_extras(vec![("connect_timeout_ms", toml::Value::Integer(250))]);
        let updated = apply_datasource_timeouts(SurrealDbEndpointConfig::default(), &ds)
            .expect("a positive integer extra must be accepted");
        assert_eq!(
            updated.connect_timeout,
            std::time::Duration::from_millis(250)
        );
        assert_eq!(
            updated.operation_timeout,
            std::time::Duration::from_secs(30),
            "an absent operation bound must keep the default"
        );
    }

    #[test]
    fn apply_timeouts_zero_extra_errors_naming_option() {
        let ds = datasource_with_extras(vec![("operation_timeout_ms", toml::Value::Integer(0))]);
        let err = apply_datasource_timeouts(SurrealDbEndpointConfig::default(), &ds)
            .expect_err("operation_timeout_ms = 0 must be rejected");
        assert!(
            err.to_string().contains("operation_timeout_ms"),
            "error must name the offending option: {err}"
        );
    }

    #[test]
    fn create_endpoint_rejects_zero_timeout_extra() {
        struct StubCatalog {
            config: camel_api::datasource::DatasourceConfig,
        }

        impl camel_api::datasource::DatasourceCatalog for StubCatalog {
            fn get_config(&self, name: &str) -> Option<camel_api::datasource::DatasourceConfig> {
                (name == "ws-ds").then(|| self.config.clone())
            }
            fn get_pool<'a>(&'a self, name: &'a str) -> camel_api::datasource::GetPoolFuture<'a> {
                Box::pin(async move {
                    Err(CamelError::Config(format!(
                        "StubCatalog has no pool for '{name}'"
                    )))
                })
            }
            fn register_factory(
                &self,
                _kind: &str,
                _factory: Arc<dyn camel_api::datasource::PoolFactory>,
            ) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let catalog = Arc::new(StubCatalog {
            config: datasource_with_extras(vec![("operation_timeout_ms", toml::Value::Integer(0))]),
        });
        let c = SurrealDbComponent::with_catalog(catalog);
        let ctx = NoOpComponentContext;
        let err = match c.create_endpoint("surrealdb:query?datasource=ws-ds", &ctx) {
            Ok(_) => panic!("an invalid timeout extra must fail endpoint creation"),
            Err(err) => err,
        };
        assert!(
            matches!(err, CamelError::EndpointCreationFailed(_)),
            "expected EndpointCreationFailed, got {err:?}"
        );
        assert!(
            err.to_string().contains("operation_timeout_ms"),
            "error must name the offending option: {err}"
        );
    }
}
