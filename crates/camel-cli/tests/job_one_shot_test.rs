//! End-to-end integration tests for `camel job` (one-shot `execute:`
//! documents).
//!
//! Each test spawns the real `camel` binary in a fixture directory
//! (Camel.toml + route file + job document), runs `camel job <doc>`,
//! and asserts the exit code plus the JSON report on stdout. The
//! fixture sets `log_level = "off"` so stdout carries ONLY the JSON
//! report — the general tracing layer writes to stdout (camel-config
//! `init_tracing_subscriber`), so any log level would interleave with
//! the report.
//!
//! This is the 25th camel-cli integration-test binary (the gate count
//! moves 24 → 25).

mod common;

use std::path::Path;
use std::time::Duration;

use common::{drain_to_buffer, spawn_camel_job};

/// Write the fixture config: routes glob unused by the job (the job's
/// route source is the document), logs off for a clean stdout report.
fn write_config(dir: &Path) {
    std::fs::write(
        dir.join("Camel.toml"),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "off"
watch = false
"#,
    )
    .expect("write Camel.toml");
}

/// Run `camel job <doc>` in `dir` to completion and return
/// `(exit_code, stdout, stderr)`.
fn run_job(dir: &Path, doc: &str) -> (i32, String, String) {
    let mut child = spawn_camel_job(dir, Path::new(doc));
    let out_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let err_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let out_handle = std::thread::spawn({
        let buf = std::sync::Arc::clone(&out_buf);
        let stdout = child.stdout.take().expect("stdout piped");
        move || drain_to_buffer(stdout, buf)
    });
    let err_handle = std::thread::spawn({
        let buf = std::sync::Arc::clone(&err_buf);
        let stderr = child.stderr.take().expect("stderr piped");
        move || drain_to_buffer(stderr, buf)
    });

    // Generous bound: the binary boots the full bundle cascade, and a
    // whole-workspace `cargo test` slows subprocess startup ~100x.
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break -1;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    };
    let _ = out_handle.join();
    let _ = err_handle.join();
    (
        exit_code,
        out_buf.lock().expect("stdout lock").clone(),
        err_buf.lock().expect("stderr lock").clone(),
    )
}

/// The happy path: one direct: route that rewrites the body, a job send
/// with `capture-reply`, exit 0, and a Completed report whose reply body
/// matches the route's output.
#[test]
fn one_shot_direct_transform_completes_with_reply() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-transform"
    from: "direct:transform"
    steps:
      - set_body:
          value: "job-done"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.test.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: "ping"
    headers:
      X-Job: cli
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "job.test.yaml");
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(report["mode"], "one-shot");
    assert_eq!(report["terminated_early"], false);
    assert_eq!(report["reply"]["body"], "job-done", "report: {report}");
    assert!(report["duration_ms"].as_u64().is_some(), "report: {report}");
}

/// A route whose only side effect is a `log:` sink: the job completes
/// without a captured reply (no `capture-reply`), exit 0.
#[test]
fn one_shot_log_sink_route_completes() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/log-route.yaml"),
        r#"routes:
  - id: "job-log"
    from: "direct:tap"
    steps:
      - to: "log:job-out"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.test.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:tap
    body: "tap-me"
routeFiles:
  - routes/log-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "job.test.yaml");
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert!(report.get("reply").is_none(), "report: {report}");
}

/// The failure path: a route step that fails (a `direct:` producer with
/// no consumer) surfaces as pipeline failure — exit 1 with a `Failed`
/// report carrying the error.
#[test]
fn one_shot_pipeline_failure_exits_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/fail-route.yaml"),
        r#"routes:
  - id: "job-fail"
    from: "direct:boom"
    steps:
      - to: "direct:missing-consumer"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.test.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:boom
routeFiles:
  - routes/fail-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "job.test.yaml");
    assert_eq!(
        code, 1,
        "expected exit 1 (pipeline failure);\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Failed", "report: {report}");
    assert!(
        report["error"].as_str().is_some_and(|e| !e.is_empty()),
        "report: {report}"
    );
}

/// Load-time validation: `mode: batch` is parsed then rejected with the
/// reserved-mode error and exit 2 (no boot).
#[test]
fn batch_mode_is_rejected_at_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::write(
        dir.path().join("job.test.yaml"),
        r#"execute:
  mode: batch
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "job.test.yaml");
    assert_eq!(
        code, 2,
        "expected exit 2 (batch reserved);\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("not available yet"),
        "stderr must name the reserved mode:\n{stderr}"
    );
}

/// Seda verdict fidelity: the runner forces
/// `waitForTaskToComplete=Always` on seda targets, so a FAILING seda
/// route surfaces as pipeline failure (exit 1, outcome `Failed`) instead
/// of the fire-and-forget default reporting `Completed` on an InOnly
/// send.
#[test]
fn one_shot_seda_failing_route_reports_failed() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/seda-fail-route.yaml"),
        r#"routes:
  - id: "job-seda-fail"
    from: "seda:work"
    steps:
      - to: "direct:missing-consumer"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.test.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  send:
    to: seda:work
routeFiles:
  - routes/seda-fail-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "job.test.yaml");
    assert_eq!(
        code, 1,
        "expected exit 1 (seda pipeline failure must not be fire-and-forget);\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Failed", "report: {report}");
    assert!(
        report["error"].as_str().is_some_and(|e| !e.is_empty()),
        "report: {report}"
    );
}

/// The mandatory overall timeout: a slow route plus `timeout: 1s`
/// reports `Timeout` and exits 2, within a bounded wall clock (boot +
/// 1 s timeout + the 5 s shutdown floor; generously bounded at 15 s).
#[test]
fn one_shot_timeout_expires_with_timeout_outcome() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/slow-route.yaml"),
        r#"routes:
  - id: "job-slow"
    from: "direct:slow"
    steps:
      - delay: 30000
      - set_body:
          value: "never-reached"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.test.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 1s
  send:
    to: direct:slow
routeFiles:
  - routes/slow-route.yaml
"#,
    )
    .expect("write job doc");

    let started = std::time::Instant::now();
    let (code, stdout, stderr) = run_job(dir.path(), "job.test.yaml");
    let elapsed = started.elapsed();
    assert_eq!(
        code, 2,
        "expected exit 2 (overall timeout);\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "timeout run must stay bounded; took {elapsed:?}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Timeout", "report: {report}");
    assert!(
        report["error"].as_str().is_some_and(|e| !e.is_empty()),
        "report: {report}"
    );
}
