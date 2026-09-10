//! Executor tests for the `validate` sql target (bd rc-25lup.2,
//! task 3.1).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! plain `#[cfg(test)]` — it compiles in BOTH feature configurations:
//! the sql-catalog tests live in the `executor` module (feature
//! `sql`), the feature-off twin gate lives in the `twin` module. The
//! executor tests drive a real in-memory SQLite pool through a stub
//! [`PoolFactory`] (the `sql_action_test` pattern): the shared cache
//! keeps every statement on one in-memory database even when a
//! spawned task seeds through its own catalog clone, and each test
//! names its own table so parallel tests never collide inside the
//! process-wide shared-cache database.

/// The sql executor behaviors (feature `sql`): seeding through
/// [`crate::sql_action::execute_sql_prepare`], validating through
/// `sql_validate_action`, and the direct `any_row_to_tuple` unit.
#[cfg(feature = "sql")]
mod executor {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use camel_matchers::{CountBound, Expectation, RowsExpectation};
    use serde_json::{Value, json};
    use sqlx::{Column, Row};

    use crate::document::SqlTarget;
    use crate::runner::{ScenarioFailure, any_row_to_tuple, sql_validate_action};
    use crate::sql_stub::{seed, sqlite_catalog};

    /// A sql validate target for `datasource` running `query`.
    fn sql_target(datasource: &str, query: &str) -> SqlTarget {
        SqlTarget {
            datasource: datasource.to_string(),
            query: query.to_string(),
        }
    }

    /// A concrete-rows expectation with the optional by-name
    /// projection.
    fn rows_expectation(
        columns: Option<Vec<&str>>,
        unordered: bool,
        rows: Vec<Vec<Expectation>>,
    ) -> RowsExpectation {
        RowsExpectation {
            columns: columns.map(|names| names.into_iter().map(String::from).collect()),
            unordered,
            rows: Some(rows),
            bound: None,
        }
    }

    /// A row-count-bound expectation (no projection, no rows).
    fn bound_expectation(bound: CountBound) -> RowsExpectation {
        RowsExpectation {
            columns: None,
            unordered: false,
            rows: None,
            bound: Some(bound),
        }
    }

    /// Runs one validation and turns `Ok(())` into a panic: the
    /// callers of this helper all assert failures, and a surprise
    /// pass is the regression signal.
    fn expect_failure(result: Result<(), ScenarioFailure>) -> ScenarioFailure {
        match result {
            Err(failure) => failure,
            Ok(()) => panic!("expected a scenario failure, got Ok"),
        }
    }

