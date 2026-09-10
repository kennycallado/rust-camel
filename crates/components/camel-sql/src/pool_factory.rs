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

/// True for sqlite URLs whose database lives in memory: the bare
/// `:memory:` host forms and the named shared-cache form
/// (`sqlite:file:memdb_x?mode=memory&cache=shared`).
fn is_sqlite_memory_url(url: &str) -> bool {
    let lowered = url.to_lowercase();
    lowered.starts_with("sqlite::memory:")
        || lowered.starts_with("sqlite://:memory:")
        || (lowered.starts_with("sqlite:") && lowered.contains("mode=memory"))
}

/// How long `close` waits for in-flight connections to finish their
/// async close after `pool.close()` resolved (sqlx 0.8.6 leaves them
/// behind; bd rc-ywwz9).
const IN_FLIGHT_DRAIN_WAIT: Duration = Duration::from_secs(10);
/// Poll interval for that wait.
const IN_FLIGHT_DRAIN_POLL: Duration = Duration::from_millis(5);

pub struct SqlPoolFactory;

impl PoolFactory for SqlPoolFactory {
    fn create<'a>(&'a self, config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
        Box::pin(async move {
            // Install all compiled-in sqlx drivers so AnyPool can resolve them.
            // This is idempotent; safe to call multiple times.
            sqlx::any::install_default_drivers();

            let max_conn = config.max_connections.unwrap_or(5);
            // A `min_connections` maintainer on an in-memory sqlite pool
            // fights the die-with-boot contract: sqlx 0.8.6's
            // `try_min_connections` re-opens connections without checking
            // `is_closed`, so a maintained pool can resurrect a connection
            // after `close()` drains it and keep a named shared-cache
            // database alive into the next boot in the same process
            // (bd rc-ywwz9). Memory pools therefore never arm the
            // maintainer, explicit setting included.
            let min_conn = if is_sqlite_memory_url(&config.db_url) {
                if config.min_connections.is_some_and(|m| m > 0) {
                    // log-policy: outside-contract
                    tracing::info!("datasource pool: min_connections ignored for in-memory sqlite");
                }
                0
            } else {
                config.min_connections.unwrap_or(1)
            };
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
            // idle connections; subsequent acquire calls fail closed.
            pool.close().await;
            // But `close().await` resolving does NOT mean the pool is
            // empty (sqlx 0.8.6, verified by probe, bd rc-ywwz9): its
            // acquire loop only blocks when every permit is held, so a
            // connection still checked out inside a spawned
            // `return_to_pool` task is left closing asynchronously —
            // and it keeps a named shared-cache memory database alive
            // into the next boot in the same process whenever the
            // worker thread's close ack lags under load. Wait for the
            // pool to actually reach size 0 (those tasks close their
            // connection before returning; the min-connections clamp in
            // `create` guarantees nothing resurrects it), bounded; a
            // stall is an error — the pool would not be empty and the
            // shutdown deadline still bounds the overall wait.
            let drain_deadline = std::time::Instant::now() + IN_FLIGHT_DRAIN_WAIT;
            while pool.size() > 0 {
                if std::time::Instant::now() >= drain_deadline {
                    // The convergence loop drains normal runs well inside
                    // the bound (idle leftovers on the first extra pass,
                    // in-flight closes within a few polls), so reaching
                    // the cap means the pool genuinely did not drain —
                    // report it: shutdown must not silently succeed while
                    // a connection can keep a named shared-cache memory
                    // database alive into the next boot (bd rc-ywwz9).
                    return Err(CamelError::ProcessorError(format!(
                        "datasource '{}': pool did not drain within {}s ({} connection(s) \
                         still open) — the database may outlive its boot",
                        handle.name,
                        IN_FLIGHT_DRAIN_WAIT.as_secs(),
                        pool.size()
                    )));
                }
                // Yield so in-flight `return_to_pool` tasks progress, then
                // drain again: a `close()` pass empties the idle queue
                // (acked closes), the sleep lets checked-out connections
                // finish their own async close. Both leftover shapes from
                // the sqlx race converge here.
                tokio::time::sleep(IN_FLIGHT_DRAIN_POLL).await;
                pool.close().await;
            }
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
