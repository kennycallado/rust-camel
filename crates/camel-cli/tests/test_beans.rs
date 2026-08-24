//! Integration tests for declarative stub beans (`beans:` block in
//! `*.test.yaml`). One document may declare named stub beans (echo, setBody,
//! fail) that the runner registers in a `BeanRegistry` before the context
//! boots, so `bean:` steps in the routes resolve against them.
//!
//! Spec: openspec/changes/bean-test-registry (Tasks 3-4).
//!
//! # Scenario → test mapping
//!
//! Every delta scenario in `openspec/changes/bean-test-registry/specs/mock-testkit/spec.md`
//! is owned by exactly one test below (or by a Task 2 `document_tests.rs` unit test,
//! noted inline). The requirement-pins from Task 2 (config variants) and both MODIFIED
//! scenarios are covered here at the execution level.
//!
//! ## ADDED — Declarative bean stubs
//! - `setBody stub transforms the body` → `setbody_stub_transforms_body`
//! - `echo stub passes the exchange through` → `echo_stub_passes_through`
//! - `fail stub surfaces as a document error` → `fail_stub_surfaces_doc_error`
//! - `fail stub without message uses the exact default` → `fail_stub_default_message`
//! - `undeclared method is a document error` → `undeclared_method_rejected_before_boot`
//! - `omitted methods accepts route invocations` → `wildcard_accepts_route_methods`
//! - `unknown kind is a document error` → Task 2 `beans_unknown_kind_rejected`
//! - `setBody without body config is a document error` → Task 2 `beans_setbody_missing_body_rejected`
//! - `kind-inappropriate config key is a document error` → Task 2 `beans_config_variant_pins`
//! - `empty methods list is a document error` → Task 2 `beans_empty_methods_rejected`
//! - `blank bean name is a document error` → `blank_bean_name_exit_2` (execution-level exit-2 pin)
//! - `blank methods entry is a document error` → Task 2 `beans_blank_method_entry_rejected`
//! - `nested unknown field in a bean declaration is a document error` → Task 2 `beans_nested_unknown_field_rejected`
//! - `multiple beans in one document` → `multiple_beans_one_document`
//! - `intercepts and beans compose` → `intercepts_and_beans_compose`
//! - `camel run ignores the beans block` → `camel_run_ignores_beans_block`
//!
//! ## MODIFIED — Exit codes, reporting, and multi-document execution
//! - `input delivery failure exits 2 and skips evaluation` → `input_delivery_failure_skips_evaluation`
//! - `all pass` / `any failure exits 1` / `parse error with assertion failure exits 2`
//!   / `malformed document exits 2` → `multi_doc_with_beans_isolated` (multi-doc isolation
//!   + exit-0 path) and the `run_tests` unit tests in `commands/test.rs`
//!
//! ## MODIFIED — In-process route execution
//! - `routes run in-process` → `setbody_stub_transforms_body` / `echo_stub_passes_through`
//!   (in-process mock delivery through a bean step)
//! - `self-starting route without inputs` → `test_runner.rs` (timer-driven, no beans)

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use camel_cli::commands::test::document::parse_test_document;
use camel_cli::commands::test::run_tests;
use camel_cli::commands::test::runner::run_test_doc;

mod common;

use common::{drain_to_buffer, send_term, spawn_camel_run, wait_exit_bounded, wait_for_marker};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("camel-test-beans-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
    dir
}

/// A bean step with no `beans:` block fails at route-add time with
/// `Bean not found` — documents pre-registry behavior; the wiring must not
/// change it (no beans block ⇒ no registry ⇒ unchanged failure).
#[tokio::test(flavor = "multi_thread")]
async fn bean_route_without_registry_fails_today() {
    let dir = temp_dir("no-registry");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: enricher
          method: enrich
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    let err = result.doc_error.expect("doc_error must be Some"); // allow-unwrap
    assert!(
        err.contains("Bean not found: enricher"),
        "doc_error must name the missing bean, got: {err}"
    );
}

/// A `setBody` stub replaces the exchange body with its configured string.
#[tokio::test(flavor = "multi_thread")]
async fn setbody_stub_transforms_body() {
    let dir = temp_dir("setbody");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: enricher
          method: enrich
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
beans:
  enricher:
    kind: setBody
    config:
      body: stubbed
expects:
  mock:out:
    count: 1
    bodies: ["stubbed"]
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    for er in &result.endpoint_results {
        assert!(
            er.outcome.is_ok(),
            "endpoint {} failed: {:?}",
            er.endpoint,
            er.outcome
        );
    }
}

