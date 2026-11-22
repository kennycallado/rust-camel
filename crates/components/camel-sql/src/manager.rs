//! Datasource-scoped [`TransactionManager`] for joined `sql:` steps
//! (sqltrans-exec step 9).
//!
//! One manager is registered per catalog datasource at bundle boot. Pool
//! resolution is LAZY: the manager holds the boot's [`DatasourceCatalog`] and
//! resolves the datasource's pool through the catalog's existing `OnceCell`
//! path, exactly like the producer's named-datasource path — boot never
//! connects. Opening a session is the session's admission sequence
//! ([`SqlTxSession::open_on_pool`]): checkout + the one-time hygiene
//! `ROLLBACK`, with the discard guard armed at checkout. `BEGIN` is never sent
//! here (lazy begin, driven by [`camel_api::transaction::TransactionSession::begin`]).
//!
//! The port owns no block deadline: the segment computes `block_abs` at block
//! entry and bounds the `open_session()` await itself, reporting `Timeout`
//! with elapsed measured from entry. The manager therefore arms only its
//! acquire deadline and passes an effectively-unbounded block instant to the
//! session's `min(block_abs, acquire_abs)` admission race.
//!
//! [`DatasourceCatalog`]: camel_api::datasource::DatasourceCatalog

use std::sync::Arc;
use std::time::Duration;

use camel_api::datasource::DatasourceCatalog;
use camel_api::transaction::{
    OpenSessionFuture, TransactionError, TransactionManager, TransactionSession,
};
use sqlx::AnyPool;
use tokio::time::Instant;

use crate::session::SqlTxSession;

/// Bound for connection acquisition reported by [`TransactionManager::acquire_deadline`].
///
/// `DatasourceConfig` exposes no `acquire_timeout` field and
/// [`crate::pool_factory::SqlPoolFactory`] never overrides the driver default,
/// so the effective bound is sqlx's pool default (30 s, covering acquire and
/// connect).
pub const DEFAULT_ACQUIRE_DEADLINE: Duration = Duration::from_secs(30);

/// Block instant used when only the acquire deadline is armed.
///
/// The block deadline belongs to the segment (the port carries none); a
/// day-long bound is unreachable before any block timeout and keeps the
/// session's biased admission race on the acquire arm.
const NO_BLOCK_DEADLINE: Duration = Duration::from_secs(24 * 60 * 60);

/// [`TransactionManager`] serving one catalog datasource.
///
/// The registry keys the manager by [`TransactionManager::datasource`]; every
/// typed error the session raises already names that datasource.
pub struct SqlTransactionManager {
    datasource: String,
    /// The boot's single datasource catalog: the pool is resolved through its
    /// `OnceCell` path on first open, never at registration.
    catalog: Arc<dyn DatasourceCatalog>,
    acquire_deadline: Duration,
}

impl SqlTransactionManager {
    /// Manager for `datasource`, resolving its pool through `catalog`.
    #[must_use]
    pub fn new(datasource: impl Into<String>, catalog: Arc<dyn DatasourceCatalog>) -> Self {
        Self {
            datasource: datasource.into(),
            catalog,
            acquire_deadline: DEFAULT_ACQUIRE_DEADLINE,
        }
    }

    /// Override the acquire deadline (tests and embedders that own their pool
    /// options).
    #[must_use]
    pub fn with_acquire_deadline(mut self, acquire_deadline: Duration) -> Self {
        self.acquire_deadline = acquire_deadline;
        self
    }

    /// Open a session bounded by `block_abs` (admission + hygiene).
    async fn open_at(
        &self,
        block_abs: Instant,
    ) -> Result<Arc<dyn TransactionSession>, TransactionError> {
        let handle = self
            .catalog
            .get_pool(&self.datasource)
            .await
            .map_err(|error| self.unavailable(&error))
            .and_then(|handle| {
                handle
                    .downcast::<AnyPool>()
                    .map_err(|e| self.unavailable(&e))
            })?;
        let session = SqlTxSession::open_on_pool(
            self.datasource.as_str(),
            &handle,
            block_abs,
            self.acquire_deadline,
        )
        .await?;
        Ok(Arc::new(session))
    }

    /// Defensive mapping for an unresolvable datasource pool.
    ///
    /// Load validation resolved this manager already, so a catalog/pool
    /// failure here means the datasource cannot serve the block. The frozen
    /// error family has no "datasource unavailable" variant; a missing manager
    /// is the fail-closed shape (never an auto-commit fallback).
    fn unavailable(&self, error: &impl std::fmt::Display) -> TransactionError {
        // log-policy: system-broken
        tracing::error!(
            datasource = %self.datasource,
            error = %error,
            "transaction manager could not resolve the datasource pool"
        );
        TransactionError::ManagerMissing {
            datasource: self.datasource.clone(),
        }
    }

    /// Admission at an explicit block instant: the seam the step-6 residual
    /// fixture drives to pin the block-deadline-first `Timeout`.
    #[cfg(test)]
    pub(crate) async fn open_session_at(
        &self,
        block_abs: Instant,
    ) -> Result<Arc<dyn TransactionSession>, TransactionError> {
        self.open_at(block_abs).await
    }
}

impl TransactionManager for SqlTransactionManager {
    fn datasource(&self) -> &str {
        &self.datasource
    }

