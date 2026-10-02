//! Executor tests for the `validate` surreal target
//! (surreal-state-tier tasks 2.3 and 2.4).
//!
//! Unit-test module of the lib target, declared in `runner.rs` under
//! plain `#[cfg(test)]` — it compiles in BOTH feature configurations:
//! the mem-engine tests live in the `executor` module (feature
//! `surreal`) and its `mapping` submodule (direct unit tests of the
//! fail-closed type law), the feature-off twin gate lives in the
//! `twin` module, and the load-error sibling pin lives in the ungated
//! `load` module (the task 1.2 grammar runs in every build). The
//! executor tests drive a real embedded `mem://` instance through a
//! stub [`PoolFactory`] (the `sql_stub` pattern): every `create`
//! spawns a fresh auth-free instance isolated per connect, so tests
//! never share state and each names its own catalog. The deadline
//! poll tests (task 2.4) follow the sql suite's sanctioned
//! spawn-and-sleep idiom: the writer/deleter runs at ~150 ms and the
//! validator polls to expiry over the same catalog.

#[cfg(feature = "surreal")]
mod executor {
    use std::any::Any;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use camel_api::datasource::{
        CheckFuture, CreatePoolFuture, DatasourceCatalog, DatasourceConfig, DatasourceHandle,
        PoolFactory,
    };
    use camel_api::error::CamelError;
    use camel_api::lifecycle::HealthStatus;
    use camel_core::datasource::RuntimeDatasourceCatalog;
    use camel_matchers::{CountBound, Expectation, RowsExpectation};

    use crate::adapters::{PartnerRouter, TransportError};
    use crate::document::{
        RouteSource, ScenarioAction, ScenarioDocument, ScenarioTarget, SurrealTarget,
        ValidateExpectation,
    };
    use crate::runner::{ScenarioFailure, ScenarioVars, run_scenario, surreal_validate_action};
    use crate::surreal_action::{SurrealAction, execute_surreal_prepare};

    /// A `mem://` pool factory: every `create` connects a fresh
    /// auth-free embedded instance (the surrealdb pool factory's mem
    /// law) and selects the default `test` namespace and database.
    struct StubSurrealFactory;

    impl PoolFactory for StubSurrealFactory {
        fn create<'a>(&'a self, config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
            Box::pin(async move {
                let client: surrealdb::Surreal<surrealdb::engine::any::Any> =
                    surrealdb::engine::any::connect(&config.db_url)
                        .await
                        .map_err(|e| CamelError::ProcessorError(e.to_string()))?;
                client
                    .use_ns("test")
                    .await
                    .map_err(|e| CamelError::ProcessorError(e.to_string()))?;
                client
                    .use_db("test")
                    .await
                    .map_err(|e| CamelError::ProcessorError(e.to_string()))?;
                Ok(Arc::new(client) as Arc<dyn Any + Send + Sync>)
            })
        }

        fn check<'a>(&'a self, _handle: &'a DatasourceHandle) -> CheckFuture<'a> {
            Box::pin(async { HealthStatus::Healthy })
        }

        fn supported_schemes(&self) -> &[&str] {
            &["mem"]
        }

