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
    run_job_args(dir, &[doc])
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
        dir.path().join("job.job.yaml"),
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

    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
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
        dir.path().join("job.job.yaml"),
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

    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
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
        dir.path().join("job.job.yaml"),
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

    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
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

/// Batch mode with NO seda consumer routes: the drain's expected queue
/// set is empty, so the drain completes immediately and the run keeps
/// the one-shot shape — exit 0, `Completed`, mode `batch`, and the
/// direct-transform reply intact.
#[test]
fn batch_no_seda_routes_completes_immediately() {
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
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: batch
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

    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(report["mode"], "batch");
    assert_eq!(report["reply"]["body"], "job-done", "report: {report}");
}

/// Read `name` under `dir`, retrying until it exists (up to 2 s) —
/// the process has already exited, so the retry only smooths FS
/// visibility, not progress.
fn read_eventually(dir: &Path, name: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
            return text;
        }
        if std::time::Instant::now() >= deadline {
            panic!("{name} missing under {} after 2 s", dir.display());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The batch drain: a direct target fans out to three seda workers,
/// each writing a file. The fire-and-forget seda sends return before
/// the workers run; the batch mode must wait for every seda queue to
/// drain (two consecutive zero-depth samples per queue) before
/// teardown, so all three files exist after exit 0.
#[test]
fn batch_drains_fanout_until_empty_exits_0() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    let routes = format!(
        r#"routes:
  - id: "fan"
    from: "direct:fan"
    steps:
      - to: "seda:w1"
      - to: "seda:w2"
      - to: "seda:w3"
  - id: "w1"
    from: "seda:w1"
    steps:
      - to: "file:{base}?fileName=w1.txt"
  - id: "w2"
    from: "seda:w2"
    steps:
      - to: "file:{base}?fileName=w2.txt"
  - id: "w3"
    from: "seda:w3"
    steps:
      - to: "file:{base}?fileName=w3.txt"
"#,
        base = dir.path().display()
    );
    std::fs::write(dir.path().join("routes/job-route.yaml"), routes).expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: batch
  timeout: 60s
  send:
    to: direct:fan
    body: "m"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["mode"], "batch", "report: {report}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    for name in ["w1.txt", "w2.txt", "w3.txt"] {
        let text = read_eventually(dir.path(), name);
        assert!(
            text.contains('m'),
            "{name} must contain the routed body; got: {text}"
        );
    }
}

/// In-flight coupling: seda's `DepthGuard` keeps queue depth >= 1 while
/// an envelope is endpoint-resident — queued or being forwarded
/// (crates/components/camel-component-seda/src/lib.rs ~803-805) — so the
/// drain loop cannot see this queue empty while the send/forward path
/// holds it. Once forwarded, the exchange lives in route-pipeline
/// residency the endpoint gauge cannot see; the drain's 2.5 s
/// zero-streak window (batch.rs `DRAIN_ZERO_SAMPLES_REQUIRED`) outlasts
/// this fixture's 1.5 s worker residency, so the file exists by the
/// time the gate — and then the process — completes, without relying on
/// teardown's in-flight wait. If seda/route stop ever gains in-flight
/// drain, the immediate-read assertion may pass via teardown alone —
/// re-point this test; the DETERMINISTIC regression net for a broken
/// drain loop is `batch_timeout_expires_with_timeout_outcome`
/// (a no-op drain would exit 0 there), not this test.
#[test]
fn batch_waits_for_in_flight_worker() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    let routes = format!(
        r#"routes:
  - id: "fan"
    from: "direct:fan"
    steps:
      - to: "seda:slow"
  - id: "slow"
    from: "seda:slow"
    steps:
      - delay: 1500
      - to: "file:{base}?fileName=slow.txt"
"#,
        base = dir.path().display()
    );
    std::fs::write(dir.path().join("routes/job-route.yaml"), routes).expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: batch
  timeout: 60s
  send:
    to: direct:fan
    body: "m"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    // Deliberately NO retry: the write lands while the drain gate still
    // counts the in-flight exchange, so the file must exist the moment
    // the process has exited.
    let text = std::fs::read_to_string(dir.path().join("slow.txt"))
        .expect("slow.txt must exist immediately after exit (no retry)");
    assert!(
        text.contains('m'),
        "slow.txt must contain the routed body; got: {text}"
    );
}

