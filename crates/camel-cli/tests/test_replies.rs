//! Integration tests for reply capture and `expectReply` assertions on
//! `*.test.yaml` inputs. Each input delivery captures the reply exchange
//! the `direct:` producer returns; an `expectReply` block asserts against
//! it (body: text exact / JSON structural; headers: exact submap) and
//! surfaces as one `reply[i] <input.to>` row appended to
//! `endpoint_results` (assertion-failure class, never a doc error).
//!
//! Spec: openspec/changes/reply-capture (Tasks 2-3).
//!
//! # Scenario → test mapping
//!
//! Every delta scenario in `openspec/changes/reply-capture/specs/mock-testkit/spec.md`
//! is owned by exactly one test below (or by a Task 1 `document_tests.rs` unit test
//! or a runner.rs unit test, noted inline). Task 3 adds CLI subprocess pins for the
//! exit-code and multi-document scenarios.
//!
//! ## ADDED — Reply capture and assertion
//! - `reply body asserted` → `reply_body_asserted`
//! - `reply body mismatch fails with exit 1` → `reply_body_mismatch_exit_1` (runner
//!   shape) + `reply_mismatch_exits_1_cli` (CLI exit-code pin)
//! - `reply headers asserted` → `reply_headers_asserted`
//! - `reply composes with endpoint expectations` → `reply_composes_with_expects`
//! - `absent expectReply keeps behavior` → `reply_captured_not_asserted_without_expect_reply`
//!   (RED guard: byte-identical output when no input declares `expectReply`)
//! - `empty expectReply is a document error` → Task 1 `expect_reply_empty_rejected`
//!   (parse) + `empty_expect_reply_exits_2_cli` (CLI exit-2 pin)
//! - `multiple inputs pair by order` → `multiple_replies_pair_by_order`
//! - `reply-only document` → Task 1 `reply_only_document_valid` (parse) +
//!   `reply_only_document_exits_0_cli` (CLI exit-0 pin)
//! - `JSON reply body` → `reply_json_body`
//! - `bean stub reply` → `reply_with_bean_stub`
//! - output-message precedence → runner.rs unit test `reply_output_message_precedence`
//! - header-mismatch class guard → `reply_header_mismatch_is_assertion_failure`
//!   (assertion-failure class shared with the "any failure exits 1" scenario)
//!
//! ## MODIFIED — Exit codes, reporting, and multi-document execution
//! - `all pass` → `reply_only_document_exits_0_cli` / `multi_doc_reply_isolation`
//!   (exit-0 path)
//! - `any failure exits 1` → `reply_mismatch_exits_1_cli` (reply failure class)
//! - `parse error with assertion failure exits 2` → `empty_expect_reply_exits_2_cli`
//!   (parse-error class) and the `run_tests` unit tests in `commands/test.rs`
//! - `malformed document exits 2` → `empty_expect_reply_exits_2_cli` (schema
//!   validation) and the `run_tests` unit tests in `commands/test.rs`
//! - `input delivery failure exits 2 and skips evaluation` →
//!   `delivery_error_still_exits_2_with_reply_declared` (MODIFIED skip clause:
//!   no PASS/FAIL lines for endpoints or replies when delivery fails)
//!
//! ## MODIFIED — Declarative test document parsing
//! - `empty expects rejected without expectReply` → Task 1
//!   `no_expects_no_expect_reply_rejected` (parse) + `reply_only_document_exits_0_cli`
//!   (relaxation exercised at the CLI)
//! - all other parsing scenarios (unknown field, route-source exclusivity, body
//!   scalar, settle range, intercept rules) → Task 1 `document_tests.rs` unit tests
//!   (unchanged by this change)

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use camel_cli::commands::test::document::parse_test_document;
use camel_cli::commands::test::run_tests;
use camel_cli::commands::test::runner::{EndpointResult, TestDocResult, run_test_doc};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("camel-test-replies-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
    dir
}

/// The reply rows of a run, in order (`endpoint` starts with `reply[`).
fn reply_rows(result: &TestDocResult) -> Vec<&EndpointResult> {
    result
        .endpoint_results
        .iter()
        .filter(|er| er.endpoint.starts_with("reply["))
        .collect()
}

/// Endpoint labels of a run, for failure diagnostics.
fn endpoint_labels(result: &TestDocResult) -> Vec<&str> {
    result
        .endpoint_results
        .iter()
        .map(|er| er.endpoint.as_str())
        .collect()
}

