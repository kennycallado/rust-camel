//! Scenario `surreal:` action read-gate and demand-gate tests
//! (surreal-state-tier task 1.1).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! plain `#[cfg(test)]` — it compiles in BOTH feature configurations:
//! the read-gate and validation tests exercise the ungated grammar,
//! and the demand-gate twin lives in the `feature_off` module (the
//! `sql_validate_test` precedent).

use crate::surreal_action::{RawSurrealAction, validate_surreal_action};
use crate::{DocError, ScenarioDocument, parse_scenario_document};

/// A raw `surreal:` action for `datasource` with `prepare` items
/// (through the test-only constructor; the raw fields stay private).
fn raw(datasource: &str, prepare: Vec<&str>) -> RawSurrealAction {
    RawSurrealAction::raw(datasource, prepare)
}

/// Writes `text` to a fresh temporary `case.test.yaml` and parses it
/// (the `doc_parse_test` helper); the ungated doc-level tests build
/// documents through here.
fn parse_case(text: &str) -> Result<ScenarioDocument, DocError> {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("case.test.yaml");
    std::fs::write(&path, text).expect("write case file");
    parse_scenario_document(&path)
}

/// `(SELECT ...)` is a read behind a parenthesis group: the re-trim
/// after the dropped `(` puts the `select` prefix first, and the
/// error names the action index and the statement index.
#[test]
fn surreal_read_gate_select_prefix() {
    let err = validate_surreal_action(&raw("appdb", vec!["(SELECT * FROM user)"]), 0).unwrap_err();
    assert!(err.contains("surreal action 0"), "got: {err}");
    assert!(err.contains("statement 0"), "got: {err}");
}

/// Leading whitespace does not hide the `select` prefix: the trimmed
/// comparison is case-insensitive.
#[test]
fn surreal_read_gate_case_insensitive() {
    let err = validate_surreal_action(&raw("appdb", vec!["  select 1"]), 1).unwrap_err();
    assert!(err.contains("surreal action 1"), "got: {err}");
    assert!(err.contains("statement 0"), "got: {err}");
}

/// `DEFINE`/`CREATE` are mutations: the validated action round-trips
/// datasource and prepare.
#[test]
fn surreal_write_prefixes_pass() {
    let action = validate_surreal_action(
        &raw(
            "appdb",
            vec![
                "DEFINE TABLE user SCHEMALESS",
                "CREATE user SET name = 'alice'",
            ],
        ),
        0,
    )
    .expect("write statements must validate");
    assert_eq!(action.datasource, "appdb");
    assert_eq!(
        action.prepare,
        vec![
            "DEFINE TABLE user SCHEMALESS".to_string(),
            "CREATE user SET name = 'alice'".to_string(),
        ]
    );
}

/// An empty prepare list is a document defect: the error names the
/// action index.
#[test]
fn empty_prepare_list_rejected() {
    let err = validate_surreal_action(&raw("appdb", vec![]), 4).unwrap_err();
    assert!(err.contains("surreal action 4"), "got: {err}");
    assert!(err.contains("prepare list must not be empty"), "got: {err}");
}

/// A document-level `surreal:` prepare item 0 that is select-prefixed
/// (`(SELECT ...)`, the parenthesis group the read gate re-trims past)
/// is a `DocError::Validation` naming the action index and the
/// statement index: the read gate fires through `build_action` in
/// BOTH feature configurations (validation precedes the demand gate).
#[test]
fn surreal_read_gate_doc_level_rejected() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- surreal:
    datasource: appdb
    prepare:
    - (SELECT * FROM user)
"#,
    )
    .expect_err("a select-prefixed surreal prepare item must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "the error must name the action index");
            assert!(
                message.contains("surreal action 0"),
                "the error must name the action index: {message}"
            );
            assert!(
                message.contains("statement 0"),
                "the error must name the statement index: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// Demand-gate twin (harness `surreal` feature off): a document
/// declaring `surreal:` fails at load with the named demand-gate
/// error, mirroring the `sql:` arm's behavior.
#[cfg(not(feature = "surreal"))]
mod feature_off {
    use super::parse_case;
    use crate::DocError;

    /// A structurally valid `surreal:` action is a named demand-gate
    /// error: it names the `surreal` feature and the rebuild
    /// instruction.
    #[test]
    fn surreal_action_feature_off_is_named_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- surreal:
    datasource: appdb
    prepare:
    - DEFINE TABLE user SCHEMALESS
"#,
        )
        .expect_err("parse must fail without the surreal feature");
        match err {
            DocError::Validation { index, message } => {
                assert_eq!(index, 0, "the error must name the action index");
                assert!(
                    message.contains("`surreal` requires the `surreal` feature"),
                    "the error must name the demand gate: {message}"
                );
                assert!(
                    message.contains("--features surreal"),
                    "the error must carry the rebuild instruction: {message}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }
}