/// The batch overall timeout on a queue that never drains: a
/// self-feeding seda route re-enqueues every message it consumes, so the
/// drain gate can never accumulate the 10 consecutive zero samples it
/// requires (batch.rs `DRAIN_ZERO_SAMPLES_REQUIRED`): 10 samples span
/// 2.5 s at the 250 ms sampler cadence, more than this fixture's 2 s
/// deadline can ever admit — the `Timeout` verdict is
/// scheduling-independent, exit 2 within a bounded wall clock. This
/// doubles as the deterministic
/// regression net for a no-op drain loop: without a real drain this run
/// would exit 0.
#[test]
fn batch_timeout_expires_with_timeout_outcome() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/loop-route.yaml"),
        r#"routes:
  - id: "loop"
    from: "seda:loop"
    steps:
      - to: "seda:loop"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: batch
  timeout: 2s
  send:
    to: seda:loop
routeFiles:
  - routes/loop-route.yaml
"#,
    )
    .expect("write job doc");

    let started = std::time::Instant::now();
    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
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
        report["error"]
            .as_str()
            .is_some_and(|e| e.contains("timed out")),
        "report: {report}"
    );
    assert!(
        report.get("shutdown_error").is_none(),
        "zero-budget teardown artifact must not surface as shutdown_error; report: {report}"
    );
}

/// Batch mode + `--arg` injection: the CLI arg reaches the seda worker
/// as a header, the worker interpolates it before the file write —
/// exit 0, `Completed`, and the tagged file carries the injected value.
#[test]
fn batch_works_with_arg_injection() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    let routes = format!(
        r#"routes:
  - id: "fan"
    from: "direct:fan"
    steps:
      - to: "seda:w1"
  - id: "w1"
    from: "seda:w1"
    steps:
      - transform: {{simple: "id-${{header.batch-id}}"}}
      - to: "file:{base}?fileName=tagged.txt"
"#,
        base = dir.path().display()
    );
    std::fs::write(dir.path().join("routes/job-route.yaml"), routes).expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: batch
  timeout: 60s
  send:
    to: direct:fan
    body: "m"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) =
        run_job_args(dir.path(), &["job.job.yaml", "--arg", "batch-id=42"]);
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    let text = read_eventually(dir.path(), "tagged.txt");
    assert!(
        text.contains("id-42"),
        "tagged.txt must carry the injected arg; got: {text}"
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
        dir.path().join("job.job.yaml"),
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

    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
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
        dir.path().join("job.job.yaml"),
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
    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
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

/// Multiroute hop with a helper route declared `auto_startup: false`:
/// every document route starts, so the helper still consumes the target
/// route's `direct:` hop. The seda side route is targeted with
/// `waitForTaskToComplete=Always` so its file write completes before the
/// target route returns — seda `stop()` aborts forwarders without
/// draining in-flight work, so without `Always` the write would be a
/// scheduling race the process exit would lose.
#[test]
fn multiroute_direct_hop_with_autostart_false_helper_completes() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    let file_uri = format!("file:{}?fileName=side.txt", dir.path().display());
    let routes = format!(
        r#"routes:
  - id: "job-target"
    from: "direct:start"
    steps:
      - to: "direct:enrich"
      - to: "seda:side?waitForTaskToComplete=Always"
  - id: "job-enrich"
    from: "direct:enrich"
    auto_startup: false
    steps:
      - set_body:
          value: "enriched"
  - id: "job-side"
    from: "seda:side"
    steps:
      - to: "{file_uri}"
"#
    );
    std::fs::write(dir.path().join("routes/job-route.yaml"), routes).expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:start
    body: "ping"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(
        report["reply"]["body"], "enriched",
        "the auto_startup: false helper route must be forced on and serve the hop; report: {report}"
    );
    let side = std::fs::read_to_string(dir.path().join("side.txt"))
        .expect("side.txt written by the seda side route");
    assert!(
        side.contains("enriched"),
        "side route must write the enriched body; got: {side}"
    );
}