/// An `echo` stub passes the exchange through untouched (any method name).
#[tokio::test(flavor = "multi_thread")]
async fn echo_stub_passes_through() {
    let dir = temp_dir("echo");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: gate
          method: whatever
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
beans:
  gate:
    kind: echo
expects:
  mock:out:
    count: 1
    bodies: ["x"]
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    for er in &result.endpoint_results {
        assert!(
            er.outcome.is_ok(),
            "endpoint {} failed: {:?}",
            er.endpoint,
            er.outcome
        );
    }
}

/// A `fail` stub with a configured message surfaces the message as a
/// document error; no endpoint evaluations run.
#[tokio::test(flavor = "multi_thread")]
async fn fail_stub_surfaces_doc_error() {
    let dir = temp_dir("fail-configured");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: gate
          method: check
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
beans:
  gate:
    kind: fail
    config:
      message: boom
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    let err = result.doc_error.expect("doc_error must be Some"); // allow-unwrap
    assert!(
        err.contains("boom"),
        "doc_error must carry the configured message, got: {err}"
    );
    assert_eq!(
        result.endpoint_results.len(),
        0,
        "no endpoint evaluations may run"
    );
}

/// A `fail` stub without `config.message` fails with the default message
/// `fail bean <name>`.
#[tokio::test(flavor = "multi_thread")]
async fn fail_stub_default_message() {
    let dir = temp_dir("fail-default");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: gate
          method: check
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
beans:
  gate:
    kind: fail
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    let err = result.doc_error.expect("doc_error must be Some"); // allow-unwrap
    assert!(
        err.contains("fail bean gate"),
        "doc_error must carry the default message, got: {err}"
    );
}