    /// The exact ordered rows of a freshly seeded two-row table pass
    /// on the first (and only) snapshot, no deadline needed.
    #[tokio::test]
    async fn ordered_rows_pass_immediately() {
        let catalog = sqlite_catalog("appdb");
        seed(
            &catalog,
            "appdb",
            &[
                "CREATE TABLE t_ordered (id INTEGER, name TEXT)",
                "INSERT INTO t_ordered VALUES (1, 'alice')",
                "INSERT INTO t_ordered VALUES (2, 'bob')",
            ],
        )
        .await;
        let target = sql_target("appdb", "SELECT id, name FROM t_ordered ORDER BY id");
        let expected = rows_expectation(
            Some(vec!["id", "name"]),
            false,
            vec![
                vec![
                    Expectation::Equals(json!(1)),
                    Expectation::Equals(json!("alice")),
                ],
                vec![
                    Expectation::Equals(json!(2)),
                    Expectation::Equals(json!("bob")),
                ],
            ],
        );
        let outcome = sql_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));
    }

    /// An `unordered` rows expectation matches the same rows
    /// declared in reverse against a query with no ORDER BY: the
    /// bipartite matching finds the perfect pairing.
    #[tokio::test]
    async fn unordered_rows_match_reorder() {
        let catalog = sqlite_catalog("appdb");
        seed(
            &catalog,
            "appdb",
            &[
                "CREATE TABLE t_unordered (id INTEGER, name TEXT)",
                "INSERT INTO t_unordered VALUES (1, 'alice')",
                "INSERT INTO t_unordered VALUES (2, 'bob')",
            ],
        )
        .await;
        let target = sql_target("appdb", "SELECT id, name FROM t_unordered");
        let expected = rows_expectation(
            None,
            true,
            vec![
                vec![
                    Expectation::Equals(json!(2)),
                    Expectation::Equals(json!("bob")),
                ],
                vec![
                    Expectation::Equals(json!(1)),
                    Expectation::Equals(json!("alice")),
                ],
            ],
        );
        let outcome = sql_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));
    }

    /// Cell wildcards compose with `equals: null`: the `ignore`
    /// wildcard matches any id, and only a SQL NULL satisfies an
    /// `Equals(null)` cell.
    #[tokio::test]
    async fn wildcard_and_equals_null() {
        let catalog = sqlite_catalog("appdb");
        seed(
            &catalog,
            "appdb",
            &[
                "CREATE TABLE t_null (id INTEGER, note TEXT)",
                "INSERT INTO t_null VALUES (7, NULL)",
            ],
        )
        .await;
        let target = sql_target("appdb", "SELECT id, note FROM t_null");
        let expected = rows_expectation(
            None,
            false,
            vec![vec![Expectation::Any, Expectation::Equals(Value::Null)]],
        );
        let outcome = sql_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));
    }

    /// Without a deadline one snapshot decides every bound shape:
    /// `atLeast` 2 holds against 3 rows, `count` 2 does not.
    #[tokio::test]
    async fn count_bounds_decide_final_snapshot() {
        let catalog = sqlite_catalog("appdb");
        seed(
            &catalog,
            "appdb",
            &[
                "CREATE TABLE t_count (id INTEGER)",
                "INSERT INTO t_count VALUES (1)",
                "INSERT INTO t_count VALUES (2)",
                "INSERT INTO t_count VALUES (3)",
            ],
        )
        .await;
        let target = sql_target("appdb", "SELECT id FROM t_count");
        let at_least = bound_expectation(CountBound::AtLeast(2));
        let outcome = sql_validate_action(0, &target, &at_least, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));

        let exact = bound_expectation(CountBound::Exact(2));
        let failure =
            expect_failure(sql_validate_action(0, &target, &exact, None, Some(&catalog)).await);
        let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(detail.contains("expected 2"), "got: {detail}");
        assert!(detail.contains("actual 3 rows"), "got: {detail}");
    }

    /// A snapshot above an `atMost` ceiling fails immediately, well
    /// inside the window: 2 rows against `atMost: 1` with a 300 ms
    /// deadline returns in under 250 ms carrying the rendered bound.
    #[tokio::test]
    async fn ceiling_breach_fails_immediately() {
        let catalog = sqlite_catalog("appdb");
        seed(
            &catalog,
            "appdb",
            &[
                "CREATE TABLE t_ceil (id INTEGER)",
                "INSERT INTO t_ceil VALUES (1)",
                "INSERT INTO t_ceil VALUES (2)",
            ],
        )
        .await;
        let target = sql_target("appdb", "SELECT id FROM t_ceil");
        let expected = bound_expectation(CountBound::AtMost(1));
        let started = Instant::now();
        let failure = expect_failure(
            sql_validate_action(
                0,
                &target,
                &expected,
                Some(Duration::from_millis(300)),
                Some(&catalog),
            )
            .await,
        );
        let elapsed = started.elapsed();
        let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(detail.contains("expected at most 1"), "got: {detail}");
        assert!(
            elapsed < Duration::from_millis(250),
            "ceiling breach took {elapsed:?}, expected immediate"
        );
    }

    /// A row appearing mid-window satisfies a `rows` assertion: the
    /// poll never settles early, but the FINAL snapshot at expiry
    /// sees the row the spawned task inserted at ~150 ms.
    #[tokio::test]
    async fn deadline_poll_passes_when_row_appears() {
        let catalog = sqlite_catalog("appdb");
        seed(&catalog, "appdb", &["CREATE TABLE t_appears (id INTEGER)"]).await;
        let seeder = Arc::clone(&catalog);
        let insert = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            seed(&seeder, "appdb", &["INSERT INTO t_appears VALUES (42)"]).await;
        });
        let target = sql_target("appdb", "SELECT id FROM t_appears");
        let expected = rows_expectation(None, false, vec![vec![Expectation::Equals(json!(42))]]);
        let outcome = sql_validate_action(
            0,
            &target,
            &expected,
            Some(Duration::from_secs(2)),
            Some(&catalog),
        )
        .await;
        assert_eq!(outcome, Ok(()));
        // The spawned insert must have succeeded — a skipped seed
        // would otherwise mask a poll bug with a false pass.
        let joined = insert.await;
        assert!(
            matches!(joined, Ok(())),
            "spawned insert failed: {joined:?}"
        );
    }

    /// A declared projection over an initially EMPTY result set must
    /// not abort the poll: zero rows carry no column names, so the
    /// projection stays vacuously satisfied on every empty snapshot
    /// and the window polls until the seeded row appears (the r_glm
    /// FIX-REQUIRED regression: an empty snapshot must never surface
    /// `unknown projection column`).
    #[tokio::test]
    async fn empty_result_with_projection_polls_until_seeded() {
        let catalog = sqlite_catalog("appdb");
        seed(
            &catalog,
            "appdb",
            &["CREATE TABLE t_proj_appears (id INTEGER, name TEXT)"],
        )
        .await;
        let seeder = Arc::clone(&catalog);
        let insert = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            seed(
                &seeder,
                "appdb",
                &["INSERT INTO t_proj_appears VALUES (9, 'iris')"],
            )
            .await;
        });
        let target = sql_target("appdb", "SELECT id, name FROM t_proj_appears");
        let expected = rows_expectation(
            Some(vec!["id", "name"]),
            false,
            vec![vec![
                Expectation::Equals(json!(9)),
                Expectation::Equals(json!("iris")),
            ]],
        );
        let outcome = sql_validate_action(
            0,
            &target,
            &expected,
            Some(Duration::from_secs(2)),
            Some(&catalog),
        )
        .await;
        assert_eq!(outcome, Ok(()));
        let joined = insert.await;
        assert!(
            matches!(joined, Ok(())),
            "spawned insert failed: {joined:?}"
        );
    }

    /// No deadline + a declared projection over an empty result set:
    /// the single empty snapshot decides through the rows matcher —
    /// the mismatch renders `actual 0 rows`, never a projection
    /// error.
    #[tokio::test]
    async fn empty_result_no_deadline_reports_row_count() {
        let catalog = sqlite_catalog("appdb");
        seed(
            &catalog,
            "appdb",
            &["CREATE TABLE t_proj_empty (id INTEGER, name TEXT)"],
        )
        .await;
        let target = sql_target("appdb", "SELECT id, name FROM t_proj_empty");
        let expected = rows_expectation(
            Some(vec!["id", "name"]),
            false,
            vec![vec![
                Expectation::Equals(json!(1)),
                Expectation::Equals(json!("alice")),
            ]],
        );
        let outcome = sql_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        let failure = expect_failure(outcome);
        let ScenarioFailure::ValidationMismatch { detail, .. } = &failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(
            detail.contains("actual 0 rows"),
            "detail must render the actual row count: {detail}"
        );
        assert!(
            !detail.contains("unknown projection column"),
            "empty result must not surface a projection error: {detail}"
        );
    }

    /// SQL state is non-monotone: a row set that matches at t=0 and
    /// is deleted at ~100 ms must NOT settle early — the final
    /// snapshot at the 400 ms deadline decides, and it fails.
    #[tokio::test]
    async fn no_early_settle_matching_snapshot_deleted() {
        let catalog = sqlite_catalog("appdb");
        seed(
            &catalog,
            "appdb",
            &[
                "CREATE TABLE t_del (id INTEGER)",
                "INSERT INTO t_del VALUES (9)",
            ],
        )
        .await;
        let seeder = Arc::clone(&catalog);
        let delete = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            seed(&seeder, "appdb", &["DELETE FROM t_del"]).await;
        });
        let target = sql_target("appdb", "SELECT id FROM t_del");
        let expected = rows_expectation(None, false, vec![vec![Expectation::Equals(json!(9))]]);
        let failure = expect_failure(
            sql_validate_action(
                0,
                &target,
                &expected,
                Some(Duration::from_millis(400)),
                Some(&catalog),
            )
            .await,
        );
        let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(detail.contains("actual 0 rows"), "got: {detail}");
        let joined = delete.await;
        assert!(
            matches!(joined, Ok(())),
            "spawned delete failed: {joined:?}"
        );
    }

    /// The by-name projection reorders columns into declaration
    /// order, and a declared name the result does not carry fails
    /// closed with the exact unknown-column detail.
    #[tokio::test]
    async fn column_projection_reorders_and_unknown_fails() {
        let catalog = sqlite_catalog("appdb");
        seed(
            &catalog,
            "appdb",
            &[
                "CREATE TABLE t_proj (id INTEGER, name TEXT)",
                "INSERT INTO t_proj VALUES (1, 'alice')",
            ],
        )
        .await;
        let target = sql_target("appdb", "SELECT id, name FROM t_proj");

        let reordered = rows_expectation(
            Some(vec!["name", "id"]),
            false,
            vec![vec![
                Expectation::Equals(json!("alice")),
                Expectation::Equals(json!(1)),
            ]],
        );
        let outcome = sql_validate_action(0, &target, &reordered, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));

        let unknown = rows_expectation(
            Some(vec!["id", "missing"]),
            false,
            vec![vec![Expectation::Equals(json!(1)), Expectation::Any]],
        );
        let failure =
            expect_failure(sql_validate_action(0, &target, &unknown, None, Some(&catalog)).await);
        let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(
            detail.contains("unknown projection column `missing`"),
            "got: {detail}"
        );
    }

    /// The mismatch detail names the datasource and the actual row
    /// count but never a seeded cell value and never the db_url —
    /// row payloads and credential bytes stay out of verdict text.
    #[tokio::test]
    async fn mismatch_detail_elides_cells_and_db_url() {
        let catalog = sqlite_catalog("appdb");
        seed(
            &catalog,
            "appdb",
            &[
                "CREATE TABLE t_mm (id INTEGER, name TEXT)",
                "INSERT INTO t_mm VALUES (1, 'alice')",
            ],
        )
        .await;
        let target = sql_target("appdb", "SELECT id, name FROM t_mm");
        let expected = rows_expectation(
            Some(vec!["id", "name"]),
            false,
            vec![vec![
                Expectation::Equals(json!(2)),
                Expectation::Equals(json!("bob")),
            ]],
        );
        let failure =
            expect_failure(sql_validate_action(0, &target, &expected, None, Some(&catalog)).await);
        let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(detail.contains("appdb"), "got: {detail}");
        assert!(detail.contains("actual 1 rows"), "got: {detail}");
        assert!(!detail.contains("alice"), "got: {detail}");
        assert!(!detail.contains("sqlite::memory:"), "got: {detail}");
    }

    /// A driver failure (missing table) is an apparatus-class
    /// `ActionTransport` naming the datasource with a sanitized
    /// error: the db_url bytes never reach the failure text.
    #[tokio::test]
    async fn driver_error_names_datasource_sanitized() {
        let catalog = sqlite_catalog("appdb");
        let target = sql_target("appdb", "SELECT * FROM t_missing_tbl");
        let expected = bound_expectation(CountBound::Exact(1));
        let failure =
            expect_failure(sql_validate_action(0, &target, &expected, None, Some(&catalog)).await);
        let ScenarioFailure::ActionTransport { source, .. } = failure else {
            panic!("expected ActionTransport, got {failure:?}");
        };
        let crate::adapters::TransportError::Other { message } = source else {
            panic!("expected TransportError::Other, got {source:?}");
        };
        assert!(message.contains("appdb"), "got: {message}");
        assert!(!message.contains("sqlite::memory:"), "got: {message}");
    }

    /// Blob cells map JSON-first (the `reply_bytes_value` precedent):
    /// a JSON text blob reads as the parsed structured value, and a
    /// non-JSON blob with invalid UTF-8 bytes reads as the lossy
    /// string.
    #[tokio::test]
    async fn blob_maps_json_first() {
        let catalog = sqlite_catalog("appdb");
        let handle = match catalog.get_pool("appdb").await {
            Ok(handle) => handle,
            Err(err) => panic!("pool acquisition failed: {err}"),
        };
        let pool = match handle.downcast::<sqlx::AnyPool>() {
            Ok(pool) => pool,
            Err(err) => panic!("pool downcast failed: {err}"),
        };
        let rows =
            match sqlx::query("SELECT CAST('{\"a\":1,\"b\":[2]}' AS BLOB) AS j, X'8081' AS b")
                .fetch_all(&*pool)
                .await
            {
                Ok(rows) => rows,
                Err(err) => panic!("blob select failed: {err}"),
            };
        let Some(row) = rows.first() else {
            panic!("blob select returned no rows");
        };
        let names: Vec<String> = row
            .columns()
            .iter()
            .map(|column| column.name().to_string())
            .collect();
        let tuple = match any_row_to_tuple(row, &names) {
            Ok(tuple) => tuple,
            Err(err) => panic!("any_row_to_tuple failed: {err}"),
        };
        assert_eq!(tuple[0], json!({"a": 1, "b": [2]}));
        assert_eq!(
            tuple[1],
            Value::String(String::from_utf8_lossy(&[0x80, 0x81]).into_owned())
        );
    }

    /// An unknown datasource name fails closed naming it, exactly
    /// like the `sql:` prepare action.
    #[tokio::test]
    async fn unknown_datasource_fails_closed() {
        let catalog = sqlite_catalog("appdb");
        let target = sql_target("nope", "SELECT 1");
        let expected = bound_expectation(CountBound::Exact(1));
        let failure =
            expect_failure(sql_validate_action(0, &target, &expected, None, Some(&catalog)).await);
        let ScenarioFailure::ActionTransport { source, .. } = failure else {
            panic!("expected ActionTransport, got {failure:?}");
        };
        let crate::adapters::TransportError::Other { message } = source else {
            panic!("expected TransportError::Other, got {source:?}");
        };
        assert!(
            message.contains("unknown datasource 'nope'"),
            "got: {message}"
        );
    }
}

/// The feature-off twin gate (the partner no-http precedent): the
/// document parser rejects sql validate targets without the feature,
/// so the twin only needs to name the gate when reached directly.
#[cfg(not(feature = "sql"))]
mod twin {
    use camel_matchers::RowsExpectation;

    use crate::document::SqlTarget;
    use crate::runner::{ScenarioFailure, sql_validate_action};

    /// The twin returns the exact gate detail for any arguments —
    /// the byte-exact string the harness docs promise.
    #[tokio::test]
    async fn feature_off_twin_names_gate() {
        let target = SqlTarget {
            datasource: "appdb".to_string(),
            query: "SELECT 1".to_string(),
        };
        let expected = RowsExpectation {
            columns: None,
            unordered: false,
            rows: Some(Vec::new()),
            bound: None,
        };
        let failure = match sql_validate_action(0, &target, &expected, None, None).await {
            Err(failure) => failure,
            Ok(()) => panic!("expected the feature gate failure, got Ok"),
        };
        let ScenarioFailure::ValidationMismatch { action: 0, detail } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert_eq!(detail, "sql validation requires the `sql` feature");
    }
}