        fn name(&self) -> &'static str {
            "stub-surreal"
        }
    }

    /// One catalog with a single `name` datasource over `mem://`,
    /// resolved through the provider key the real boot registers
    /// (the e2e `Camel.toml` shape).
    fn surreal_catalog(name: &str) -> Arc<dyn DatasourceCatalog> {
        surreal_catalog_with_url(name, "mem://")
    }

    /// [`surreal_catalog`] with an explicit `db_url`: the redaction
    /// tests plant a credential sentinel in the URL the way a real
    /// config would carry one (`mem://` tolerates query params, the
    /// engine ignores the keys it does not know).
    fn surreal_catalog_with_url(name: &str, db_url: &str) -> Arc<dyn DatasourceCatalog> {
        let mut configs = HashMap::new();
        configs.insert(
            name.to_string(),
            DatasourceConfig {
                db_url: db_url.to_string(),
                provider: Some("surrealdb".into()),
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
                .register_factory("surrealdb", Arc::new(StubSurrealFactory))
                .is_ok(),
            "stub factory registration failed"
        );
        Arc::new(catalog)
    }

    /// Seeds `stmts` through the real prepare executor; a seed failure
    /// is a test-harness defect, never the subject under test, so it
    /// panics with the executor's own error text.
    async fn seed(catalog: &Arc<dyn DatasourceCatalog>, datasource: &str, stmts: &[&str]) {
        let action = SurrealAction {
            datasource: datasource.to_string(),
            prepare: stmts.iter().map(|stmt| stmt.to_string()).collect(),
        };
        if let Err(err) = execute_surreal_prepare(catalog, &action).await {
            panic!("seed failed: {err}");
        }
    }

    /// A surreal validate target for `datasource` running `query`.
    fn surreal_target(datasource: &str, query: &str) -> SurrealTarget {
        SurrealTarget {
            datasource: datasource.to_string(),
            query: query.to_string(),
        }
    }

    /// A concrete-rows expectation with the declared projection (the
    /// loader law: surreal row patterns always carry `columns`).
    fn rows_expectation(
        columns: Vec<&str>,
        unordered: bool,
        rows: Vec<Vec<Expectation>>,
    ) -> RowsExpectation {
        RowsExpectation {
            columns: Some(columns.into_iter().map(String::from).collect()),
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

    /// The exact ordered rows of a freshly seeded two-record table
    /// pass on the first (and only) snapshot, no deadline needed. The
    /// numeric `num` field is user-defined, so it projects as a JSON
    /// number; the record ids are explicit and project as `table:key`
    /// strings.
    #[tokio::test]
    async fn ordered_rows_pass_immediately() {
        let catalog = surreal_catalog("statedb");
        seed(
            &catalog,
            "statedb",
            &[
                "CREATE user:1 SET num = 1, name = 'alice'",
                "CREATE user:2 SET num = 2, name = 'bob'",
            ],
        )
        .await;
        let target = surreal_target("statedb", "SELECT num, name FROM user ORDER BY num ASC");
        let expected = rows_expectation(
            vec!["num", "name"],
            false,
            vec![
                vec![
                    Expectation::Equals(serde_json::json!(1)),
                    Expectation::Equals(serde_json::json!("alice")),
                ],
                vec![
                    Expectation::Equals(serde_json::json!(2)),
                    Expectation::Equals(serde_json::json!("bob")),
                ],
            ],
        );
        let outcome = surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));
    }

    /// An `unordered` rows expectation matches the same two records
    /// declared in reverse against a query with no `ORDER BY`: the
    /// bipartite matching finds the perfect pairing.
    #[tokio::test]
    async fn unordered_rows_match_reorder() {
        let catalog = surreal_catalog("statedb");
        seed(
            &catalog,
            "statedb",
            &[
                "CREATE user:1 SET num = 1, name = 'alice'",
                "CREATE user:2 SET num = 2, name = 'bob'",
            ],
        )
        .await;
        let target = surreal_target("statedb", "SELECT num, name FROM user");
        let expected = rows_expectation(
            vec!["num", "name"],
            true,
            vec![
                vec![
                    Expectation::Equals(serde_json::json!(2)),
                    Expectation::Equals(serde_json::json!("bob")),
                ],
                vec![
                    Expectation::Equals(serde_json::json!(1)),
                    Expectation::Equals(serde_json::json!("alice")),
                ],
            ],
        );
        let outcome = surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));
    }

    /// A stored null projects as `null` (never dropped, never a
    /// wildcard): the `ignore` wildcard matches any `num`, and only a
    /// stored NULL satisfies an `Equals(null)` cell.
    #[tokio::test]
    async fn wildcard_ignore_matches_any_cell() {
        let catalog = surreal_catalog("statedb");
        seed(
            &catalog,
            "statedb",
            &["CREATE user:1 SET num = 7, name = null"],
        )
        .await;
        let target = surreal_target("statedb", "SELECT num, name FROM user");
        let expected = rows_expectation(
            vec!["num", "name"],
            false,
            vec![vec![
                Expectation::Any,
                Expectation::Equals(serde_json::Value::Null),
            ]],
        );
        let outcome = surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));
    }

    /// Without a deadline one snapshot decides the bound shapes:
    /// `atLeast` 2 holds against 3 records.
    #[tokio::test]
    async fn count_bound_passes_on_record_count() {
        let catalog = surreal_catalog("statedb");
        seed(
            &catalog,
            "statedb",
            &[
                "CREATE user:1 SET num = 1",
                "CREATE user:2 SET num = 2",
                "CREATE user:3 SET num = 3",
            ],
        )
        .await;
        let target = surreal_target("statedb", "SELECT id FROM user");
        let expected = bound_expectation(CountBound::AtLeast(2));
        let outcome = surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));
    }

    /// The by-name projection reorders fields into declaration order:
    /// the query selects `id, name` and the expectation names
    /// `[name, id]`.
    #[tokio::test]
    async fn field_projection_reorders_by_name() {
        let catalog = surreal_catalog("statedb");
        seed(&catalog, "statedb", &["CREATE user:1 SET name = 'alice'"]).await;
        let target = surreal_target("statedb", "SELECT id, name FROM user");
        let expected = rows_expectation(
            vec!["name", "id"],
            false,
            vec![vec![
                Expectation::Equals(serde_json::json!("alice")),
                Expectation::Equals(serde_json::json!("user:1")),
            ]],
        );
        let outcome = surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));
    }

    /// A record id projects as its `table:key` string form.
    #[tokio::test]
    async fn record_id_projects_as_string() {
        let catalog = surreal_catalog("statedb");
        seed(&catalog, "statedb", &["CREATE user:1 SET num = 1"]).await;
        let target = surreal_target("statedb", "SELECT id FROM user");
        let expected = rows_expectation(
            vec!["id"],
            false,
            vec![vec![Expectation::Equals(serde_json::json!("user:1"))]],
        );
        let outcome = surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        assert_eq!(outcome, Ok(()));
    }

    /// A declared projection field the result objects do not carry
    /// fails closed naming it: never a silently dropped column.
    #[tokio::test]
    async fn unknown_projection_field_fails_closed() {
        let catalog = surreal_catalog("statedb");
        seed(
            &catalog,
            "statedb",
            &["CREATE user:1 SET num = 1, name = 'alice'"],
        )
        .await;
        let target = surreal_target("statedb", "SELECT id, num, name FROM user");
        let expected = rows_expectation(
            vec!["id", "missing"],
            false,
            vec![vec![Expectation::Any, Expectation::Any]],
        );
        let failure = expect_failure(
            surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await,
        );
        let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(detail.contains("missing"), "must name the field: {detail}");
    }

    /// A field holding a value kind outside the mapping (a geometry)
    /// fails closed naming the field and its SurrealQL kind.
    #[tokio::test]
    async fn unknown_value_kind_fails_closed() {
        let catalog = surreal_catalog("statedb");
        seed(
            &catalog,
            "statedb",
            &[
                "CREATE user:1 SET loc = <geometry<point>> { type: 'Point', coordinates: \
               [-0.118092, 51.509865] }",
            ],
        )
        .await;
        let target = surreal_target("statedb", "SELECT loc FROM user");
        let expected = rows_expectation(vec!["loc"], false, vec![vec![Expectation::Any]]);
        let failure = expect_failure(
            surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await,
        );
        let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(detail.contains("loc"), "must name the field: {detail}");
        assert!(detail.contains("geometry"), "must name the kind: {detail}");
    }

    /// An unsupported kind nested inside an array fails closed naming
    /// the field path and the nested kind: recursion never launders a
    /// kind the top-level mapping rejects.
    #[tokio::test]
    async fn nested_unsupported_kind_fails_closed() {
        let catalog = surreal_catalog("statedb");
        seed(
            &catalog,
            "statedb",
            &[
                "CREATE user:1 SET track = [<geometry<point>> { type: 'Point', coordinates: \
               [-0.118092, 51.509865] }]",
            ],
        )
        .await;
        let target = surreal_target("statedb", "SELECT track FROM user");
        let expected = rows_expectation(vec!["track"], false, vec![vec![Expectation::Any]]);
        let failure = expect_failure(
            surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await,
        );
        let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(
            detail.contains("track[0]"),
            "must name the nested field path: {detail}"
        );
        assert!(
            detail.contains("geometry"),
            "must name the nested kind: {detail}"
        );
    }

    /// The fail-closed backstop in the single-action loop: a surreal
    /// validate action with no catalog in hand ([`run_scenario`]
    /// always passes `None`) is an apparatus-class failure naming the
    /// missing catalog, never a silently skipped assertion. The
    /// document is constructed directly — the call path is the
    /// prepare-side pin's (`surreal_no_catalog_fails_closed`).
    #[tokio::test]
    async fn surreal_validate_no_catalog_fails_closed() {
        let doc = ScenarioDocument {
            source_path: std::path::PathBuf::new(),
            route_source: RouteSource::RouteFiles(vec!["routes.yaml".into()]),
            scenario: vec![ScenarioAction::Validate {
                target: ScenarioTarget::Surreal(surreal_target("statedb", "SELECT id FROM user")),
                expectation: ValidateExpectation::Rows(rows_expectation(
                    vec!["id"],
                    false,
                    vec![vec![Expectation::Any]],
                )),
                deadline: None,
                elapsed_at_least: None,
            }],
            partners: None,
            env: None,
            env_passthrough: None,
            profile: None,
            send_deadline: None,
            inbound: None,
            logs: None,
        };
        let router = PartnerRouter::new(BTreeMap::new());
        let mut vars = ScenarioVars::new();
        let failure = run_scenario(&doc, &router, &mut vars)
            .await
            .expect_err("the missing catalog must fail the scenario");
        let ScenarioFailure::ActionTransport { action, source } = failure else {
            panic!("expected ActionTransport, got {failure:?}");
        };
        assert_eq!(action, 0, "the failure must carry the action index");
        let TransportError::Other { message } = source else {
            panic!("expected TransportError::Other, got {source:?}");
        };
        assert!(
            message.contains("no datasource catalog"),
            "the failure must name the missing catalog: {message}"
        );
    }

    /// A record appearing mid-window satisfies a `rows` assertion
    /// (task 2.4): the poll never settles early, but the FINAL
    /// snapshot at expiry sees the record the spawned task writes at
    /// ~150 ms over the same catalog. The writer's join completes
    /// BEFORE the validate result is read — the join proves the write
    /// landed inside the deadline window, so the pass cannot come
    /// from the write never happening (spawn-scheduling independent:
    /// the first poll is immediate and empty, the write lands at
    /// ~150 ms, the decision happens at expiry).
    #[tokio::test]
    async fn deadline_poll_passes_when_record_appears() {
        let catalog = surreal_catalog("statedb");
        // The empty-table precondition: on surrealdb 3.2.4 a SELECT
        // from a MISSING table is a driver error, not an empty
        // result, so the table is defined up front (zero records) —
        // the first poll is immediate and observes the ABSENT record,
        // not a missing table.
        seed(&catalog, "statedb", &["DEFINE TABLE user"]).await;
        let writer_catalog = Arc::clone(&catalog);
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await; // allow-test-sleep: mid-window writer, the sql suite's sanctioned idiom — the ~150 ms write timing is the behavior under test (sql_validate_test.rs precedent)
            seed(
                &writer_catalog,
                "statedb",
                &["CREATE user SET num = 42, name = 'carol'"],
            )
            .await;
        });
        let target = surreal_target("statedb", "SELECT num, name FROM user");
        let expected = rows_expectation(
            vec!["num", "name"],
            false,
            vec![vec![
                Expectation::Equals(serde_json::json!(42)),
                Expectation::Equals(serde_json::json!("carol")),
            ]],
        );
        let validate_target = target.clone();
        let validate_expected = expected.clone();
        let validate_catalog = Arc::clone(&catalog);
        let validate = tokio::spawn(async move {
            surreal_validate_action(
                0,
                &validate_target,
                &validate_expected,
                Some(Duration::from_secs(2)),
                Some(&validate_catalog),
            )
            .await
        });
        // The write must have succeeded INSIDE the window: a skipped
        // seed would otherwise mask a poll bug with a false pass.
        let joined = tokio::time::timeout(Duration::from_secs(10), writer)
            .await
            .expect("writer joins (finite: 150 ms sleep + one seed op)");
        assert!(matches!(joined, Ok(())), "spawned write failed: {joined:?}");
        let outcome = match tokio::time::timeout(Duration::from_secs(10), validate).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(err)) => panic!("validate task failed: {err}"),
            Err(_) => panic!("validate join exceeded outer bound (internal deadline 2 s)"),
        };
        assert_eq!(outcome, Ok(()));
    }

    /// Surreal record sets are non-monotone: a record set that
    /// matches at t=0 and is deleted at ~150 ms must NOT settle early
    /// (task 2.4) — the final snapshot at the deadline decides, and
    /// it fails on the empty table. Presence at t=0 is proven by a
    /// preliminary no-deadline call, the spawned deleter's join
    /// completes BEFORE the validate result is read (deletion landed
    /// inside the window), so the mismatch cannot come from the
    /// delete never happening — the outcome is independent of spawn
    /// scheduling.
    #[tokio::test]
    async fn no_early_settle_final_snapshot_decides() {
        let catalog = surreal_catalog("statedb");
        seed(&catalog, "statedb", &["CREATE user:9 SET num = 9"]).await;
        let target = surreal_target("statedb", "SELECT num FROM user");
        let expected = rows_expectation(
            vec!["num"],
            false,
            vec![vec![Expectation::Equals(serde_json::json!(9))]],
        );
        // Presence proof: the immediate (no-deadline) call observes
        // the record on its single snapshot.
        let preliminary =
            surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await;
        assert_eq!(preliminary, Ok(()));
        let deleter_catalog = Arc::clone(&catalog);
        let deleter = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await; // allow-test-sleep: mid-window deleter, the sql suite's sanctioned idiom — the ~150 ms delete timing is the behavior under test (sql_validate_test.rs precedent)
            seed(&deleter_catalog, "statedb", &["DELETE user"]).await;
        });
        let validate_target = target.clone();
        let validate_expected = expected.clone();
        let validate_catalog = Arc::clone(&catalog);
        let validate = tokio::spawn(async move {
            surreal_validate_action(
                0,
                &validate_target,
                &validate_expected,
                Some(Duration::from_millis(400)),
                Some(&validate_catalog),
            )
            .await
        });
        let joined = tokio::time::timeout(Duration::from_secs(10), deleter)
            .await
            .expect("deleter joins (finite: 150 ms sleep + one seed op)");
        assert!(
            matches!(joined, Ok(())),
            "spawned delete failed: {joined:?}"
        );
        let outcome = match tokio::time::timeout(Duration::from_secs(10), validate).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(err)) => panic!("validate task failed: {err}"),
            Err(_) => panic!("validate join exceeded outer bound (internal deadline 400 ms)"),
        };
        let failure = expect_failure(outcome);
        let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(detail.contains("actual 0 rows"), "got: {detail}");
    }

    /// A snapshot above an `atMost` ceiling fails immediately, well
    /// inside the window (task 2.4): 2 records against `atMost: 1`
    /// with a 300 ms deadline returns in under 250 ms carrying the
    /// rendered bound.
    #[tokio::test]
    async fn ceiling_breach_fails_immediately() {
        let catalog = surreal_catalog("statedb");
        seed(
            &catalog,
            "statedb",
            &["CREATE user:1 SET num = 1", "CREATE user:2 SET num = 2"],
        )
        .await;
        let target = surreal_target("statedb", "SELECT id FROM user");
        let expected = bound_expectation(CountBound::AtMost(1));
        let started = Instant::now();
        let failure = expect_failure(
            surreal_validate_action(
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

    /// The mismatch detail names the datasource, the expected and
    /// actual row counts, and the projection field names, but never a
    /// seeded cell value and never the credential-bearing `db_url`
    /// (task 2.4 redaction, ADR-0051): the sentinel credential is
    /// planted in the config's `db_url` the way a real config would
    /// carry one.
    #[tokio::test]
    async fn mismatch_detail_elides_cells_and_db_url() {
        let catalog = surreal_catalog_with_url("statedb", "mem://?credential=s3cr3t-sentinel");
        seed(
            &catalog,
            "statedb",
            &["CREATE user:1 SET num = 1, name = 'alice'"],
        )
        .await;
        let target = surreal_target("statedb", "SELECT num, name FROM user");
        let expected = rows_expectation(
            vec!["num", "name"],
            false,
            vec![vec![
                Expectation::Equals(serde_json::json!(2)),
                Expectation::Equals(serde_json::json!("bob")),
            ]],
        );
        let failure = expect_failure(
            surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await,
        );
        let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert!(detail.contains("statedb"), "got: {detail}");
        assert!(detail.contains("expected 1 rows"), "got: {detail}");
        assert!(detail.contains("actual 1 rows"), "got: {detail}");
        assert!(detail.contains("num, name"), "got: {detail}");
        assert!(!detail.contains("s3cr3t-sentinel"), "got: {detail}");
        assert!(!detail.contains("alice"), "got: {detail}");
        assert!(!detail.contains("bob"), "got: {detail}");
    }

    /// A driver failure (a SurrealQL parse error that passes the
    /// load-time select-prefix gate and errors inside the response,
    /// which `.check()` surfaces) is an apparatus-class
    /// `ActionTransport` naming the datasource with a sanitized
    /// error: the db_url bytes never reach the failure text (task
    /// 2.4, ADR-0051).
    #[tokio::test]
    async fn driver_error_is_sanitized() {
        let catalog = surreal_catalog("statedb");
        let target = surreal_target("statedb", "SELECT FROM WHERE");
        let expected = bound_expectation(CountBound::Exact(1));
        let failure = expect_failure(
            surreal_validate_action(0, &target, &expected, None, Some(&catalog)).await,
        );
        let ScenarioFailure::ActionTransport { source, .. } = failure else {
            panic!("expected ActionTransport, got {failure:?}");
        };
        let TransportError::Other { message } = source else {
            panic!("expected TransportError::Other, got {source:?}");
        };
        assert!(message.contains("statedb"), "got: {message}");
        assert!(!message.contains("mem://"), "got: {message}");
    }
}

/// Direct unit tests of the fail-closed type law
/// ([`surreal_value_to_cell`]) for the arms a `mem://` seed cannot
/// reach deterministically: decimal and non-finite numbers, an
/// unsupported scalar kind, the nested object path, and the
/// record-id string form.
#[cfg(all(test, feature = "surreal"))]
mod mapping {
    use surrealdb::types::{
        Datetime, Decimal, Duration as SurrealDuration, Number, Object, Uuid, Value as Sv,
    };

    use crate::runner::{surreal_rows_to_tuples, surreal_value_to_cell};

    #[test]
    fn decimal_number_fails_closed() {
        let err = surreal_value_to_cell(&Sv::Number(Number::Decimal(Decimal::from(1))), "price")
            .expect_err("a decimal number must fail closed");
        assert!(err.contains("price"), "must name the field: {err}");
        assert!(err.contains("decimal"), "must name the kind: {err}");
    }

    #[test]
    fn non_finite_float_fails_closed() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = surreal_value_to_cell(&Sv::Number(Number::Float(bad)), "score")
                .expect_err("a non-finite float must fail closed");
            assert!(err.contains("score"), "must name the field: {err}");
        }
    }

    #[test]
    fn duration_fails_closed() {
        let err = surreal_value_to_cell(
            &Sv::Duration(SurrealDuration::from(std::time::Duration::from_secs(1))),
            "elapsed",
        )
        .expect_err("a duration must fail closed");
        assert!(err.contains("elapsed"), "must name the field: {err}");
        assert!(err.contains("duration"), "must name the kind: {err}");
    }

    #[test]
    fn nested_object_path_names_the_path() {
        let mut inner = Object::new();
        inner.insert(
            "inner",
            Sv::Duration(SurrealDuration::from(std::time::Duration::from_secs(1))),
        );
        let mut outer = Object::new();
        outer.insert("outer", Sv::Object(inner));
        let err = surreal_value_to_cell(&Sv::Object(outer), "meta")
            .expect_err("a nested unsupported kind must fail closed");
        assert!(
            err.contains("meta.outer.inner"),
            "must name the field path: {err}"
        );
    }

    #[test]
    fn record_id_maps_to_table_key_string() {
        // `CREATE user:1` stores a NUMERIC key, whose string form is
        // the unescaped `table:key` pair the expectations match (a
        // string key that would parse as a number renders escaped —
        // `user:`1`` — the SurrealQL literal form).
        let rid = surrealdb::types::RecordId::new("user", 1_i64);
        let cell = surreal_value_to_cell(&Sv::RecordId(rid), "id").expect("a record id must map");
        assert_eq!(cell, serde_json::json!("user:1"));
    }

    /// A UUID projects as its canonical hyphenated string form.
    #[test]
    fn uuid_projects_as_string() {
        let uuid = Uuid::new_v4();
        let cell = surreal_value_to_cell(&Sv::Uuid(uuid), "id").expect("a uuid must map");
        assert_eq!(cell, serde_json::json!(uuid.to_string()));
    }

    /// A datetime projects as its string form.
    #[test]
    fn datetime_projects_as_string() {
        let datetime = Datetime::from_timestamp(1_700_000_000, 0).expect("a valid timestamp");
        let cell = surreal_value_to_cell(&Sv::Datetime(datetime), "created_at")
            .expect("a datetime must map");
        assert_eq!(cell, serde_json::json!(datetime.to_string()));
    }

    #[test]
    fn rows_to_tuples_projects_declared_order() {
        let mut object = Object::new();
        object.insert("num", Sv::Number(Number::Int(1)));
        object.insert("name", Sv::String("alice".to_string()));
        let tuples = surreal_rows_to_tuples(
            vec![Sv::Object(object)],
            &["name".to_string(), "num".to_string()],
        )
        .expect("a carried field must project");
        assert_eq!(
            tuples,
            vec![vec![serde_json::json!("alice"), serde_json::json!(1)]]
        );
    }

    #[test]
    fn rows_to_tuples_absent_field_fails_closed() {
        let mut object = Object::new();
        object.insert("num", Sv::Number(Number::Int(1)));
        let err = surreal_rows_to_tuples(
            vec![Sv::Object(object)],
            &["num".to_string(), "missing".to_string()],
        )
        .expect_err("an absent field must fail closed");
        assert!(err.contains("missing"), "must name the field: {err}");
    }
}

