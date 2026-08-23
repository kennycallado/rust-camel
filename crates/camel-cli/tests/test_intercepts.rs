use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use camel_cli::commands::test::document::parse_test_document;
use camel_cli::commands::test::run_tests;
use camel_cli::commands::test::runner::run_test_doc;

mod common;

use common::{drain_to_buffer, send_term, spawn_camel_run, wait_exit_bounded, wait_for_marker};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "camel-test-intercepts-{tag}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn skip_to_unregistered_component_passes() {
    let dir = temp_dir("skip-unregistered");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "kafka:orders"
inputs:
  - to: "direct:start"
    body: "x"
intercepts:
  kafka:orders:
    skipTo: mock:orders
expects:
  mock:orders:
    count: 1
    bodies: ["x"]
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _mock) = run_test_doc(&doc, &dir).await;
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

#[tokio::test(flavor = "multi_thread")]
async fn intercept_target_and_expects_meet_on_endpoint() {
    let dir = temp_dir("naming-bridge");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "kafka:orders"
inputs:
  - to: "direct:start"
    body: "x"
intercepts:
  kafka:orders:
    skipTo: mock:orders
expects:
  mock:orders:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, mock) = run_test_doc(&doc, &dir).await;
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
    let inner = mock
        .get_endpoint("orders")
        .expect("mock:orders endpoint exists"); // allow-unwrap
    assert_eq!(inner.received_count().await, 1);
}

// ---------------------------------------------------------------------------
// Task 3: execution semantics matrix
// ---------------------------------------------------------------------------

/// Divert copies the exchange to the mock BEFORE the real send, and the real
/// send continues: the pipeline proceeds past the intercepted `seda:audit`
/// send to `mock:sink`, the seda consumer route drains the real queue into
/// `mock:drained`, and the divert copy lands on `mock:audit`. All three
/// expectations must be satisfied with one input and no document error.
#[tokio::test(flavor = "multi_thread")]
async fn divert_copies_while_real_seda_receives() {
    let dir = temp_dir("divert-seda");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "seda:audit"
      - to: "mock:sink"
  - id: r2
    from: "seda:audit"
    steps:
      - to: "mock:drained"
inputs:
  - to: "direct:start"
    body: "x"
intercepts:
  seda:audit:
    divertCopyTo: mock:audit
expects:
  mock:audit:
    count: 1
  mock:drained:
    count: 1
  mock:sink:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _mock) = run_test_doc(&doc, &dir).await;
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), 3);
    for er in &result.endpoint_results {
        assert!(
            er.outcome.is_ok(),
            "endpoint {} failed: {:?}",
            er.endpoint,
            er.outcome
        );
    }
}

/// Divert on an unregistered real component fails at route load: the divert
/// composes the copy stage in front of the REAL producer, so `kafka:orders`
/// must resolve — it cannot, and the enriched `ComponentNotFound` naming
/// `kafka` surfaces as a document error (exit-2 class, unchanged).
#[tokio::test(flavor = "multi_thread")]
async fn divert_unregistered_fails_route_load() {
    let dir = temp_dir("divert-unregistered");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "kafka:orders"
inputs:
  - to: "direct:start"
    body: "x"
intercepts:
  kafka:orders:
    divertCopyTo: mock:orders
expects:
  mock:orders:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _mock) = run_test_doc(&doc, &dir).await;
    let err = result.doc_error.expect("doc_error must be Some"); // allow-unwrap
    assert!(
        err.contains("kafka"),
        "doc_error must name the unresolvable component, got: {err}"
    );
}