/// An explicit `methods` allowlist is cross-validated against the route's
/// bean calls BEFORE boot: invoking an undeclared method is a document error
/// naming the method, not a runtime `Bean not found`.
#[tokio::test(flavor = "multi_thread")]
async fn undeclared_method_rejected_before_boot() {
    let dir = temp_dir("undeclared-method");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: enricher
          method: transform
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
beans:
  enricher:
    kind: echo
    methods: [enrich]
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    let err = result.doc_error.expect("doc_error must be Some"); // allow-unwrap
    assert!(
        err.contains("method transform is not declared"),
        "doc_error must name the undeclared method, got: {err}"
    );
    assert!(
        !err.contains("Bean not found"),
        "cross-validation must fire before boot, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Task 4: execution matrix — validation, non-interference, composition
// ---------------------------------------------------------------------------

/// A bean with no `methods` allowlist accepts every method the routes invoke
/// on it (wildcard). Two bean steps on the same `gate` bean with different
/// methods `m1`, `m2` both resolve; the exchange passes through and `mock:out`
/// records count 1.
#[tokio::test(flavor = "multi_thread")]
async fn wildcard_accepts_route_methods() {
    let dir = temp_dir("wildcard");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: gate
          method: m1
      - bean:
          name: gate
          method: m2
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
beans:
  gate:
    kind: echo
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
    assert_eq!(result.endpoint_results.len(), 1);
    for er in &result.endpoint_results {
        assert!(
            er.outcome.is_ok(),
            "endpoint {} failed: {:?}",
            er.endpoint,
            er.outcome
        );
    }
}

/// Two beans declared in one document both resolve: `a` (setBody) stamps the
/// body `first`, `b` (echo) passes it through, and `mock:out` records body
/// `first` with count 1.
#[tokio::test(flavor = "multi_thread")]
async fn multiple_beans_one_document() {
    let dir = temp_dir("multi-beans");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: a
          method: m1
      - bean:
          name: b
          method: m2
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
beans:
  a:
    kind: setBody
    config:
      body: first
  b:
    kind: echo
expects:
  mock:out:
    count: 1
    bodies: ["first"]
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    for er in &result.endpoint_results {
        assert!(
            er.outcome.is_ok(),
            "endpoint {} failed: {:?}",
            er.endpoint,
            er.outcome
        );
    }
}

/// A blank bean name is a parse-time document error that maps to exit code 2
/// through the CLI (`test.rs:182-207`): `camel test` on a doc with
/// `beans: {"  ": {kind: echo}}` exits 2 and stderr names the `non-blank`
/// requirement.
#[test]
fn blank_bean_name_exit_2() {
    let dir = temp_dir("blank-name");
    let doc = dir.join("blank.test.yaml");
    fs::write(
        &doc,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
beans:
  "  ":
    kind: echo
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write blank.test.yaml"); // allow-unwrap

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
        "blank bean name must exit 2; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("non-blank"),
        "stderr must state the non-blank requirement; stderr:\n{stderr}"
    );
}

/// A `fail` stub's processor error propagates out of the route as an input
/// delivery failure: `camel test` exits 2, the output carries `boom`, and
/// stdout has NO `PASS`/`FAIL` line for `mock:out` (evaluation is skipped for
/// that document — pins the MODIFIED exit-codes scenario).
#[test]
fn input_delivery_failure_skips_evaluation() {
    let dir = temp_dir("input-fail");
    let doc = dir.join("fail.test.yaml");
    fs::write(
        &doc,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: gate
          method: check
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
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
        "input delivery failure must exit 2; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("boom"),
        "stderr must carry the fail message; stderr:\n{stderr}"
    );
    for line in stdout.lines() {
        assert!(
            !(line.starts_with("PASS") || line.starts_with("FAIL")),
            "no endpoint line may be printed for the failed doc; got: {line}\nstdout:\n{stdout}"
        );
    }
}

/// `camel run` must never read the `beans` block. Route discovery skips
/// `*.test.yaml` documents (reserved test suffix), so a colocated test
/// document declaring an intentionally INVALID `beans:` block is a no-op for
/// the production run: the process starts, stays alive, and neither output
/// stream names the test document, the invalid kind name `teleport` (which
/// exists only in the test document), or the `unknown kind` validation
/// fragment. Had `camel run` parsed the document, the unknown kind would fail
/// loudly.
#[test]
fn camel_run_ignores_beans_block() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap

    // Marker + explicit glob so discovery actually encounters the test doc
    // and the reserved-suffix skip is exercised on the happy path.
    std::fs::write(
        dir.path().join("Camel.toml"),
        r#"[default]
routes = ["config/*.yaml"]
log_level = "INFO"
"#,
    )
    .expect("write Camel.toml"); // allow-unwrap
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).expect("create config/"); // allow-unwrap
    std::fs::write(
        config_dir.join("routes.yaml"),
        r#"routes:
  - id: "demo"
    from: "direct:start"
    steps:
      - to: "log:done"
"#,
    )
    .expect("write config/routes.yaml"); // allow-unwrap
    // Intentionally invalid beans block: an unknown kind would fail
    // validation loudly if `camel run` ever parsed this document.
    std::fs::write(
        config_dir.join("probe.test.yaml"),
        r#"routeFiles: [routes.yaml]
inputs: []
beans:
  x:
    kind: teleport
expects:
  mock:result:
    count: 1
"#,
    )
    .expect("write config/probe.test.yaml"); // allow-unwrap

    let mut child = spawn_camel_run(dir.path());
    let out_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let err_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    let stdout = child
        .stdout
        .take()
        .expect("child stdout was configured as piped"); // allow-unwrap
    let stderr = child
        .stderr
        .take()
        .expect("child stderr was configured as piped"); // allow-unwrap

    let out_thread_buf = Arc::clone(&out_buf);
    let err_thread_buf = Arc::clone(&err_buf);
    let out_handle = thread::spawn(move || drain_to_buffer(stdout, out_thread_buf));
    let err_handle = thread::spawn(move || drain_to_buffer(stderr, err_thread_buf));

    // Liveness window: the run must reach the steady-state "running" log
    // line, printed only after discovery + context start succeeded.
    let alive = wait_for_marker(
        &mut child,
        &[Arc::clone(&out_buf), Arc::clone(&err_buf)],
        "camel-cli: running",
        Duration::from_secs(30),
    );
    let captured_so_far = || {
        format!(
            "stdout:\n{}\nstderr:\n{}",
            out_buf.lock().expect("stdout buffer lock poisoned"),
            err_buf.lock().expect("stderr buffer lock poisoned")
        )
    };
    assert!(
        alive,
        "camel run did not reach the running state within 30 s;\n{}",
        captured_so_far()
    );

    // Give any late error a chance to surface, then probe liveness again.
    thread::sleep(Duration::from_secs(1));
    let status = child.try_wait().expect("try_wait after liveness window"); // allow-unwrap
    assert!(
        status.is_none(),
        "camel run died after startup (status: {status:?}); the run must ignore \
         the colocated test document;\n{}",
        captured_so_far()
    );

    // Terminate cleanly with exactly ONE SIGTERM.
    send_term(&child);
    let exited = wait_exit_bounded(&mut child, Duration::from_secs(10));
    let _ = out_handle.join();
    let _ = err_handle.join();
    assert!(
        exited,
        "camel run did not exit within 10 s after SIGTERM;\n{}",
        captured_so_far()
    );

    // Neither stream may name the test document or carry a test-document
    // validation error. Three targeted bans cover the contract: the
    // reserved-suffix filename, the invalid kind name `teleport` (it exists
    // only in the test document, so a benign log cannot contain it), and the
    // `unknown variant` validation fragment. The bare word `beans` is NOT banned:
    // `camel run`'s unconditional CWD-trust startup warning legitimately
    // contains it (run.rs), so the ban is on the invalid kind instead.
    let stdout_text = out_buf.lock().expect("stdout buffer lock poisoned").clone(); // allow-unwrap
    let stderr_text = err_buf.lock().expect("stderr buffer lock poisoned").clone(); // allow-unwrap
    for (stream, text) in [("stdout", &stdout_text), ("stderr", &stderr_text)] {
        assert!(
            !text.contains("probe.test.yaml"),
            "{stream} names the test document; camel run must skip *.test.yaml:\n{text}"
        );
        assert!(
            !text.contains("teleport"),
            "{stream} names the invalid bean kind from the test document; camel \
             run must not read it:\n{text}"
        );
        assert!(
            !text.contains("unknown variant"),
            "{stream} carries a bean validation error; camel run must not parse \
             test documents:\n{text}"
        );
    }
}