/// RED guard: a document without `expectReply` behaves byte-identically to
/// today — PASS lines for mock endpoints only, no reply lines, no extra
/// rows in `endpoint_results`.
#[tokio::test(flavor = "multi_thread")]
async fn reply_captured_not_asserted_without_expect_reply() {
    let dir = temp_dir("no-expect-reply");
    let path = dir.join("a.test.yaml");
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - set_body:
          value: "enriched"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write a.test.yaml"); // allow-unwrap

    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    let out_str = String::from_utf8(out).expect("out utf8"); // allow-unwrap
    let err_str = String::from_utf8(err).expect("err utf8"); // allow-unwrap
    assert!(err_str.is_empty(), "err must be empty: {err_str}");
    assert_eq!(summary.exit_code, 0, "out: {out_str}");
    assert_eq!(summary.passed, 1, "only the mock endpoint row: {out_str}");
    assert_eq!(summary.failed, 0, "out: {out_str}");
    assert!(out_str.contains("PASS"), "out: {out_str}");
    assert!(out_str.contains("#out"), "out: {out_str}");
    assert!(
        !out_str.contains("reply["),
        "no reply line may appear without expectReply; out: {out_str}"
    );
}

/// An `expectReply.body` assertion passes when the reply message body
/// matches the body after the route's `set_body` step, and the result
/// carries one passing `reply[0] direct:in` row.
#[tokio::test(flavor = "multi_thread")]
async fn reply_body_asserted() {
    let dir = temp_dir("body-asserted");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - set_body:
          value: "enriched"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      body: "enriched"
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    let rows = reply_rows(&result);
    assert_eq!(rows.len(), 1, "rows: {:?}", endpoint_labels(&result));
    assert_eq!(rows[0].endpoint, "reply[0] direct:in");
    assert!(
        rows[0].outcome.is_ok(),
        "reply row must pass: {:?}",
        rows[0].outcome
    );
}

/// A mismatching `expectReply.body` is an assertion failure (exit-1
/// class): one failed reply row, no doc error, FAIL line naming the reply.
#[tokio::test(flavor = "multi_thread")]
async fn reply_body_mismatch_exit_1() {
    let dir = temp_dir("body-mismatch");
    let path = dir.join("a.test.yaml");
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - set_body:
          value: "enriched"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      body: "wrong"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write a.test.yaml"); // allow-unwrap

    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    let out_str = String::from_utf8(out).expect("out utf8"); // allow-unwrap
    let err_str = String::from_utf8(err).expect("err utf8"); // allow-unwrap
    assert!(
        err_str.is_empty(),
        "body mismatch is an assertion failure, not a doc error; err: {err_str}"
    );
    assert_eq!(summary.exit_code, 1, "out: {out_str}");
    assert_eq!(summary.passed, 1, "mock row still passes: {out_str}");
    assert_eq!(summary.failed, 1, "reply row fails: {out_str}");
    assert!(out_str.contains("FAIL"), "out: {out_str}");
    assert!(
        out_str.contains("FAIL") && out_str.contains("reply[0] direct:in"),
        "FAIL line must name the reply: {out_str}"
    );
}

/// An `expectReply.headers` assertion passes when every expected header is
/// present on the reply message with an equal JSON value (exact submap).
#[tokio::test(flavor = "multi_thread")]
async fn reply_headers_asserted() {
    let dir = temp_dir("headers-asserted");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - set_header:
          key: stamp
          value: "yes"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      headers:
        stamp: "yes"
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    let rows = reply_rows(&result);
    assert_eq!(rows.len(), 1, "rows: {:?}", endpoint_labels(&result));
    assert_eq!(rows[0].endpoint, "reply[0] direct:in");
    assert!(
        rows[0].outcome.is_ok(),
        "reply row must pass: {:?}",
        rows[0].outcome
    );
}

/// JSON reply bodies compare structurally: a `set_body` object literal and
/// an `expectReply.body` object with the same shape match regardless of
/// key order.
#[tokio::test(flavor = "multi_thread")]
async fn reply_json_body() {
    let dir = temp_dir("json-body");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - set_body:
          value:
            status: ok
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      body:
        status: ok
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    let rows = reply_rows(&result);
    assert_eq!(rows.len(), 1, "rows: {:?}", endpoint_labels(&result));
    assert!(
        rows[0].outcome.is_ok(),
        "reply row must pass: {:?}",
        rows[0].outcome
    );
}

