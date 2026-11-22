//! Load-time rejections of `transacted:` blocks (sqltrans, slice 1, task 4.2).
//!
//! One fixture per rejection kind, each authored as a MINIMAL YAML route —
//! only the offending construct plus, where the rule needs it, a joined sql
//! step on the block datasource. Every fixture goes through the production
//! entry point `compile_declarative_route` (the YAML loader's lowering call,
//! which delegates to the validating path with the always-absent production
//! resolver), so the assertions pin the load-time contract rather than the
//! internal validator.
//!
//! The last fixture (`unregistered_datasource_fails_at_load`) proves the
//! structural pass is reachable and that the manager lookup is typed: a clean
//! block falls through every structural rule and fails on `ManagerMissing`.

use camel_api::CamelError;
use camel_api::transaction::TransactionError;
use camel_dsl::{
    compile_declarative_route, compile_declarative_route_with, parse_canonical_json,
    parse_yaml_to_declarative,
};

/// Author one YAML route, compile it through the dark production entry, and
/// unwrap the typed transaction error.
fn compile_error(yaml: &str) -> TransactionError {
    let routes = parse_yaml_to_declarative(yaml).expect("fixture YAML must author-parse");
    assert_eq!(routes.len(), 1, "fixture must author exactly one route");
    let route = routes.into_iter().next().expect("one route");

    match compile_declarative_route(route) {
        Ok(_) => panic!("expected a typed transaction error at load, got Ok"),
        Err(CamelError::Transaction(error)) => error,
        Err(other) => panic!("expected CamelError::Transaction, got {other:?}"),
    }
}

#[test]
fn parallel_split_inside_block_rejected() {
    let yaml = r#"
routes:
  - id: tx-parallel-split
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - split:
                expression: body_lines
                parallel: true
                steps:
                  - to: "sql:select 1?datasource=pg"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::ParallelInBlock { step, datasource }
                if step == "split" && datasource == "pg"
        ),
        "expected ParallelInBlock {{ step: \"split\", datasource: \"pg\" }}, got {error:?}"
    );
}

#[test]
fn multicast_inside_block_rejected() {
    let yaml = r#"
routes:
  - id: tx-multicast
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - multicast:
                steps:
                  - to: "sql:select 1?datasource=pg"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::MulticastInBlock { step, datasource }
                if step == "multicast" && datasource == "pg"
        ),
        "expected MulticastInBlock {{ step: \"multicast\", datasource: \"pg\" }}, got {error:?}"
    );
}

#[test]
fn wiretap_inside_block_rejected() {
    let yaml = r#"
routes:
  - id: tx-wiretap
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - wire_tap: "sql:insert into audit values (1)?datasource=pg"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::WireTapInBlock { step, datasource }
                if step == "sql:insert into audit values (1)?datasource=pg"
                    && datasource == "pg"
        ),
        "expected WireTapInBlock naming the tap URI on 'pg', got {error:?}"
    );
}

#[test]
fn async_handoff_inside_block_rejected() {
    let yaml = r#"
routes:
  - id: tx-async-handoff
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - to: "sql:select 1?datasource=pg"
            - to: "kafka:topic"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::AsyncInBlock { step, datasource }
                if step == "kafka:topic" && datasource == "pg"
        ),
        "expected AsyncInBlock {{ step: \"kafka:topic\", datasource: \"pg\" }}, got {error:?}"
    );
}

#[test]
fn seda_escape_inside_block_rejected() {
    let yaml = r#"
routes:
  - id: tx-seda-escape
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - to: "sql:select 1?datasource=pg"
            - to: "seda:q"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::SedaInBlock { step, datasource }
                if step == "seda:q" && datasource == "pg"
        ),
        "expected SedaInBlock {{ step: \"seda:q\", datasource: \"pg\" }}, got {error:?}"
    );
}

