use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use camel_api::datasource::{CheckFuture, CreatePoolFuture};
use camel_api::datasource::{DatasourceConfig, DatasourceHandle, PoolFactory};
use camel_api::error::CamelError;
use camel_api::lifecycle::HealthStatus;
use sqlx::AnyPool;
use sqlx::any::AnyPoolOptions;

use crate::config::{enrich_db_url_with_ssl_params, redact_db_url};

pub struct SqlPoolFactory;

impl PoolFactory for SqlPoolFactory {
    fn create<'a>(&'a self, config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
        Box::pin(async move {
            // Install all compiled-in sqlx drivers so AnyPool can resolve them.
            // This is idempotent; safe to call multiple times.
            sqlx::any::install_default_drivers();

            let max_conn = config.max_connections.unwrap_or(5);
            let min_conn = config.min_connections.unwrap_or(1);
            let idle_timeout = Duration::from_secs(config.idle_timeout_secs.unwrap_or(300));
            let max_lifetime = Duration::from_secs(config.max_lifetime_secs.unwrap_or(1800));

            let db_url = enrich_db_url_with_ssl_params(
                &config.db_url,
                config.ssl_mode.as_deref(),
                config.ssl_root_cert.as_deref(),
                config.ssl_cert.as_deref(),
                config.ssl_key.as_deref(),
            )?;

            let pool = AnyPoolOptions::new()
                .max_connections(max_conn)
                .min_connections(min_conn)
                .idle_timeout(idle_timeout)
                .max_lifetime(max_lifetime)
                .connect(&db_url)
                .await
                .map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "failed to create datasource pool ({}): {}",
                        redact_db_url(&config.db_url),
                        e
                    ))
                })?;

            tracing::info!("datasource pool created: max_connections={}", max_conn);
            Ok(Arc::new(pool) as Arc<dyn Any + Send + Sync>)
        })
    }

    fn check<'a>(&'a self, handle: &'a DatasourceHandle) -> CheckFuture<'a> {
        Box::pin(async move {
            match handle.downcast::<AnyPool>() {
                Ok(pool) => match sqlx::query("SELECT 1").execute(&*pool).await {
                    Ok(_) => HealthStatus::Healthy,
                    Err(e) => {
                        // log-policy: outside-contract
                        tracing::warn!("datasource '{}' health check failed: {}", handle.name, e);
                        HealthStatus::Unhealthy
                    }
                },
                Err(_) => HealthStatus::Unhealthy,
            }
        })
    }

    fn supported_schemes(&self) -> &[&str] {
        &["postgres", "postgresql", "mysql", "sqlite"]
    }

    fn name(&self) -> &'static str {
        "sqlx"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_pool_factory_name() {
        let f = SqlPoolFactory;
        assert_eq!(f.name(), "sqlx");
    }

    #[test]
    fn sql_pool_factory_supported_schemes() {
        let f = SqlPoolFactory;
        assert!(f.supported_schemes().contains(&"postgres"));
        assert!(f.supported_schemes().contains(&"mysql"));
        assert!(f.supported_schemes().contains(&"sqlite"));
    }

    #[test]
    fn sql_pool_factory_matches_postgres() {
        let f = SqlPoolFactory;
        let cfg = DatasourceConfig {
            db_url: "postgres://localhost/test".into(),
            provider: None,
            max_connections: None,
            min_connections: None,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra: std::collections::HashMap::new(),
        };
        assert!(f.matches(&cfg));
    }
}
