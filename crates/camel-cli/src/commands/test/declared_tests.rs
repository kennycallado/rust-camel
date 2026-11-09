//! Declared-label extraction tests (test-count-honesty-b D1).

use std::fs;
use std::path::{Path, PathBuf};

use camel_integration_test::parse_scenario_document;

use super::declared_labels;
use super::not_run_deficit;
use super::skipped_cases;
use crate::commands::test::document::{ParsedDocument, parse_test_document};
use crate::commands::test::runner::EndpointResult;

/// An `Ok` endpoint-result row with the given label.
fn ok_row(endpoint: &str) -> EndpointResult {
    EndpointResult {
        endpoint: endpoint.to_string(),
        outcome: Ok(()),
    }
}

/// An `Err` endpoint-result row with the given label.
fn err_row(endpoint: &str) -> EndpointResult {
    EndpointResult {
        endpoint: endpoint.to_string(),
        outcome: Err("failed".to_string()),
    }
}

/// A unique temp directory for one test, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("camel-cli-declared-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Parses a scenario document written to a temp project directory.
fn parse_scenario(tag: &str, yaml: &str) -> ParsedDocument {
    let dir = TempDir::new(tag);
    fs::write(dir.path().join("routes.yaml"), "routes: []\n").expect("write routes.yaml"); // allow-unwrap
    let path = dir.path().join("case.test.yaml");
    fs::write(&path, yaml).expect("write scenario doc"); // allow-unwrap
    let doc = parse_scenario_document(&path).expect("parse scenario doc"); // allow-unwrap
    ParsedDocument::Scenario(Box::new(doc))
}

#[test]
fn declared_unit_lists_expects_replies_and_sequence() {
    let text = r#"
routes: []
expects:
  mock:b: {count: 1}
  mock:a: {count: 1}
inputs:
  - to: direct:first
    body: one
  - to: direct:x
    body: two
    expectReply: {body: ok}
sequence: ["mock:a", "mock:b"]
"#;
    let doc = ParsedDocument::Unit(Box::new(parse_test_document(text).expect("parse unit doc"))); // allow-unwrap
    assert_eq!(
        declared_labels(&doc),
        ["a", "b", "reply[1] direct:x", "<sequence>"]
    );
}

#[test]
fn declared_scenario_lists_action_labels() {
    let doc = parse_scenario(
        "scenario-actions",
        r#"
routeFiles: [routes.yaml]
scenario:
  - send:
      to: direct:start
  - receive:
      from: direct:start
      deadline: 2s
  - validate:
      target:
        lastReceived: direct:start
      expectation:
        equals: ok
logs:
  contains: ["hello"]
"#,
    );
    assert_eq!(
        declared_labels(&doc),
        [
            "scenario[0] send",
            "scenario[1] receive",
            "scenario[2] validate"
        ]
    );
}

#[test]
fn declared_scenario_without_logs_matches_actions() {
    let doc = parse_scenario(
        "scenario-sleep",
        r#"
routeFiles: [routes.yaml]
scenario:
  - sleep:
      duration: 1ms
"#,
    );
    assert_eq!(declared_labels(&doc), ["scenario[0] sleep"]);
}

#[test]
fn deficit_counts_settle_collapse() {
    let declared: Vec<String> = ["a", "b", "c", "d", "e", "f", "g", "h"]
        .iter()
        .map(|label| label.to_string())
        .collect();
    let rows = [err_row("<settle>")];
    assert_eq!(not_run_deficit(&declared, &rows), 7);
}

#[test]
fn deficit_zero_when_rows_cover_declared() {
    let declared: Vec<String> = ["a", "b", "c"].iter().map(|l| l.to_string()).collect();
    let rows = [ok_row("a"), ok_row("b"), ok_row("c")];
    assert_eq!(not_run_deficit(&declared, &rows), 0);
}

#[test]
fn deficit_saturates_when_rows_exceed_declared() {
    let declared: Vec<String> = ["a", "b"].iter().map(|l| l.to_string()).collect();
    let rows = [ok_row("a"), ok_row("b"), err_row("logs")];
    assert_eq!(not_run_deficit(&declared, &rows), 0);
}

#[test]
fn logs_failure_yields_no_skips_and_zero_deficit() {
    let declared: Vec<String> = ["scenario[0] send", "scenario[1] receive"]
        .iter()
        .map(|l| l.to_string())
        .collect();
    let rows = [
        ok_row("scenario[0] send"),
        ok_row("scenario[1] receive"),
        err_row("logs"),
    ];
    assert_eq!(not_run_deficit(&declared, &rows), 0);
    assert!(skipped_cases(&declared, &rows, false).is_empty());
}

#[test]
fn skips_list_all_labels_after_settle_collapse() {
    let declared: Vec<String> = ["a", "b"].iter().map(|l| l.to_string()).collect();
    let rows = [err_row("<settle>")];
    assert_eq!(
        skipped_cases(&declared, &rows, false),
        vec![
            ("a", "settle timeout".to_string()),
            ("b", "settle timeout".to_string())
        ]
    );
}

#[test]
fn skips_reason_document_error() {
    let declared: Vec<String> = ["a", "b", "c"].iter().map(|l| l.to_string()).collect();
    let skipped = skipped_cases(&declared, &[], true);
    assert_eq!(skipped.len(), 3);
    assert!(
        skipped.iter().all(|(_, reason)| reason == "document error"),
        "all reasons document error: {skipped:?}"
    );
}

#[test]
fn skips_reason_stopped_after_failure() {
    let declared: Vec<String> = [
        "scenario[0] send",
        "scenario[1] receive",
        "scenario[2] validate",
        "scenario[3] send",
    ]
    .iter()
    .map(|l| l.to_string())
    .collect();
    let rows = [
        ok_row("scenario[0] send"),
        ok_row("scenario[1] receive"),
        err_row("scenario[2] validate"),
    ];
    assert_eq!(
        skipped_cases(&declared, &rows, false),
        vec![(
            "scenario[3] send",
            "stopped after scenario[2] failure".to_string()
        )]
    );
}