#[test]
fn stream_list_inside_block_rejected() {
    // The only sql step in the block produces a StreamList: unconditional
    // rejection, no consumer step needs to exist.
    let yaml = r#"
routes:
  - id: tx-stream-list
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - to: "sql:select 1?datasource=pg&outputType=StreamList"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::StreamListInBlock { step, datasource }
                if step == "sql:select 1?datasource=pg&outputType=StreamList"
                    && datasource == "pg"
        ),
        "expected StreamListInBlock naming the sql step on 'pg', got {error:?}"
    );
}

#[test]
fn foreign_datasource_inside_block_rejected() {
    let yaml = r#"
routes:
  - id: tx-foreign-datasource
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - to: "sql:select 1?datasource=audit"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::ForeignDatasource {
                step,
                block_datasource,
                step_datasource,
            } if step == "sql:select 1?datasource=audit"
                && block_datasource == "pg"
                && step_datasource == "audit"
        ),
        "expected ForeignDatasource {{ block: \"pg\", step: \"audit\" }}, got {error:?}"
    );
}

#[test]
fn unnamed_datasource_inside_block_rejected() {
    let yaml = r#"
routes:
  - id: tx-unnamed-datasource
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - to: "sql:select 1?db_url=postgres://localhost/x"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::UnnamedDatasource { step }
                if step == "sql:select 1?db_url=postgres://localhost/x"
        ),
        "expected UnnamedDatasource naming the inline db_url step, got {error:?}"
    );
}

#[test]
fn unnamed_bare_sql_step_inside_block_rejected() {
    // Fail-closed: no `datasource` parameter at all — not even a `db_url` — is
    // still an unnamed step that cannot join the block transaction.
    let yaml = r#"
routes:
  - id: tx-bare-sql
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - to: "sql:select 1"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::UnnamedDatasource { step } if step == "sql:select 1"
        ),
        "expected UnnamedDatasource for the bare sql step, got {error:?}"
    );
}

#[test]
fn on_exception_clause_transacted_rejected() {
    // The block lives in an `on_exceptions` clause, not in `route.steps`. Error
    // handler lowering keeps only `Processor` steps, so without the route-wide
    // walk this block would be dropped silently at load.
    let yaml = r#"
routes:
  - id: tx-on-exception-block
    from: direct:start
    error_handler:
      on_exceptions:
        - message_contains: "boom"
          steps:
            - transacted:
                datasource: pg
                steps:
                  - to: "sql:select 1?datasource=pg"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::ManagerMissing { datasource } if datasource == "pg"
        ),
        "expected ManagerMissing {{ datasource: \"pg\" }} from the clause block, got {error:?}"
    );
}

#[test]
fn circuit_breaker_fallback_transacted_rejected() {
    let yaml = r#"
routes:
  - id: tx-circuit-breaker-fallback
    from: direct:start
    circuit_breaker:
      failure_threshold: 3
      open_duration_ms: 1000
      fallback:
        - transacted:
            datasource: pg
            steps:
              - to: "sql:select 1?datasource=pg"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::ManagerMissing { datasource } if datasource == "pg"
        ),
        "expected ManagerMissing {{ datasource: \"pg\" }} from the fallback block, got {error:?}"
    );
}

#[test]
fn nested_transacted_block_rejected() {
    let yaml = r#"
routes:
  - id: tx-nested
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - transacted:
                datasource: pg
                steps: []
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::NestedBlock { inner } if inner == "pg"
        ),
        "expected NestedBlock {{ inner: \"pg\" }}, got {error:?}"
    );
}

#[test]
fn enrich_sql_step_inside_block_rejected() {
    // `enrich` carries a static URI, so the datasource identity rules apply to
    // its target just like a `to:` step.
    let yaml = r#"
routes:
  - id: tx-enrich-sql
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - enrich: "sql:select 1?datasource=audit"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::ForeignDatasource {
                step,
                block_datasource,
                step_datasource,
            } if step == "sql:select 1?datasource=audit"
                && block_datasource == "pg"
                && step_datasource == "audit"
        ),
        "expected ForeignDatasource for the enrich target, got {error:?}"
    );
}

