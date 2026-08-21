//! Integration tests for the in-process mock test runner
//! (`camel_cli::commands::test::runner`).

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use camel_cli::commands::test::document::parse_test_document;
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
