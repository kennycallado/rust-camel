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
use camel_dsl::{compile_declarative_route, parse_yaml_to_declarative};

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