#[test]
fn poll_enrich_sql_step_inside_block_rejected() {
    // StreamList is rejected unconditionally, and the rule sees the
    // `poll_enrich` target URI, not only `to:` steps.
    let yaml = r#"
routes:
  - id: tx-poll-enrich-stream-list
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - poll_enrich: "sql:select 1?datasource=pg&outputType=StreamList"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::StreamListInBlock { step, datasource }
                if step == "sql:select 1?datasource=pg&outputType=StreamList"
                    && datasource == "pg"
        ),
        "expected StreamListInBlock naming the poll_enrich step, got {error:?}"
    );
}

#[test]
fn foreign_wiretap_uri_inside_block_rejected() {
    // The tap target is a foreign-datasource sql step. No joined sql step
    // exists on the block datasource, so the structural `WireTapInBlock` rule
    // does not fire: the foreign-datasource classification of the tap URI must
    // be the outcome.
    let yaml = r#"
routes:
  - id: tx-foreign-wiretap
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - wire_tap: "sql:select 1?datasource=audit"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::ForeignDatasource {
                step,
                block_datasource,
                step_datasource,
            } if step == "sql:select 1?datasource=audit"
                && block_datasource == "pg"
                && step_datasource == "audit"
        ),
        "expected ForeignDatasource for the wire tap target, got {error:?}"
    );
}

#[test]
fn unregistered_datasource_fails_at_load() {
    // Structurally clean block: every structural rule passes and the failure
    // comes from the manager lookup, typed and naming the datasource.
    let yaml = r#"
routes:
  - id: tx-unregistered
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - to: "sql:select 1?datasource=pg"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::ManagerMissing { datasource } if datasource == "pg"
        ),
        "expected ManagerMissing {{ datasource: \"pg\" }}, got {error:?}"
    );
}

#[test]
fn walk_per_arm_rejection_not_masked_by_manager_missing() {
    // Frozen walk precedence (per-arm BEFORE manager, resolved by ONE
    // visitor): with the always-absent production resolver, a COMMIT-text
    // block fails `TransactionControlSql` — the per-arm check — never
    // `ManagerMissing`. The S1 AST gate judges structure only; manager
    // resolution is the lowering walk's tail.
    let yaml = r#"
routes:
  - id: tx-per-arm-precedence
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - to: "sql:update t set a=1; COMMIT?datasource=pg"
"#;

    let error = compile_error(yaml);
    assert!(
        matches!(&error, TransactionError::TransactionControlSql { .. }),
        "a per-arm rejection must not be masked by ManagerMissing, got {error:?}"
    );
}

// ---------------------------------------------------------------------------
// sqltrans-exec step 3: the ONE load walk through the fan-ins.
//
// The fixtures below go THROUGH the declarative lowering fan-in
// (`compile_declarative_route_with`, the resolver-parameterized seam): the S1
// validating pass runs first with a registered test manager, then lowering
// produces `BuilderStep::Transacted` and the walk fires the req-5 rejections.
// Dialect rejections are NOT reachable through parse seams (no datasource
// catalog there — probe R2-3); their fixtures are direct-call seam tests in
// `camel-core/src/transacted_walk.rs`.
// ---------------------------------------------------------------------------

use std::sync::Arc;
use std::time::Duration;

use camel_api::transaction::{OpenSessionFuture, TransactionManager};

/// Minimal manager for the validated compile seam; the walk never opens a
/// session, so `open_session` is unreachable by construction.
struct WalkSeamManager {
    datasource: &'static str,
}

impl TransactionManager for WalkSeamManager {
    fn datasource(&self) -> &str {
        self.datasource
    }

    fn open_session(&self) -> OpenSessionFuture<'_> {
        panic!("the load walk must never open a transaction session");
    }

    fn acquire_deadline(&self) -> Duration {
        Duration::from_secs(1)
    }
}

/// Resolver registering only the `pg` block datasource.
fn pg_resolver() -> impl Fn(&str) -> Option<Arc<dyn TransactionManager>> {
    let manager: Arc<dyn TransactionManager> = Arc::new(WalkSeamManager { datasource: "pg" });
    move |datasource| (datasource == "pg").then(|| manager.clone())
}