    fn open_session(&self) -> OpenSessionFuture<'_> {
        Box::pin(async move { self.open_at(Instant::now() + NO_BLOCK_DEADLINE).await })
    }

    fn acquire_deadline(&self) -> Duration {
        self.acquire_deadline
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::sync::Arc;

    use camel_api::datasource::{DatasourceConfig, DatasourceHandle, GetPoolFuture};
    use camel_api::error::CamelError;
    use sqlx::AnyPool;
    use sqlx::any::AnyPoolOptions;

    use super::*;

    /// Catalog holding one pool, served for every name.
    struct FixedCatalog {
        pool: AnyPool,
        config: DatasourceConfig,
    }

    impl DatasourceCatalog for FixedCatalog {
        fn get_config(&self, _name: &str) -> Option<DatasourceConfig> {
            Some(self.config.clone())
        }

        fn get_pool<'a>(&'a self, name: &'a str) -> GetPoolFuture<'a> {
            let pool = self.pool.clone();
            let name = name.to_string();
            Box::pin(async move {
                let inner: Arc<dyn Any + Send + Sync> = Arc::new(pool);
                Ok(DatasourceHandle::new(name, "sqlx".to_string(), inner))
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

    fn datasource_config() -> DatasourceConfig {
        DatasourceConfig {
            db_url: "postgres://camel:camel@127.0.0.1:5432/camel".to_string(),
            provider: Some("sqlx".to_string()),
            max_connections: Some(1),
            min_connections: Some(0),
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra: std::collections::HashMap::new(),
        }
    }

    /// A single-connection in-memory pool. SQLite never reaches the server
    /// legs the fixtures block on (acquire/hygiene), so it is enough to hold
    /// the pool's only connection and pin the admission deadlines.
    async fn sqlite_pool() -> AnyPool {
        sqlx::any::install_default_drivers();
        AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            // allow-test-wait: test-local in-memory sqlite connect; immediate, bounded by construction
            .await
            .expect("in-memory sqlite pool")
    }

    fn manager(pool: &AnyPool, acquire_deadline: Duration) -> SqlTransactionManager {
        SqlTransactionManager::new(
            "pg",
            Arc::new(FixedCatalog {
                pool: pool.clone(),
                config: datasource_config(),
            }),
        )
        .with_acquire_deadline(acquire_deadline)
    }

    #[tokio::test]
    async fn manager_reports_datasource_and_default_acquire_deadline() {
        let pool = sqlite_pool().await;
        let catalog: Arc<dyn DatasourceCatalog> = Arc::new(FixedCatalog {
            pool,
            config: datasource_config(),
        });
        let manager = SqlTransactionManager::new("pg", catalog);
        assert_eq!(manager.datasource(), "pg");
        assert_eq!(
            manager.acquire_deadline(),
            DEFAULT_ACQUIRE_DEADLINE,
            "the datasource config exposes no acquire_timeout; sqlx's 30 s default is the bound"
        );
        assert_eq!(DEFAULT_ACQUIRE_DEADLINE, Duration::from_secs(30));
    }

    /// Frozen admission clause, manager tier: the acquire deadline is the
    /// first to elapse with a real pool → typed `PoolExhausted` naming the
    /// datasource, the configured pool size, and the waited milliseconds.
    #[tokio::test]
    async fn manager_open_session_pool_exhausted_names_datasource() {
        let pool = sqlite_pool().await;
        // Hold the pool's only connection so checkout cannot resolve.
        let _held = pool.acquire().await.expect("held connection"); // allow-test-wait: test-local sqlite pool's only connection is free; acquire resolves immediately
        tokio::time::pause();
        let manager = manager(&pool, Duration::from_millis(10));

        let error = match manager.open_session().await {
            Err(error) => error,
            Ok(_) => panic!("an exhausted pool must fail the open"),
        };

        match error {
            TransactionError::PoolExhausted {
                datasource,
                max_connections,
                waited_ms,
            } => {
                assert_eq!(datasource, "pg");
                assert_eq!(max_connections, 1);
                // Tokio's paused clock rounds a deadline up by <1 ms after
                // `pause()` lands mid-tick; the bound is the 10 ms deadline.
                assert!(
                    (10..=11).contains(&waited_ms),
                    "waited_ms must measure the 10 ms acquire deadline, got {waited_ms}"
                );
            }
            other => panic!("expected typed PoolExhausted, got {other:?}"),
        }
    }

    /// Step-6 residual, manager tier: the block-deadline-first admission
    /// reports typed `Timeout` with elapsed measured from entry (block entry
    /// is the `open_session_at` call), never `PoolExhausted`.
    #[tokio::test]
    async fn manager_open_session_block_deadline_first_reports_timeout() {
        let pool = sqlite_pool().await;
        let _held = pool.acquire().await.expect("held connection"); // allow-test-wait: test-local sqlite pool's only connection is free; acquire resolves immediately
        tokio::time::pause();
        // The manager arms only its acquire deadline in production; this seam
        // supplies the segment's block instant to pin the Timeout arm.
        let manager = manager(&pool, Duration::from_secs(60));
        let entry = Instant::now();

        let error = match manager
            .open_session_at(entry + Duration::from_millis(30))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("the block deadline must win the admission race"),
        };

        match error {
            TransactionError::Timeout {
                datasource,
                elapsed_ms,
            } => {
                assert_eq!(datasource, "pg");
                // Elapsed is measured from block entry (the `open_session_at`
                // call); the paused clock may round the deadline up <1 ms.
                assert!(
                    (30..=31).contains(&elapsed_ms),
                    "elapsed_ms must measure the 30 ms block deadline, got {elapsed_ms}"
                );
            }
            other => panic!("expected typed Timeout (never PoolExhausted), got {other:?}"),
        }
    }
}