/// `expects` and `expectReply` compose: both rows pass and the driver
/// reports exit 0 with both rows counted.
#[tokio::test(flavor = "multi_thread")]
async fn reply_composes_with_expects() {
    let dir = temp_dir("composes");
    let path = dir.join("a.test.yaml");
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - set_body:
          value: "enriched"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      body: "enriched"
expects:
  mock:out:
    count: 1
    bodies: ["enriched"]
"#,
    )
    .expect("write a.test.yaml"); // allow-unwrap

    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    let out_str = String::from_utf8(out).expect("out utf8"); // allow-unwrap
    let err_str = String::from_utf8(err).expect("err utf8"); // allow-unwrap
    assert!(err_str.is_empty(), "err: {err_str}");
    assert_eq!(summary.exit_code, 0, "out: {out_str}");
    assert_eq!(summary.passed, 2, "mock row + reply row: {out_str}");
    assert_eq!(summary.failed, 0, "out: {out_str}");
    assert!(out_str.contains("#out"), "out: {out_str}");
    assert!(out_str.contains("reply[0] direct:in"), "out: {out_str}");
}

/// Multiple inputs pair with their replies by delivery order: two routes,
/// two inputs, two reply rows labeled `reply[0] direct:first` and
/// `reply[1] direct:second`, both passing in input order.
#[tokio::test(flavor = "multi_thread")]
async fn multiple_replies_pair_by_order() {
    let dir = temp_dir("pair-order");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:first"
    steps:
      - set_body:
          value: "first-done"
      - to: "mock:oa"
  - id: r2
    from: "direct:second"
    steps:
      - set_body:
          value: "second-done"
      - to: "mock:ob"
inputs:
  - to: "direct:first"
    body: "x"
    expectReply:
      body: "first-done"
  - to: "direct:second"
    body: "y"
    expectReply:
      body: "second-done"
expects:
  mock:oa:
    count: 1
  mock:ob:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    let rows = reply_rows(&result);
    assert_eq!(rows.len(), 2, "rows: {:?}", endpoint_labels(&result));
    assert_eq!(rows[0].endpoint, "reply[0] direct:first");
    assert_eq!(rows[1].endpoint, "reply[1] direct:second");
    assert!(
        rows[0].outcome.is_ok(),
        "reply[0] must pass: {:?}",
        rows[0].outcome
    );
    assert!(
        rows[1].outcome.is_ok(),
        "reply[1] must pass: {:?}",
        rows[1].outcome
    );
}

/// A mismatching `expectReply.headers` value is an assertion failure: the
/// result carries exactly one FAILED reply row and no `doc_error`.
#[tokio::test(flavor = "multi_thread")]
async fn reply_header_mismatch_is_assertion_failure() {
    let dir = temp_dir("header-mismatch");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - set_header:
          key: stamp
          value: "no"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      headers:
        stamp: "yes"
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "header mismatch is an assertion failure, not a doc error; doc_error: {:?}",
        result.doc_error
    );
    let rows = reply_rows(&result);
    assert_eq!(rows.len(), 1, "rows: {:?}", endpoint_labels(&result));
    let detail = rows[0].outcome.as_ref().expect_err("reply row must fail"); // allow-unwrap
    assert!(
        detail.contains("reply header mismatch 'stamp'"),
        "failure detail must name the mismatched key, got: {detail}"
    );
    assert!(
        detail.contains("expected"),
        "failure detail must name expected vs actual, got: {detail}"
    );
}

/// Reply assertions compose with the bean-test-registry: a `setBody`
/// stub's body mutation is visible on the captured reply.
#[tokio::test(flavor = "multi_thread")]
async fn reply_with_bean_stub() {
    let dir = temp_dir("bean-stub");
    let yaml = r#"
beans:
  enricher:
    kind: setBody
    config:
      body: enriched
routes:
  - id: r1
    from: "direct:in"
    steps:
      - bean:
          name: enricher
          method: enrich
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      body: "enriched"
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    let rows = reply_rows(&result);
    assert_eq!(rows.len(), 1, "rows: {:?}", endpoint_labels(&result));
    assert_eq!(rows[0].endpoint, "reply[0] direct:in");
    assert!(
        rows[0].outcome.is_ok(),
        "reply row must pass: {:?}",
        rows[0].outcome
    );
}