/// Compile through the seam with the fake `pg` manager registered (S1 passes;
/// the walk runs during lowering) and unwrap the typed transaction error.
fn seam_compile_error(yaml: &str) -> TransactionError {
    let routes = parse_yaml_to_declarative(yaml).expect("fixture YAML must author-parse");
    assert_eq!(routes.len(), 1, "fixture must author exactly one route");
    let route = routes.into_iter().next().expect("one route");

    match compile_declarative_route_with(route, &pg_resolver()) {
        Ok(_) => panic!("expected a typed transaction error at load, got Ok"),
        Err(CamelError::Transaction(error)) => error,
        Err(other) => panic!("expected CamelError::Transaction, got {other:?}"),
    }
}

/// Author the block body carrying one joined sql step, wrapped in each of the
/// four nesting containers: choice, (sequential) split, circuit-breaker
/// fallback, loop.
fn nested_yaml(body_line: &str) -> Vec<String> {
    vec![
        // choice arm
        format!(
            r#"
routes:
  - id: tx-nested-choice
    from: direct:start
    steps:
      - choice:
          when:
            - simple: "true"
              steps:
                - transacted:
                    datasource: pg
                    steps:
                      {body_line}
"#
        ),
        // sequential split body
        format!(
            r#"
routes:
  - id: tx-nested-split
    from: direct:start
    steps:
      - split:
          expression: body_lines
          steps:
            - transacted:
                datasource: pg
                steps:
                  {body_line}
"#
        ),
        // circuit-breaker fallback body
        format!(
            r#"
routes:
  - id: tx-nested-fallback
    from: direct:start
    circuit_breaker:
      failure_threshold: 3
      open_duration_ms: 1000
      fallback:
        - transacted:
            datasource: pg
            steps:
              {body_line}
    steps:
      - to: "log:before"
"#
        ),
        // loop body
        format!(
            r#"
routes:
  - id: tx-nested-loop
    from: direct:start
    steps:
      - loop:
          count: 1
          steps:
            - transacted:
                datasource: pg
                steps:
                  {body_line}
"#
        ),
    ]
}

#[test]
fn walk_rejects_transaction_control_sql_nested() {
    // A joined sql step issuing COMMIT inside the block, with the block
    // nested inside choice / split / fallback / loop: the walk reaches it
    // through every container and names the step and the statement.
    for yaml in nested_yaml("- to: \"sql:update t set a=1; COMMIT?datasource=pg\"").iter() {
        let error = seam_compile_error(yaml);
        assert!(
            matches!(
                &error,
                TransactionError::TransactionControlSql { step, statement }
                    if step == "sql:update t set a=1; COMMIT?datasource=pg"
                        && statement == "COMMIT"
            ),
            "expected TransactionControlSql naming step + statement, got {error:?}"
        );
    }
}

#[test]
fn walk_rejects_runtime_sql_in_joined_step_nested() {
    // Header/body-provided statement twin (`allowDynamicQuery=true`), block
    // nested in a choice arm.
    let header_yaml =
        &nested_yaml("- to: \"sql:select 1?datasource=pg&allowDynamicQuery=true\"")[0];
    let error = seam_compile_error(header_yaml);
    assert!(
        matches!(
            &error,
            TransactionError::RuntimeSqlInJoinedStep { step }
                if step == "sql:select 1?datasource=pg&allowDynamicQuery=true"
        ),
        "expected RuntimeSqlInJoinedStep naming the step, got {error:?}"
    );

    // `file:` source twin, block nested in a loop body: an edit after load
    // must never be able to change what executes.
    let file_yaml = &nested_yaml("- to: \"sql:file:/queries/q.sql?datasource=pg\"")[3];
    let error = seam_compile_error(file_yaml);
    assert!(
        matches!(
            &error,
            TransactionError::RuntimeSqlInJoinedStep { step }
                if step == "sql:file:/queries/q.sql?datasource=pg"
        ),
        "expected RuntimeSqlInJoinedStep for the file: source, got {error:?}"
    );
}