// ── Bare-name resolution + optional document (job-ux-reshape) ──────────

/// Run `camel job <args...>` in `dir` with arbitrary args (no Path
/// coercion) and return `(exit_code, stdout, stderr)`. Delegates to the
/// shared [`common::run_binary`] runner, which these job tests keep
/// covered end to end.
fn run_job_args(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let mut full: Vec<&str> = vec!["job"];
    full.extend(args.iter().copied());
    common::run_binary(dir, Path::new(env!("CARGO_BIN_EXE_camel")), &full, &[])
}

/// The canonical bare-name fixture: `jobs/` dir + `routeFilesFromRoot`
/// (the blessed mechanism for a separate jobs dir, ADR-0062 Rule 3).
fn write_bare_name_fixture(dir: &Path) {
    write_config(dir);
    std::fs::create_dir_all(dir.join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-transform"
    from: "direct:transform"
    steps:
      - set_body:
          value: "job-done"
"#,
    )
    .expect("write route");
    std::fs::create_dir_all(dir.join("jobs")).expect("mkdir jobs");
    std::fs::write(
        dir.join("jobs/job.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: "ping"
routeFilesFromRoot:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");
}

#[test]
fn bare_name_resolves_from_jobs_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bare_name_fixture(dir.path());
    let (code, stdout, stderr) = run_job(dir.path(), "job");
    assert_eq!(
        code, 0,
        "bare name must resolve jobs/job.job.yaml;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
}

#[test]
fn bare_name_miss_single_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bare_name_fixture(dir.path());
    let (code, stdout, stderr) = run_job(dir.path(), "nope");
    assert_eq!(
        code, 2,
        "bare miss is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("no job `nope`"),
        "stderr must carry the miss error; got:\n{stderr}"
    );
    assert_eq!(
        stderr.matches("nope.job.yaml").count(),
        1,
        "exactly one mention of the probed file; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "no report on a miss; got:\n{stdout}"
    );
}

#[test]
fn explicit_path_wins_over_jobs_resolution() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir_all(dir.path().join("ops")).expect("mkdir ops");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
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
    // Explicit .job.yml spelling in a non-jobs dir; no jobs/ dir exists at
    // all — resolution must not probe it.
    std::fs::write(
        dir.path().join("ops/x.job.yml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:transform
    body: "ping"
routeFilesFromRoot:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "ops/x.job.yml");
    assert_eq!(
        code, 0,
        "explicit path is used as-is;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
}

#[test]
fn route_source_still_mandatory_no_routes_fallback() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A routes/ dir with a REAL route exists and Camel.toml's routes glob
    // covers it — the missing route source must still fail, never fall
    // back to routes/ discovery.
    write_bare_name_fixture(dir.path());
    std::fs::write(
        dir.path().join("jobs/nosrc.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:transform
    body: "ping"
"#,
    )
    .expect("write job doc without a route source");

    let (code, stdout, stderr) = run_job(dir.path(), "nosrc");
    assert_eq!(
        code, 2,
        "missing route source is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("exactly one route source"),
        "stderr must carry the route-source error; got:\n{stderr}"
    );
    assert!(
        !stderr.contains("routes/*.yaml"),
        "no routes/ glob fallback may occur; got:\n{stderr}"
    );
}

#[test]
fn jobs_dir_anchored_at_config_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bare_name_fixture(dir.path());
    let nested = dir.path().join("nested/deeper");
    std::fs::create_dir_all(&nested).expect("mkdir nested");
    // From a nested CWD, --config pointing at the root Camel.toml: the
    // bare name must resolve the ROOT jobs/ dir, not ./jobs/ under CWD.
    let config = dir.path().join("Camel.toml");
    let config_arg = config.to_str().expect("path is valid utf-8");
    let (code, stdout, stderr) = run_job_args(&nested, &["--config", config_arg, "job"]);
    assert_eq!(
        code, 0,
        "bare name anchors at the Camel.toml root;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
}

#[test]
fn report_without_document_is_usage_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bare_name_fixture(dir.path());
    let (code, stdout, stderr) = run_job_args(dir.path(), &["--report", "out.json"]);
    assert_eq!(
        code, 2,
        "--report without a document is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("--report requires a job document"),
        "stderr must name the usage error; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "no listing, no report on stdout; got:\n{stdout}"
    );
}

// ── No-argument listing (job-ux-reshape) ───────────────────────────────

#[test]
fn listing_shows_names_and_descriptions() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bare_name_fixture(dir.path());
    std::fs::write(
        dir.path().join("jobs/reindex.job.yaml"),
        "execute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\nrouteFilesFromRoot:\n  - routes/job-route.yaml\n",
    )
    .expect("write reindex job (no description)");
    // Give the first job a description.
    std::fs::write(
        dir.path().join("jobs/job.job.yaml"),
        "description: create things via direct:in\nexecute:\n  mode: one-shot\n  timeout: 60s\n  send:\n    to: direct:transform\n    body: ping\nrouteFilesFromRoot:\n  - routes/job-route.yaml\n",
    )
    .expect("rewrite job with description");

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines[0], "Jobs in jobs/:",
        "header names the dir; got:\n{stdout}"
    );
    let job_line = lines
        .iter()
        .find(|l| l.starts_with("  job "))
        .expect("job row listed");
    assert!(
        job_line.contains("create things via direct:in"),
        "job row carries its description; got:\n{stdout}"
    );
    let reindex = lines
        .iter()
        .find(|l| l.starts_with("  reindex "))
        .expect("reindex listed");
    assert!(
        reindex.contains("(no description)"),
        "reindex has no description; got:\n{stdout}"
    );
}