// ---------------------------------------------------------------------------
// Task 3: CLI-level exit codes + execution matrix (subprocess pins)
// ---------------------------------------------------------------------------

/// A mismatching `expectReply.body` through the real binary exits 1: stdout
/// carries a FAIL line naming the reply and the summary counts the reply row.
#[test]
fn reply_mismatch_exits_1_cli() {
    let dir = temp_dir("mismatch-cli");
    let doc = dir.join("a.test.yaml");
    fs::write(
        &doc,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - set_body:
          value: "enriched"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      body: "wrong"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write a.test.yaml"); // allow-unwrap

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "reply mismatch must exit 1; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("FAIL") && stdout.contains("reply[0]"),
        "stdout must carry a FAIL line naming the reply; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("1 passed, 1 failed"),
        "summary must count the reply row; stdout:\n{stdout}"
    );
}

/// A reply-only document (no `expects`) through the real binary exits 0: one
/// PASS reply line, no endpoint lines.
#[test]
fn reply_only_document_exits_0_cli() {
    let dir = temp_dir("reply-only-cli");
    let doc = dir.join("a.test.yaml");
    fs::write(
        &doc,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - set_body:
          value: "done"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      body: "done"
"#,
    )
    .expect("write a.test.yaml"); // allow-unwrap

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "reply-only doc must exit 0; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("PASS") && stdout.contains("reply[0]"),
        "stdout must carry one PASS reply line; stdout:\n{stdout}"
    );
    for line in stdout.lines() {
        assert!(
            !(line.starts_with("PASS") && !line.contains("reply[")),
            "no endpoint PASS line may appear for a reply-only doc; got: {line}\nstdout:\n{stdout}"
        );
    }
}

/// A fail-bean doc with `expectReply` declared still exits 2 (delivery
/// failure), and NO PASS/FAIL lines print for endpoints or replies — pins the
/// MODIFIED exit-codes skip clause.
#[test]
fn delivery_error_still_exits_2_with_reply_declared() {
    let dir = temp_dir("delivery-fail-reply");
    let doc = dir.join("fail.test.yaml");
    fs::write(
        &doc,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - bean:
          name: gate
          method: check
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      body: "enriched"
beans:
  gate:
    kind: fail
    config:
      message: boom
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write fail.test.yaml"); // allow-unwrap

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "delivery failure must exit 2; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("boom"),
        "stderr must carry the fail message; stderr:\n{stderr}"
    );
    for line in stdout.lines() {
        assert!(
            !(line.starts_with("PASS") || line.starts_with("FAIL")),
            "no endpoint or reply line may print for the failed doc; got: {line}\nstdout:\n{stdout}"
        );
    }
}

/// Two docs in one invocation — a.test.yaml reply-only, b.test.yaml
/// expects-only — both pass and the process exits 0 (reply rows and endpoint
/// rows coexist across documents without leaking).
#[test]
fn multi_doc_reply_isolation() {
    let dir = temp_dir("multi-reply-isolation");
    let a_path = dir.join("a.test.yaml");
    let b_path = dir.join("b.test.yaml");
    fs::write(
        &a_path,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - set_body:
          value: "done"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply:
      body: "done"
"#,
    )
    .expect("write a.test.yaml"); // allow-unwrap
    fs::write(
        &b_path,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "y"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write b.test.yaml"); // allow-unwrap

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&a_path)
        .arg(&b_path)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "both docs must pass (exit 0); stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("a.test.yaml#reply[0]"),
        "stdout must carry the reply PASS line for a.test.yaml; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("b.test.yaml#out"),
        "stdout must carry the endpoint PASS line for b.test.yaml; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("2 passed, 0 failed"),
        "summary must be 2 passed, 0 failed; stdout:\n{stdout}"
    );
}

/// An `expectReply: {}` doc through the real binary exits 2 and stderr states
/// the body-or-headers requirement.
#[test]
fn empty_expect_reply_exits_2_cli() {
    let dir = temp_dir("empty-reply-cli");
    let doc = dir.join("a.test.yaml");
    fs::write(
        &doc,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "x"
    expectReply: {}
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write a.test.yaml"); // allow-unwrap

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "empty expectReply must exit 2; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("expectReply must declare body or headers"),
        "stderr must state the body-or-headers requirement; stderr:\n{stderr}"
    );
}