/// The feature-off twin gate (the sql twin precedent): the document
/// grammar is ungated, so a well-formed surreal validate target parses
/// in every build; the twin only needs to name the gate when reached.
#[cfg(not(feature = "surreal"))]
mod twin {
    use camel_matchers::{Expectation, RowsExpectation};

    use crate::document::SurrealTarget;
    use crate::runner::{ScenarioFailure, surreal_validate_action};

    /// The twin returns the exact gate detail for a well-formed
    /// surreal validate target — the byte-exact string the harness
    /// docs promise.
    #[tokio::test]
    async fn surreal_validate_feature_off_names_gate() {
        let target = SurrealTarget {
            datasource: "statedb".to_string(),
            query: "SELECT id FROM user".to_string(),
        };
        let expected = RowsExpectation {
            columns: Some(vec!["id".to_string()]),
            unordered: false,
            rows: Some(vec![vec![Expectation::Any]]),
            bound: None,
        };
        let failure = match surreal_validate_action(0, &target, &expected, None, None).await {
            Err(failure) => failure,
            Ok(()) => panic!("expected the feature gate failure, got Ok"),
        };
        let ScenarioFailure::ValidationMismatch { action: 0, detail } = failure else {
            panic!("expected ValidationMismatch, got {failure:?}");
        };
        assert_eq!(detail, "surreal validation requires the `surreal` feature");
    }
}

/// The load-error sibling (surreal-state-tier task 1.2 pinned at the
/// e2e level): an expectation row whose width differs from the
/// declared `columns` never loads. Ungated — the grammar runs in
/// every build.
mod load {
    use crate::document::DocError;
    use crate::parse_scenario_document;

    #[test]
    fn row_length_mismatch_load_error_sibling() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("case.test.yaml");
        std::fs::write(
            &path,
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      surreal:
        datasource: statedb
        query: SELECT num, name FROM user
    expectation:
      columns: [num, name]
      rows:
      - [1, "alice", "extra"]
"#,
        )
        .expect("write case file");
        let err = parse_scenario_document(&path).expect_err("the width mismatch must not load");
        let DocError::Validation { index: 0, message } = err else {
            panic!("expected Validation, got {err:?}");
        };
        assert!(
            message.contains("3 cells"),
            "must name the declared cell count: {message}"
        );
        assert!(
            message.contains("names 2"),
            "must name the column count: {message}"
        );
    }
}