#[test]
fn listing_empty_dir_exit_0() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir_all(dir.path().join("jobs")).expect("mkdir jobs");
    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "empty dir is exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("No jobs found in jobs/") && stdout.contains("<name>.job.yaml"),
        "friendly hint on stdout; got:\n{stdout}"
    );
}

#[test]
fn listing_absent_dir_exit_0() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "absent dir is exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("No jobs found in jobs/"),
        "friendly hint on stdout; got:\n{stdout}"
    );
}

#[test]
fn listing_unparseable_sibling() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bare_name_fixture(dir.path());
    std::fs::write(dir.path().join("jobs/broken.job.yaml"), "{not yaml").expect("write broken");
    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "unparseable sibling keeps listing at exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("broken") && stdout.contains("(unparseable)"),
        "broken sibling shows as unparseable; got:\n{stdout}"
    );
    assert!(
        stdout.contains("job"),
        "valid job still listed; got:\n{stdout}"
    );
}

#[test]
fn listing_job_yml_not_bare_resolvable() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir_all(dir.path().join("jobs")).expect("mkdir jobs");
    std::fs::write(
        dir.path().join("jobs/legacy.job.yml"),
        "execute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\n",
    )
    .expect("write legacy job.yml");

    let (code, stdout, _stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(code, 0, "listing exits 0; got:\n{stdout}");
    assert!(
        stdout.contains("legacy"),
        "the .job.yml is listed by its stripped name; got:\n{stdout}"
    );

    let (code, _out, stderr) = run_job(dir.path(), "legacy");
    assert_eq!(
        code, 2,
        "bare token does not resolve .job.yml; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("no job `legacy`") && stderr.contains("legacy.job.yaml"),
        "miss error names the probed .job.yaml; got:\n{stderr}"
    );
}

