//! End-to-end bundle route for template interpolation (mission 359,
//! Task 2.4).
//!
//! One YAML route drives the whole template wave through the EXISTING
//! scenario/log-capture harness: the top-level `set_body` string with a
//! Simple slot interpolates, the file producer resolves its `fileName`
//! Simple template, and the `log:` marker observes the post-file body.
//! Both the captured log event and the written file bytes are read
//! back, so no hand-built runtime is involved.

mod common;

use camel_integration_test::ScenarioVerdict;
use common::{lock_run, run_logs_document};

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn simple_template_bundle_route() {
    let out_dir = tempfile::tempdir().expect("temp dir");
    let doc = format!(
        r#"
env:
  OUT_DIR: "{}"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: x
    headers:
      id: X
logs:
  contains: ['Got -> X']
"#,
        out_dir.path().display()
    );

    let _guard = lock_run();
    let outcome = run_logs_document(&doc, "simple-template.routes.yaml").await;

    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the template route must pass: {outcome:?}"
    );
    assert!(outcome.logs_failure.is_none());

    let written = std::fs::read_to_string(out_dir.path().join("X-01.txt"))
        .expect("the file producer must have written X-01.txt");
    assert_eq!(written, "X-X", "the interpolated body is the file content");
}