/// Intercepts and beans compose independently in one document: the intercept
/// diverts `kafka:orders` to `mock:orders` (route A), while a `setBody` bean
/// stamps the body `stamped` in route B before `mock:out`. Both routes'
/// expectations evaluate.
#[tokio::test(flavor = "multi_thread")]
async fn intercepts_and_beans_compose() {
    let dir = temp_dir("compose");
    let yaml = r#"
routes:
  - id: a
    from: "direct:a"
    steps:
      - to: "kafka:orders"
  - id: b
    from: "direct:b"
    steps:
      - bean:
          name: gate
          method: mark
      - to: "mock:out"
inputs:
  - to: "direct:a"
    body: "k"
  - to: "direct:b"
    body: "z"
intercepts:
  kafka:orders:
    skipTo: mock:orders
beans:
  gate:
    kind: setBody
    config:
      body: stamped
expects:
  mock:orders:
    count: 1
    bodies: ["k"]
  mock:out:
    count: 1
    bodies: ["stamped"]
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 2);
    for er in &result.endpoint_results {
        assert!(
            er.outcome.is_ok(),
            "endpoint {} failed: {:?}",
            er.endpoint,
            er.outcome
        );
    }
}

/// The bean registry is per-document: two docs in one CLI invocation — one
/// with a `beans:` block, one without — both pass, proving no registry leaks
/// across documents.
#[tokio::test(flavor = "multi_thread")]
async fn multi_doc_with_beans_isolated() {
    let dir = temp_dir("multi-isolated");
    let a_path = dir.join("a.test.yaml");
    let b_path = dir.join("b.test.yaml");
    fs::write(
        &a_path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: a1
          method: mark
      - to: "mock:oa"
inputs:
  - to: "direct:start"
    body: "x"
beans:
  a1:
    kind: setBody
    config:
      body: aa
expects:
  mock:oa:
    count: 1
    bodies: ["aa"]
"#,
    )
    .expect("write a.test.yaml"); // allow-unwrap
    fs::write(
        &b_path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:ob"
inputs:
  - to: "direct:start"
    body: "y"
expects:
  mock:ob:
    count: 1
    bodies: ["y"]
"#,
    )
    .expect("write b.test.yaml"); // allow-unwrap

    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[a_path.clone(), b_path.clone()], &mut out, &mut err).await;
    let out_str = String::from_utf8(out).expect("out utf8"); // allow-unwrap
    let err_str = String::from_utf8(err).expect("err utf8"); // allow-unwrap
    assert!(
        err_str.is_empty(),
        "no doc_error expected; err was: {err_str}\nout was: {out_str}"
    );
    assert_eq!(
        summary.exit_code, 0,
        "both docs must pass (exit 0); err: {err_str} out: {out_str}"
    );
    assert_eq!(
        summary.passed, 2,
        "both endpoints must pass; out: {out_str} err: {err_str}"
    );
    assert_eq!(
        summary.failed, 0,
        "no failures expected; out: {out_str} err: {err_str}"
    );
    assert!(
        out_str.contains("a.test.yaml#oa"),
        "out must contain PASS for a.test.yaml mock:oa; out: {out_str}"
    );
    assert!(
        out_str.contains("b.test.yaml#ob"),
        "out must contain PASS for b.test.yaml mock:ob; out: {out_str}"
    );
    assert!(
        out_str.contains("2 passed, 0 failed"),
        "out summary must be 2 passed, 0 failed; out: {out_str}"
    );
}
