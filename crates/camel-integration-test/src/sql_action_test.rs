//! Executor tests for the scenario `sql:` action (bd rc-25lup.1).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(all(test, feature = "sql"))]`. The executor drives a real
//! in-memory SQLite pool through a stub [`PoolFactory`] so the prepare
//! statements run against an actual datasource; the stub keeps every
//! statement on one connection so CREATE/INSERT state is not split
//! across private per-connection `:memory:` databases (camel-sql test
//! precedent: consumer.rs, health.rs, producer.rs).

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

fn sqlite_catalog(name: &str) -> Arc<dyn DatasourceCatalog> {
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
    catalog
        .register_factory("sqlite", Arc::new(StubPoolFactory))
        .unwrap();
    Arc::new(catalog)
}

fn action(datasource: &str, prepare: Vec<&str>) -> SqlAction {
    SqlAction {
        datasource: datasource.to_string(),
        prepare: prepare.into_iter().map(str::to_string).collect(),
    }
}

#[tokio::test]
async fn executor_seeds_and_stops_on_error() {
    let catalog = sqlite_catalog("appdb");
    let err = execute_sql_prepare(
        &catalog,
        &action(
            "appdb",
            vec![
                "CREATE TABLE t (v TEXT UNIQUE)",
                "INSERT INTO t VALUES ('a')",
                "INSERT INTO t VALUES ('a')",
            ],
        ),
    )
    .await
    .unwrap_err();
    assert!(err.contains("statement [2]"), "got: {err}");
    assert!(err.contains("appdb"), "got: {err}");
    assert!(!err.contains("sqlite::memory:"), "got: {err}");
}

#[tokio::test]
async fn executor_unknown_datasource_names_it_only() {
    let catalog = sqlite_catalog("appdb");
    let err = execute_sql_prepare(
        &catalog,
        &action("missing", vec!["CREATE TABLE t (x INTEGER)"]),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "sql action: unknown datasource 'missing'");
}

#[tokio::test]
async fn executor_success_is_silent() {
    let catalog = sqlite_catalog("appdb");
    let result = execute_sql_prepare(
        &catalog,
        &action(
            "appdb",
            vec!["CREATE TABLE t (x INTEGER)", "INSERT INTO t VALUES (1)"],
        ),
    )
    .await;
    assert_eq!(result, Ok(()));
}
