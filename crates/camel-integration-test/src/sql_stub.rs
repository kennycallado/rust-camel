//! Shared cfg(test) SQLite stub-catalog support (bd rc-mu3aq).
//!
//! One home for the stub [`PoolFactory`], `sqlite_catalog`, and
//! `seed` helpers previously duplicated module-privately by
//! `sql_action_test.rs`, `sql_validate_test.rs`, and `runner_test.rs`
//! (r_glm review of task 3.2): the shared-cache URL, the
//! `max_connections(1)` single-connection incantation, and the
//! driver-install call must not drift between copies.
//!
//! Tests name their own tables: the shared cache is process-wide, so
//! parallel tests must not collide inside the one database.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use camel_api::datasource::{
    CheckFuture, CreatePoolFuture, DatasourceCatalog, DatasourceConfig, DatasourceHandle,
    PoolFactory,
};
use camel_api::error::CamelError;
use camel_api::lifecycle::HealthStatus;
use camel_core::datasource::RuntimeDatasourceCatalog;

use crate::sql_action::{SqlAction, execute_sql_prepare};

/// The shared-cache in-memory URL every sql test uses.
const DB_URL: &str = "sqlite::memory:?cache=shared";

struct StubPoolFactory;

impl PoolFactory for StubPoolFactory {
    fn create<'a>(&'a self, config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
        Box::pin(async move {
            // AnyPool connect fails without the compiled-in drivers
            // registered (camel-sql pool_factory.rs precedent).
            // max_connections(1) keeps all statements on one
            // connection: a private :memory: DB per connection would
            // otherwise split CREATE/INSERT state (camel-sql test
            // precedent: consumer.rs, health.rs, producer.rs).
            sqlx::any::install_default_drivers();
            let pool = sqlx::any::AnyPoolOptions::new()
                .max_connections(1)
                .connect(&config.db_url)
                .await
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;
            Ok(Arc::new(pool) as Arc<dyn Any + Send + Sync>)
        })
    }

    fn check<'a>(&'a self, _handle: &'a DatasourceHandle) -> CheckFuture<'a> {
        Box::pin(async { HealthStatus::Healthy })
    }

    fn supported_schemes(&self) -> &[&str] {
        &["sqlite"]
    }

    fn name(&self) -> &'static str {
        "stub"
    }
}

/// One catalog with a single `name` datasource over the shared
/// in-memory SQLite database (the `sql_action_test` pattern).
pub(crate) fn sqlite_catalog(name: &str) -> Arc<dyn DatasourceCatalog> {
    let mut configs = HashMap::new();
    configs.insert(
        name.to_string(),
        DatasourceConfig {
            db_url: DB_URL.to_string(),
            provider: None,
            max_connections: None,
            min_connections: None,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra: HashMap::new(),
        },
    );
    let catalog = RuntimeDatasourceCatalog::new(configs);
    assert!(
        catalog
            .register_factory("sqlite", Arc::new(StubPoolFactory))
            .is_ok(),
        "stub factory registration failed"
    );
    Arc::new(catalog)
}

/// Seeds `stmts` through the real prepare executor; a seed failure is
/// a test-harness defect, never the subject under test, so it panics
/// with the executor's own error text.
pub(crate) async fn seed(catalog: &Arc<dyn DatasourceCatalog>, datasource: &str, stmts: &[&str]) {
    let action = SqlAction {
        datasource: datasource.to_string(),
        prepare: stmts.iter().map(|stmt| stmt.to_string()).collect(),
    };
    if let Err(err) = execute_sql_prepare(catalog, &action).await {
        panic!("seed failed: {err}");
    }
}
