use async_trait::async_trait;
use camel_api::datasource::DatasourceCatalog;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use sqlx::AnyPool;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

trait SqlHealthProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;
}

struct SqlPoolProbe {
    pool: Arc<OnceCell<Arc<AnyPool>>>,
    catalog: Option<Arc<dyn DatasourceCatalog>>,
    datasource_name: Option<String>,
}

impl SqlPoolProbe {
    fn new(
        pool: Arc<OnceCell<Arc<AnyPool>>>,
        catalog: Option<Arc<dyn DatasourceCatalog>>,
        datasource_name: Option<String>,
    ) -> Self {
        Self {
            pool,
            catalog,
            datasource_name,
        }
    }
}

impl SqlHealthProbe for SqlPoolProbe {
    fn probe(&self) -> ProbeFuture {
        let pool = Arc::clone(&self.pool);
        let catalog = self.catalog.clone();
        let datasource_name = self.datasource_name.clone();
        Box::pin(async move {
            // Resolve the effective pool. When the route uses a named datasource,
            // the producer's own OnceCell is never populated — writes (and the
            // health probe) must go through the catalog's shared pool. This
            // mirrors SqlProducer::check_connection / call pool resolution.
            let pool: Arc<AnyPool> = if let (Some(catalog), Some(ds_name)) =
                (catalog.as_ref(), datasource_name.as_deref())
            {
                let handle = catalog.get_pool(ds_name).await?;
                handle.downcast::<AnyPool>()?
            } else {
                pool.get()
                    .ok_or_else(|| {
                        CamelError::ProcessorError(
                            "SQL connection pool not initialized".to_string(),
                        )
                    })?
                    .clone()
            };

            sqlx::query("SELECT 1")
                .execute(pool.as_ref())
                .await
                .map_err(|e| {
                    CamelError::ProcessorError(format!("SQL health check failed: {}", e))
                })?;

            Ok(())
        })
    }
}

pub struct SqlHealthCheck {
    probe: Arc<dyn SqlHealthProbe>,
    timeout: Duration,
}

impl SqlHealthCheck {
    pub fn new(
        pool: Arc<OnceCell<Arc<AnyPool>>>,
        catalog: Option<Arc<dyn DatasourceCatalog>>,
        datasource_name: Option<String>,
    ) -> Self {
        Self {
            probe: Arc::new(SqlPoolProbe::new(pool, catalog, datasource_name)),
            timeout: Duration::from_secs(2),
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn SqlHealthProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for SqlHealthCheck {
    fn name(&self) -> &str {
        "sql"
    }

    async fn check(&self) -> CheckResult {
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "SELECT 1 timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::HealthStatus;
    use camel_api::datasource::{
        DatasourceCatalog, DatasourceConfig, DatasourceHandle, GetPoolFuture, PoolFactory,
    };
    use std::any::Any;

    /// Catalog mock that hands out a pre-built pool for a single named datasource.
    /// Mirrors how a real route configured with `?datasource=cartodb` resolves its pool.
    struct CatalogWithPool {
        name: String,
        pool: Arc<AnyPool>,
    }

    impl DatasourceCatalog for CatalogWithPool {
        fn get_config(&self, name: &str) -> Option<DatasourceConfig> {
            (name == self.name).then(|| DatasourceConfig {
                db_url: "sqlite::memory:".into(),
                provider: None,
                max_connections: None,
                min_connections: None,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                ssl_mode: None,
                ssl_root_cert: None,
                ssl_cert: None,
                ssl_key: None,
                extra: Default::default(),
            })
        }

        fn get_pool<'a>(&'a self, name: &'a str) -> GetPoolFuture<'a> {
            Box::pin(async move {
                if name != self.name {
                    return Err(CamelError::Config(format!("unknown datasource '{}'", name)));
                }
                let inner: Arc<dyn Any + Send + Sync> = self.pool.clone();
                Ok(DatasourceHandle::new(
                    name.to_string(),
                    "sql".to_string(),
                    inner,
                ))
            })
        }

        fn register_factory(
            &self,
            _kind: &str,
            _factory: Arc<dyn PoolFactory>,
        ) -> Result<(), CamelError> {
            Err(CamelError::Config(
                "mock: register_factory not supported".into(),
            ))
        }
    }

    struct MockProbe {
        responder: Arc<dyn Fn() -> ProbeFuture + Send + Sync>,
    }

    impl MockProbe {
        fn new<F>(f: F) -> Self
        where
            F: Fn() -> ProbeFuture + Send + Sync + 'static,
        {
            Self {
                responder: Arc::new(f),
            }
        }
    }

    impl SqlHealthProbe for MockProbe {
        fn probe(&self) -> ProbeFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn sql_health_check_healthy_when_probe_succeeds() {
        let probe = Arc::new(MockProbe::new(|| Box::pin(async { Ok(()) })));
        let check = SqlHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "sql");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn sql_health_check_unhealthy_when_probe_fails() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ProcessorError(
                    "simulated sql error".to_string(),
                ))
            })
        }));
        let check = SqlHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "sql");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated sql error"))
        );
    }

    #[tokio::test]
    async fn sql_health_check_unhealthy_when_probe_times_out() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
        }));
        let check = SqlHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));

        let result = check.check().await;

        assert_eq!(result.name, "sql");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("SELECT 1 timed out"));
    }

    // Regression: when a route uses `?datasource=<name>`, the producer's own
    // pool OnceCell is never populated (writes go via the catalog's pool), so
    // the health check must resolve the effective pool from the catalog.
    // Otherwise it reports a permanent false-negative "pool not initialized".
    #[tokio::test]
    async fn sql_health_check_healthy_via_datasource_when_pool_cell_empty() {
        sqlx::any::install_default_drivers();
        let pool = Arc::new(
            sqlx::any::AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .expect("sqlite pool"),
        );
        let catalog: Arc<dyn DatasourceCatalog> = Arc::new(CatalogWithPool {
            name: "cartodb".to_string(),
            pool,
        });
        // Empty pool cell — simulates the datasource route where the producer's
        // own OnceCell stays unset because real writes go through the catalog.
        let empty_cell: Arc<OnceCell<Arc<AnyPool>>> = Arc::new(OnceCell::new());

        let check = SqlHealthCheck::new(empty_cell, Some(catalog), Some("cartodb".to_string()));

        let result = check.check().await;

        assert_eq!(
            result.status,
            HealthStatus::Healthy,
            "health check must resolve pool from catalog when datasource_name is set"
        );
    }
}
