use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use camel_api::datasource::{CheckFuture, CloseFuture, CreatePoolFuture};
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
                Err(e) => {
                    // log-policy: outside-contract
                    tracing::warn!(
                        "datasource '{}' health check failed: pool downcast error: {}",
                        handle.name,
                        e
                    );
                    HealthStatus::Unhealthy
                }
            }
        })
    }

    fn close<'a>(&'a self, handle: &'a DatasourceHandle) -> CloseFuture<'a> {
        Box::pin(async move {
            let pool = handle.downcast::<AnyPool>().map_err(|e| {
                CamelError::ProcessorError(format!(
                    "datasource '{}': pool close downcast failed: {}",
                    handle.name, e
                ))
            })?;
            // sqlx `close()` is infallible: it signals closure and drains
            // the connections; subsequent acquire calls fail closed.
            pool.close().await;
            Ok(())
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

    #[tokio::test]
    async fn sql_pool_factory_close_closes_the_pool() {
        let f = SqlPoolFactory;
        let cfg = DatasourceConfig {
            db_url: "sqlite::memory:?cache=shared".into(),
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
        let inner = f.create(&cfg).await.unwrap();
        let pool = Arc::downcast::<AnyPool>(Arc::clone(&inner)).unwrap();
        let handle = DatasourceHandle::new("appdb".into(), f.name().into(), Arc::clone(&inner));

        f.close(&handle).await.unwrap();
        assert!(
            pool.is_closed(),
            "factory close must drain the sqlx pool (bd rc-25lup.4)"
        );
    }

    /// Probe (bd rc-25lup.4 review): does the named shared-memory URI
    /// form genuinely share state across pooled connections? The
    /// answer decides whether a lingering boot's connection could leak
    /// rows into a later boot over the same URI.
    #[tokio::test]
    async fn named_shared_memory_uri_probe() {
        use sqlx::Row;

        let f = SqlPoolFactory;
        let cfg = DatasourceConfig {
            db_url: "sqlite:file:memdb_probe?mode=memory&cache=shared".into(),
            provider: None,
            max_connections: Some(3),
            min_connections: None,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra: std::collections::HashMap::new(),
        };
        let inner = f.create(&cfg).await.unwrap();
        let pool = Arc::downcast::<AnyPool>(Arc::clone(&inner)).unwrap();

        sqlx::query("CREATE TABLE probe (v TEXT)")
            .execute(&*pool)
            .await
            .expect("create");
        // Force a second connection: hold one acquire while running the
        // INSERT on another.
        let conn1 = pool.acquire().await.expect("conn1");
        sqlx::query("INSERT INTO probe VALUES ('x')")
            .execute(&*pool)
            .await
            .expect("insert on a second connection");
        drop(conn1);

        let row = sqlx::query("SELECT COUNT(*) FROM probe")
            .fetch_one(&*pool)
            .await
            .expect("count");
        let n = row.try_get::<i64, usize>(0).expect("count i64");
        assert_eq!(
            n, 1,
            "named shared memory URI must share across pool connections"
        );
    }
}
