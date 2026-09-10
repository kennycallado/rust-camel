//! Executor tests for the scenario `sql:` action (bd rc-25lup.1).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(all(test, feature = "sql"))]`. The executor drives a real
//! in-memory SQLite pool through the shared stub catalog
//! (`crate::sql_stub`, bd rc-mu3aq) so the prepare statements run
//! against an actual datasource; the stub keeps every statement on
//! one connection so CREATE/INSERT state is not split across private
//! per-connection `:memory:` databases (camel-sql test precedent:
//! consumer.rs, health.rs, producer.rs).

use crate::sql_action::{SqlAction, execute_sql_prepare};
use crate::sql_stub::sqlite_catalog;

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
