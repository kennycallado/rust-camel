//! Integration tests for the in-process mock test runner
//! (`camel_cli::commands::test::runner`).

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use camel_cli::commands::test::document::parse_test_document;
use camel_cli::commands::test::run_tests;
use camel_cli::commands::test::runner::run_test_doc;

/// Create a unique temp directory for one test.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("camel-test-runner-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
    dir
}

/// Parse a document and run it, returning the result.
async fn run(
    yaml: &str,
    doc_dir: &std::path::Path,
) -> camel_cli::commands::test::runner::TestDocResult {
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    run_test_doc(&doc, doc_dir).await.0
}

#[tokio::test(flavor = "multi_thread")]
async fn timer_route_settles_and_passes() {
    let dir = temp_dir("timer");
    let yaml = r#"
routes:
  - id: r1
    from: "timer:tick?period=50&repeatCount=3"
    steps:
      - to: "mock:result"
expects:
  mock:result:
    count: 3
"#;
    let result = run(yaml, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    let er = &result.endpoint_results[0];
    assert_eq!(er.endpoint, "result", "endpoint must carry the bare name");
    assert!(er.outcome.is_ok(), "expected Ok, got {:?}", er.outcome);
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_input_reaches_mock() {
    let dir = temp_dir("direct");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
    bodies: ["x"]
"#;
    let result = run(yaml, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    let er = &result.endpoint_results[0];
    assert_eq!(er.endpoint, "out");
    assert!(er.outcome.is_ok(), "expected Ok, got {:?}", er.outcome);
}

#[tokio::test(flavor = "multi_thread")]
async fn route_files_resolve_relative_to_doc_dir() {
    let dir = temp_dir("routefiles");
    let cfg_dir = dir.join("cfg");
    fs::create_dir_all(&cfg_dir).expect("create cfg dir"); // allow-unwrap
    fs::write(
        cfg_dir.join("demo.yaml"),
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:rel"
"#,
    )
    .expect("write route file"); // allow-unwrap
    let yaml = r#"
routeFiles: [cfg/demo.yaml]
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:rel:
    count: 1
"#;
    let result = run(yaml, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    let er = &result.endpoint_results[0];
    assert_eq!(er.endpoint, "rel");
    assert!(er.outcome.is_ok(), "expected Ok, got {:?}", er.outcome);
}

#[tokio::test(flavor = "multi_thread")]
async fn mismatch_reports_change1_detail() {
    let dir = temp_dir("mismatch");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:result"
inputs:
  - to: "direct:start"
    body: "a"
  - to: "direct:start"
    body: "b"
expects:
  mock:result:
    count: 3
"#;
    let result = run(yaml, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    let er = &result.endpoint_results[0];
    let err = er.outcome.as_ref().expect_err("must fail"); // allow-unwrap
    assert!(
        err.contains("expected 3 exchanges, got 2"),
        "error text: {err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn absent_endpoint_fails() {
    let dir = temp_dir("absent");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:result"
inputs:
  - to: "direct:start"
    body: "a"
expects:
  mock:ghost:
    count: 1
"#;
    let result = run(yaml, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    let er = &result.endpoint_results[0];
    let err = er.outcome.as_ref().expect_err("must fail"); // allow-unwrap
    assert!(err.contains("ghost"), "error text: {err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn failure_does_not_abort_other_endpoints() {
    let dir = temp_dir("multi");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:good"
      - to: "mock:bad"
inputs:
  - to: "direct:start"
    body: "a"
expects:
  mock:good:
    count: 1
  mock:bad:
    count: 3
"#;
    let result = run(yaml, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 2, "both endpoints evaluated");
    let good = result
        .endpoint_results
        .iter()
        .find(|er| er.endpoint == "good")
        .expect("good endpoint present"); // allow-unwrap
    let bad = result
        .endpoint_results
        .iter()
        .find(|er| er.endpoint == "bad")
        .expect("bad endpoint present"); // allow-unwrap
    assert!(good.outcome.is_ok(), "good should pass: {:?}", good.outcome);
    assert!(bad.outcome.is_err(), "bad should fail");
}

#[tokio::test(flavor = "multi_thread")]
async fn settle_window_resets_on_change() {
    let dir = temp_dir("settle-reset");
    let yaml = r#"
routes:
  - id: r1
    from: "timer:tick?period=100&repeatCount=4"
    steps:
      - to: "mock:result"
expects:
  mock:result:
    count: 4
settle: "250ms"
"#;
    let started = Instant::now();
    let result = run(yaml, &dir).await;
    let elapsed = started.elapsed();
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    let er = &result.endpoint_results[0];
    assert!(er.outcome.is_ok(), "expected Ok, got {:?}", er.outcome);
    // Ticks land at ~0/100/200/300ms; a reset must have occurred, so settle
    // exits no earlier than ~500ms. Lower bound only — do not assert ≥650ms.
    assert!(
        elapsed >= std::time::Duration::from_millis(500),
        "elapsed {elapsed:?} proves a quiet-window reset occurred"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn headers_roundtrip_input_to_expectation() {
    let dir = temp_dir("headers");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "h"
    headers:
      kind: greeting
expects:
  mock:out:
    count: 1
    headers:
      kind: greeting
"#;
    let result = run(yaml, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    let er = &result.endpoint_results[0];
    assert_eq!(er.endpoint, "out");
    assert!(er.outcome.is_ok(), "expected Ok, got {:?}", er.outcome);
}

#[tokio::test(flavor = "multi_thread")]
async fn header_expectation_mismatch_fails() {
    let dir = temp_dir("header-mismatch");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "h"
    headers:
      kind: greeting
expects:
  mock:out:
    count: 1
    headers:
      kind: farewell
"#;
    let result = run(yaml, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    let er = &result.endpoint_results[0];
    let err = er.outcome.as_ref().expect_err("must fail"); // allow-unwrap
    assert!(
        err.contains("kind"),
        "error text must name header key: {err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_doc_stops_context() {
    // slow: settle-timeout doc (~5s instability budget). After it returns, the
    // context must have been stopped, so the timer producer path is dead and
    // the mock's received_count stops growing.
    let dir = temp_dir("stop-ctx");
    let yaml = r#"
routes:
  - id: r1
    from: "timer:tick?period=10&repeatCount=100000"
    steps:
      - to: "mock:result"
expects:
  mock:result:
    count: 100000
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, mock) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    let er = &result.endpoint_results[0];
    assert_eq!(er.endpoint, "<settle>");
    assert!(er.outcome.is_err(), "settle must time out");

    // Poll the mock endpoint's received_count for ~500ms after return; it must
    // stop growing once the context is stopped (timer producer path dead).
    let inner = mock
        .get_endpoint("result")
        .expect("mock:result endpoint exists"); // allow-unwrap
    let baseline = inner.received_count().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let after = inner.received_count().await;
    assert_eq!(
        baseline, after,
        "received_count must stop growing after the context is stopped \
         (baseline {baseline}, after {after})"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn settle_deadline_times_out() {
    // slow: deadline cap (~5s instability budget).
    let dir = temp_dir("settle-timeout");
    let yaml = r#"
routes:
  - id: r1
    from: "timer:tick?period=10&repeatCount=100000"
    steps:
      - to: "mock:result"
expects:
  mock:result:
    count: 100000
"#;
    let result = run(yaml, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 1);
    let er = &result.endpoint_results[0];
    assert_eq!(er.endpoint, "<settle>");
    let err = er.outcome.as_ref().expect_err("must fail"); // allow-unwrap
    assert!(err.contains("settle timeout"), "error text: {err}");
}

// ---------------------------------------------------------------------------
// routeFilesFromRoot (root-anchored test documents)
// ---------------------------------------------------------------------------

/// Write the standard `routeFilesFromRoot` project fixture under `root`:
/// `Camel.toml` at the top, `routes/orders.yaml`, and a nested document at
/// `tests/integration/orders.test.yaml`. Returns the absolute document path.
fn write_from_root_project(root: &std::path::Path) -> PathBuf {
    fs::write(root.join("Camel.toml"), "[default]\n").expect("write Camel.toml"); // allow-unwrap
    fs::create_dir_all(root.join("routes")).expect("create routes/"); // allow-unwrap
    fs::write(
        root.join("routes/orders.yaml"),
        r#"routes:
  - id: orders
    from: "direct:start"
    steps:
      - to: "mock:result"
"#,
    )
    .expect("write routes/orders.yaml"); // allow-unwrap
    let tests_dir = root.join("tests").join("integration");
    fs::create_dir_all(&tests_dir).expect("create tests/integration/"); // allow-unwrap
    let doc = tests_dir.join("orders.test.yaml");
    fs::write(
        &doc,
        r#"routeFilesFromRoot:
  - routes/orders.yaml
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:result:
    count: 1
"#,
    )
    .expect("write orders.test.yaml"); // allow-unwrap
    doc
}

/// Run one document through the in-process `camel test` entry, returning the
/// summary plus captured stdout/stderr text.
async fn run_cli(
    doc: &std::path::Path,
) -> (camel_cli::commands::test::TestRunSummary, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(std::slice::from_ref(&doc.to_path_buf()), &mut out, &mut err).await;
    let out = String::from_utf8(out).expect("stdout utf-8"); // allow-unwrap
    let err = String::from_utf8(err).expect("stderr utf-8"); // allow-unwrap
    (summary, out, err)
}

/// A document nested under `tests/integration/` resolves its
/// `routeFilesFromRoot` entries against the project root's `Camel.toml`.
#[tokio::test(flavor = "multi_thread")]
async fn route_files_from_root_nested_doc_passes() {
    let dir = temp_dir("rfr-nested");
    let doc = write_from_root_project(&dir);
    let (summary, out, err) = run_cli(&doc).await;
    assert_eq!(summary.exit_code, 0, "out:\n{out}\nerr:\n{err}");
    assert_eq!(summary.failed, 0, "out:\n{out}\nerr:\n{err}");
    let pass_lines = out.lines().filter(|l| l.starts_with("PASS ")).count();
    assert_eq!(pass_lines, 1, "exactly one PASS line expected, out:\n{out}");
}

/// Nearest ancestor wins: the per-service `Camel.toml` under
/// `services/orders/` anchors the document even though an outer `Camel.toml`
/// also exists, and `routes/a.yaml` exists only under `services/orders/`.
#[tokio::test(flavor = "multi_thread")]
async fn route_files_from_root_nearest_ancestor_wins() {
    let outer = temp_dir("rfr-ancestor");
    fs::write(outer.join("Camel.toml"), "[default]\n").expect("write outer Camel.toml"); // allow-unwrap
    let svc = outer.join("services").join("orders");
    fs::create_dir_all(svc.join("routes")).expect("create services/orders/routes/"); // allow-unwrap
    fs::create_dir_all(svc.join("tests")).expect("create services/orders/tests/"); // allow-unwrap
    fs::write(svc.join("Camel.toml"), "[default]\n").expect("write svc Camel.toml"); // allow-unwrap
    fs::write(
        svc.join("routes").join("a.yaml"),
        r#"routes:
  - id: a
    from: "direct:start"
    steps:
      - to: "mock:svc"
"#,
    )
    .expect("write routes/a.yaml"); // allow-unwrap
    let doc = svc.join("tests").join("a.test.yaml");
    fs::write(
        &doc,
        r#"routeFilesFromRoot:
  - routes/a.yaml
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:svc:
    count: 1
"#,
    )
    .expect("write a.test.yaml"); // allow-unwrap
    let (summary, out, err) = run_cli(&doc).await;
    assert_eq!(summary.exit_code, 0, "out:\n{out}\nerr:\n{err}");
    assert_eq!(summary.failed, 0, "out:\n{out}\nerr:\n{err}");
    let pass_lines = out.lines().filter(|l| l.starts_with("PASS ")).count();
    assert_eq!(pass_lines, 1, "exactly one PASS line expected, out:\n{out}");
}

/// No `Camel.toml` in any ancestor: exit code 2 with a `NoProjectRoot`
/// error naming the document directory (fail closed).
#[tokio::test(flavor = "multi_thread")]
async fn route_files_from_root_no_root_exit_2() {
    let dir = temp_dir("rfr-no-root");
    let doc = dir.join("orders.test.yaml");
    fs::write(
        &doc,
        r#"routeFilesFromRoot:
  - routes/orders.yaml
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:result:
    count: 1
"#,
    )
    .expect("write orders.test.yaml"); // allow-unwrap
    let (summary, out, err) = run_cli(&doc).await;
    assert_eq!(summary.exit_code, 2, "out:\n{out}\nerr:\n{err}");
    assert!(
        err.contains("NoProjectRoot"),
        "error must identify NoProjectRoot, err:\n{err}"
    );
    let doc_dir = dir.display().to_string();
    assert!(
        err.contains(&doc_dir),
        "error must name the document directory {doc_dir}, err:\n{err}"
    );
}

/// cwd independence: a child `camel test <absolute doc path>` run from an
/// unrelated working directory still resolves `routeFilesFromRoot` through
/// the root walk-up, with no `../` climbing in the document. The test never
/// mutates its own process working directory.
#[test]
fn route_files_from_root_cwd_independent_subprocess() {
    let project = temp_dir("rfr-subproc-project");
    let doc = write_from_root_project(&project);
    let elsewhere = temp_dir("rfr-subproc-elsewhere");

    let doc_text = fs::read_to_string(&doc).expect("read orders.test.yaml"); // allow-unwrap
    assert!(
        !doc_text.contains("../"),
        "document must not climb with ../, doc:\n{doc_text}"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc)
        .current_dir(&elsewhere)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit {:?} from cwd {};\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
        elsewhere.display(),
    );
}
