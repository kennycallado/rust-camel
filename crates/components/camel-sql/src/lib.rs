pub mod bundle;
pub mod config;
pub mod consumer;
pub mod endpoint;
pub mod headers;
pub mod health;
pub mod pool_factory;
pub mod producer;
pub mod query;
pub(crate) mod utils;

use std::sync::Arc;

use camel_api::datasource::DatasourceCatalog;
use camel_component_api::CamelError;
use camel_component_api::UriConfig;
use camel_component_api::{Component, Endpoint};
use sqlx::AnyPool;
use tokio::sync::OnceCell;

pub use bundle::SqlBundle;
pub use config::{
    PollStrategy, ProcessingStrategy, SqlEndpointConfig, SqlGlobalConfig, SqlOutputType,
    TransactionMode,
};
pub use health::SqlHealthCheck;

type SharedPool = Arc<OnceCell<Arc<AnyPool>>>;

pub struct SqlComponent {
    config: Option<SqlGlobalConfig>,
    catalog: Option<Arc<dyn DatasourceCatalog>>,
}

impl SqlComponent {
    pub fn new() -> Self {
        Self {
            config: None,
            catalog: None,
        }
    }

    pub fn with_config(config: SqlGlobalConfig) -> Self {
        Self {
            config: Some(config),
            catalog: None,
        }
    }

    pub fn with_optional_config(config: Option<SqlGlobalConfig>) -> Self {
        Self {
            config,
            catalog: None,
        }
    }

    pub fn with_config_and_catalog(
        config: SqlGlobalConfig,
        catalog: Arc<dyn DatasourceCatalog>,
    ) -> Self {
        Self {
            config: Some(config),
            catalog: Some(catalog),
        }
    }

    pub fn catalog(&self) -> Option<&Arc<dyn DatasourceCatalog>> {
        self.catalog.as_ref()
    }
}

impl Default for SqlComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for SqlComponent {
    fn scheme(&self) -> &str {
        "sql"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let mut config = SqlEndpointConfig::from_uri(uri)?;

        if config.datasource_name.is_some() && self.catalog.is_none() {
            return Err(CamelError::Config(
                "datasource parameter requires catalog — no datasource catalog configured".into(),
            ));
        }

        if let Some(ref ds_name) = config.datasource_name
            && let Some(ref catalog) = self.catalog
        {
            let ds_config = catalog.get_config(ds_name).ok_or_else(|| {
                CamelError::Config(format!("datasource '{}' not found in catalog", ds_name))
            })?;
            config.db_url = ds_config.db_url;
        }

        if let Some(ref global_config) = self.config {
            config.apply_defaults(global_config);
        }
        config.resolve_defaults();
        let pool: SharedPool = Arc::new(OnceCell::new());
        let health_check = SqlHealthCheck::new(
            Arc::clone(&pool),
            self.catalog.clone(),
            config.datasource_name.clone(),
        );
        ctx.register_current_route_health_check(Arc::new(health_check));

        if config.datasource_name.is_some()
            && let Some(ref catalog) = self.catalog
        {
            Ok(Box::new(endpoint::SqlEndpoint::new_with_pool_and_catalog(
                uri.to_string(),
                config,
                pool,
                Arc::clone(catalog),
            )))
        } else {
            Ok(Box::new(endpoint::SqlEndpoint::new_with_pool(
                uri.to_string(),
                config,
                pool,
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::Component;
    use camel_component_api::NoOpComponentContext;

    #[test]
    fn test_component_scheme() {
        let c = SqlComponent::new();
        assert_eq!(c.scheme(), "sql");
    }

    #[test]
    fn test_component_creates_endpoint() {
        let c = SqlComponent::new();
        let ctx = NoOpComponentContext;
        let ep = c.create_endpoint("sql:select 1?db_url=postgres://localhost/test", &ctx);
        assert!(ep.is_ok());
    }

    #[test]
    fn test_component_rejects_wrong_scheme() {
        let c = SqlComponent::new();
        let ctx = NoOpComponentContext;
        let ep = c.create_endpoint("redis://localhost", &ctx);
        assert!(ep.is_err());
    }

    #[test]
    fn test_endpoint_uri() {
        let c = SqlComponent::new();
        let ctx = NoOpComponentContext;
        let ep = c
            .create_endpoint("sql:select 1?db_url=postgres://localhost/test", &ctx)
            .unwrap();
        assert_eq!(ep.uri(), "sql:select 1?db_url=postgres://localhost/test");
    }

    #[test]
    fn test_component_with_global_config() {
        let global = SqlGlobalConfig::default().with_max_connections(20);
        let c = SqlComponent::with_config(global);
        let ctx = NoOpComponentContext;
        // Verify the component can create endpoints with global config applied
        assert_eq!(c.scheme(), "sql");
        let ep = c.create_endpoint("sql:select 1?db_url=postgres://localhost/test", &ctx);
        assert!(ep.is_ok());
    }

    #[test]
    fn test_global_config_applied_to_endpoint() {
        // Verify that when URI does NOT set pool params, global config fills them in.
        // Tests the same logic as create_endpoint: from_uri + apply_defaults + resolve_defaults.
        let global = SqlGlobalConfig::default()
            .with_max_connections(20)
            .with_min_connections(3)
            .with_idle_timeout_secs(600)
            .with_max_lifetime_secs(3600);
        let mut cfg =
            config::SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test")
                .unwrap();
        cfg.apply_defaults(&global);
        cfg.resolve_defaults();
        assert_eq!(cfg.max_connections, Some(20));
        assert_eq!(cfg.min_connections, Some(3));
        assert_eq!(cfg.idle_timeout_secs, Some(600));
        assert_eq!(cfg.max_lifetime_secs, Some(3600));
    }

    #[test]
    fn test_uri_param_wins_over_global_config() {
        // Verify that URI-set pool params are NOT overridden by global config.
        let global = SqlGlobalConfig::default()
            .with_max_connections(20)
            .with_min_connections(3);
        let mut cfg = config::SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&maxConnections=99&minConnections=7",
        )
        .unwrap();
        cfg.apply_defaults(&global);
        cfg.resolve_defaults();
        assert_eq!(cfg.max_connections, Some(99)); // URI wins
        assert_eq!(cfg.min_connections, Some(7)); // URI wins
    }
}