#[test]
fn walk_rejects_dynamic_targets_in_block() {
    // A dynamic-routing step inside the block (itself nested in a split
    // body): membership cannot be proven statically, so the load fails
    // closed naming the step.
    let yaml = r#"
routes:
  - id: tx-dynamic-target
    from: direct:start
    steps:
      - split:
          expression: body_lines
          steps:
            - transacted:
                datasource: pg
                steps:
                  - to: "sql:select 1?datasource=pg"
                  - recipient_list:
                      simple: "header.targets"
                      delimiter: ","
"#;

    let error = seam_compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::DynamicTargetUnprovable { step } if step == "recipient-list"
        ),
        "expected DynamicTargetUnprovable naming the step, got {error:?}"
    );
}

#[test]
fn walk_rejects_unregistered_datasource_nested() {
    // Clean-SQL block nested in a loop, loaded through the PRODUCTION entry
    // (always-absent resolvers): the manager lookup is the resolution gate
    // and runs before any dialect stage, so the failure is typed
    // ManagerMissing naming the datasource.
    let yaml = &nested_yaml("- to: \"sql:select 1?datasource=pg\"")[3];
    let error = compile_error(yaml);
    assert!(
        matches!(
            &error,
            TransactionError::ManagerMissing { datasource } if datasource == "pg"
        ),
        "expected ManagerMissing {{ datasource: \"pg\" }}, got {error:?}"
    );
}

#[test]
fn standalone_mysql_and_sqlite_steps_still_load() {
    // Joined steps are PostgreSQL-only in v1, but STANDALONE steps outside
    // blocks are untouched: a route whose sql steps name no transacted block
    // still compiles.
    for uri in [
        "sql:select 1?db_url=mysql://host/db",
        "sql:select 1?db_url=sqlite://mem",
    ] {
        let yaml = format!(
            r#"
routes:
  - id: standalone-non-pg
    from: direct:start
    steps:
      - to: "{uri}"
"#
        );
        let routes = parse_yaml_to_declarative(&yaml).expect("fixture YAML must author-parse");
        assert_eq!(routes.len(), 1, "fixture must author exactly one route");
        let route = routes.into_iter().next().expect("one route");
        assert!(
            compile_declarative_route(route).is_ok(),
            "standalone step '{uri}' outside blocks must still load"
        );
    }
}

#[test]
fn canonical_fan_in_rejects_transaction_control_sql_at_parse() {
    // The SECOND canonical fan-in (the DSL canonical lowering arm, not just
    // camel-core's): compiling the canonical route runs the ONE load walk on
    // the emitted BuilderStep block, so a joined sql step issuing COMMIT is
    // rejected with the typed error naming step + statement.
    let spec = camel_api::runtime::CanonicalRouteSpec {
        route_id: "canonical-tx-control".into(),
        from: "timer:tick".into(),
        steps: vec![camel_api::runtime::CanonicalStepSpec::Transacted(
            camel_api::runtime::CanonicalTransactedSpec {
                datasource: "pg".into(),
                steps: vec![camel_api::runtime::CanonicalStepSpec::To {
                    uri: "sql:update t set a=1; COMMIT?datasource=pg".into(),
                }],
            },
        )],
        circuit_breaker: None,
        auto_startup: None,
        startup_order: None,
        concurrency: None,
        version: camel_api::runtime::CANONICAL_CONTRACT_VERSION,
    };
    let json = serde_json::to_string(&serde_json::json!({ "routes": [spec] }))
        .expect("canonical spec must serialize");

    match parse_canonical_json(&json) {
        Ok(_) => panic!("expected the canonical fan-in to reject control SQL, got Ok"),
        Err(CamelError::Transaction(TransactionError::TransactionControlSql {
            step,
            statement,
        })) => {
            assert_eq!(step, "sql:update t set a=1; COMMIT?datasource=pg");
            assert_eq!(statement, "COMMIT");
        }
        Err(other) => panic!("expected TransactionControlSql, got {other:?}"),
    }
}