#[test]
fn listing_multiline_description_one_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir_all(dir.path().join("jobs")).expect("mkdir jobs");
    std::fs::write(
        dir.path().join("jobs/multi.job.yaml"),
        "description: |\n  first line\n  second line\nexecute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\n",
    )
    .expect("write multi job");
    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let multi_line = stdout
        .lines()
        .find(|l| l.contains("multi"))
        .expect("multi listed");
    assert!(
        multi_line.contains("first line") && multi_line.contains("second line"),
        "both parts on the listing row; got:\n{stdout}"
    );
    assert!(
        multi_line.matches("multi").count() == 1,
        "single row for the job; got:\n{stdout}"
    );
    assert_eq!(
        stdout
            .lines()
            .filter(|l| l.contains("first line") || l.contains("second line"))
            .count(),
        1,
        "description renders on ONE line; got:\n{stdout}"
    );
}

#[test]
fn listing_anchored_at_config_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bare_name_fixture(dir.path());
    let nested = dir.path().join("nested/deeper");
    std::fs::create_dir_all(&nested).expect("mkdir nested");
    // A decoy ./jobs under the CWD must NOT be what gets listed.
    std::fs::create_dir_all(nested.join("jobs")).expect("mkdir decoy jobs");
    std::fs::write(
        nested.join("jobs/decoy.job.yaml"),
        "description: wrong dir\nexecute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\n",
    )
    .expect("write decoy");
    let config = dir.path().join("Camel.toml");
    let config_arg = config.to_str().expect("path is valid utf-8");
    let (code, stdout, stderr) = run_job_args(&nested, &["--config", config_arg]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("job") && !stdout.contains("decoy"),
        "ROOT jobs/ is listed, not ./jobs/ relative to CWD; got:\n{stdout}"
    );
}

// ── --arg header injection (add-job-args-batch) ────────────────────────

/// `--arg NAME=VALUE` pairs reach the route as message headers: the flag
/// is repeatable, and two distinct names both interpolate in a single
/// simple expression.
#[test]
fn arg_single_and_repeated_reach_route_as_headers() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-arg-headers"
    from: "direct:transform"
    steps:
      - transform: {simple: "${header.name}-${header.tier}"}
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: "x"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job_args(
        dir.path(),
        &["job.job.yaml", "--arg", "name=John", "--arg", "tier=gold"],
    );
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(report["reply"]["body"], "John-gold", "report: {report}");
}

/// A CLI `--arg` overrides a colliding document `send.headers` entry:
/// CLI values are applied after document headers, so the last write
/// wins.
#[test]
fn arg_overrides_document_header() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-arg-override"
    from: "direct:transform"
    steps:
      - transform: {simple: "${header.name}"}
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: "x"
    headers:
      name: Doc
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job_args(dir.path(), &["job.job.yaml", "--arg", "name=Cli"]);
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["reply"]["body"], "Cli", "report: {report}");
}

/// A malformed `--arg` value (no `=`, or an empty name) is a clap usage
/// error: exit 2 with the value-parser message on stderr.
#[test]
fn malformed_arg_is_usage_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bare_name_fixture(dir.path());

    let (code, _stdout, stderr) = run_job_args(dir.path(), &["job.job.yaml", "--arg", "noequals"]);
    assert_eq!(code, 2, "missing = is a usage error; stderr:\n{stderr}");
    assert!(
        stderr.contains("expected NAME=VALUE"),
        "stderr must carry the usage error; got:\n{stderr}"
    );

    let (code, _stdout, stderr) = run_job_args(dir.path(), &["job.job.yaml", "--arg", "=value"]);
    assert_eq!(code, 2, "empty name is a usage error; stderr:\n{stderr}");
    assert!(
        stderr.contains("name is empty"),
        "stderr must carry the usage error; got:\n{stderr}"
    );
}