/// Intercept source URIs match verbatim: a route sending to
/// `kafka:orders?x=1` does NOT match the rule key `kafka:orders`, so no rule
/// applies and resolution of the unregistered kafka component fails at route
/// load, surfacing as a document error naming `kafka`.
#[tokio::test(flavor = "multi_thread")]
async fn source_query_params_are_significant() {
    let dir = temp_dir("query-significant");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "kafka:orders?x=1"
inputs:
  - to: "direct:start"
    body: "x"
intercepts:
  kafka:orders:
    skipTo: mock:orders
expects:
  mock:orders:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _mock) = run_test_doc(&doc, &dir).await;
    let err = result.doc_error.expect("doc_error must be Some"); // allow-unwrap
    assert!(
        err.contains("kafka"),
        "doc_error must name the unresolvable component, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// camel run non-interference (subprocess)
// ---------------------------------------------------------------------------

/// `camel run` must never read the `intercepts` block. Route discovery skips
/// `*.test.yaml` documents (reserved test suffix), so a colocated test
/// document declaring an intentionally INVALID intercept block is a no-op for
/// the production run: the process starts, stays alive, and neither output
/// stream names the test document, `intercepts`, or any intercept validation
/// error. Had `camel run` parsed the document, the `mock:` source would fail
/// validation loudly (exit-2 class).
#[test]
fn camel_run_ignores_intercepts_block() {
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
    // Intentionally invalid intercept block: a `mock:` source would fail
    // validation loudly if `camel run` ever parsed this document.
    std::fs::write(
        config_dir.join("orders.test.yaml"),
        r#"routeFiles: [routes.yaml]
inputs: []
intercepts:
  mock:result:
    skipTo: mock:intercepted
expects:
  mock:result:
    count: 1
"#,
    )
    .expect("write config/orders.test.yaml"); // allow-unwrap

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
    // line, which is printed only after discovery + context start succeeded.
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

    // Neither stream may name the test document, the intercepts block, or
    // carry a test-document validation error. Three targeted bans cover the
    // contract: the reserved-suffix filename, the `intercepts` YAML key, and
    // the mock-scheme validation fragments in BOTH quoting variants —
    // single-quoted `start with 'mock:'` (Stage A target check) and
    // backticked ``start with `mock:` `` (document source check) — so a
    // wrongly-parsed rule fails on whichever path it hits. The bans are
    // exact, so a benign log mentioning "interception" cannot flake the
    // test.
    let stdout_text = out_buf.lock().expect("stdout buffer lock poisoned").clone(); // allow-unwrap
    let stderr_text = err_buf.lock().expect("stderr buffer lock poisoned").clone(); // allow-unwrap
    for (stream, text) in [("stdout", &stdout_text), ("stderr", &stderr_text)] {
        assert!(
            !text.contains("orders.test.yaml"),
            "{stream} names the test document; camel run must skip *.test.yaml:\n{text}"
        );
        assert!(
            !text.contains("intercepts"),
            "{stream} names the intercepts block; camel run must not read it:\n{text}"
        );
        assert!(
            !text.contains("start with 'mock:'") && !text.contains("start with `mock:`"),
            "{stream} carries an intercept validation error; camel run must not parse \
             test documents:\n{text}"
        );
    }
}

/// Holistic-review F2 — multi-document isolation: each document boots a fresh
/// `CamelContext`, so intercepts declared in one document do not leak into
/// another. Document `a` intercepts the unregistered `kafka:orders` to
/// `mock:orders` (would fail to boot without the rule); document `b` has no
/// intercepts and sends to `mock:plain` plain. Run through the real multi-doc
/// driver (`run_tests`) in argument order and prove both docs pass — neither
/// a leak (`b` failing from `a`'s rule) nor a break (`a` failing to map).
#[tokio::test(flavor = "multi_thread")]
async fn mixed_multi_doc_run_isolated() {
    let dir = temp_dir("mixed-isolated");
    let a_path = dir.join("a.test.yaml");
    let b_path = dir.join("b.test.yaml");
    fs::write(
        &a_path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "kafka:orders"
inputs:
  - to: "direct:start"
    body: "a"
intercepts:
  kafka:orders:
    skipTo: mock:orders
expects:
  mock:orders:
    count: 1
    bodies: ["a"]
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
      - to: "mock:plain"
inputs:
  - to: "direct:start"
    body: "b"
expects:
  mock:plain:
    count: 1
    bodies: ["b"]
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
        out_str.contains("a.test.yaml#orders") || out_str.contains("a.test.yaml#mock:orders"),
        "out must contain PASS for a.test.yaml mock:orders; out: {out_str}"
    );
    assert!(
        out_str.contains("b.test.yaml#plain") || out_str.contains("b.test.yaml#mock:plain"),
        "out must contain PASS for b.test.yaml mock:plain; out: {out_str}"
    );
    assert!(
        out_str.contains("2 passed, 0 failed"),
        "out summary must be 2 passed, 0 failed; out: {out_str}"
    );
}
