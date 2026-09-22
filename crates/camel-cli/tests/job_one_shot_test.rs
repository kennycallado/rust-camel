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

/// In-flight coupling: every accepted exchange holds an RAII
/// `InFlightClaim` across its whole lifecycle — seda queue residency,
/// dispatch, and route-pipeline residency (the batch drain contract in
/// `job/batch.rs`) — so while this fixture's 1.5 s worker delay parks
/// the exchange mid-pipeline, `CamelContext::total_in_flight()` reads
/// at least 1 and the drain gate's single-counter poll cannot report
/// zero before the file write completes. The file exists by the time
/// the gate — and then the process — completes, without relying on
/// teardown's in-flight wait. If claims ever stop spanning
/// route-pipeline residency, the immediate-read assertion may pass via
/// teardown alone — re-point this test; the DETERMINISTIC regression
/// net for a broken drain loop is
/// `batch_timeout_expires_with_timeout_outcome` (a no-op drain would
/// exit 0 there), not this test.
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
/// self-feeding seda route re-enqueues every message it consumes, and
/// each accepted exchange holds its `InFlightClaim` until its pipeline
/// completes, so the route always owns at least one
/// accepted-not-completed exchange and `CamelContext::total_in_flight()`
/// never reads zero (the batch drain contract in `job/batch.rs`). The
/// drain gate naps up to the overall `timeout` deadline — the `Timeout`
/// verdict is scheduling-independent, exit 2 within a bounded wall
/// clock. This doubles as the deterministic regression net for a no-op
/// drain loop: without a real drain this run would exit 0.
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

/// Exactly-once side effect before a never-activating seda consumer
/// (spec "pre-SEDA side effect executes exactly once"): the entry route
/// appends the body to a file, then targets `seda:worker` — a queue no
/// route consumes. The single-mode SEDA gate must fail the send
/// immediately (exit 1, `Failed` report naming the gate) with the
/// pre-SEDA file write executed exactly once: a retry replay of the
/// pipeline would leave `"ticktick"` in the file, so exact bytes are the
/// only discriminating assertion. No `duration_ms` check — the run is
/// boot-dominated and the wall-clock proof lives in the in-process
/// fail-fast tests.
#[test]
fn seda_gate_side_effect_executes_exactly_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    let routes = format!(
        r#"routes:
  - id: "job-gate"
    from: "direct:jobs"
    steps:
      - to: "file:{base}?fileName=count.txt&fileExist=append"
      - to: "seda:worker"
"#,
        base = dir.path().display()
    );
    std::fs::write(dir.path().join("routes/job-route.yaml"), routes).expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:jobs
    body: "tick"
routeFiles:
  - routes/job-route.yaml
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
        report["error"]
            .as_str()
            .is_some_and(|e| e.contains("has no active consumers")),
        "report must name the seda gate; report: {report}"
    );
    // The file write is synchronous with the send, so a plain read
    // after process exit suffices.
    let count = std::fs::read_to_string(dir.path().join("count.txt"))
        .expect("count.txt must hold the pre-SEDA side effect");
    assert_eq!(
        count, "tick",
        "side effect must execute exactly once (a replay would read \"ticktick\"); got: {count:?}"
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
fn execute_sequence_is_rejected_before_boot() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::write(
        dir.path().join("sequence.job.yaml"),
        r#"execute:
  - mode: one-shot
    timeout: 30s
    send:
      to: direct:first
  - mode: batch
    timeout: 30s
    send:
      to: direct:second
routes: []
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "sequence.job.yaml");
    assert_eq!(
        code, 2,
        "execute sequence must be a parser rejection;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "no report before boot; got:\n{stdout}"
    );
    assert!(
        stderr.contains("invalid job document") && stderr.contains("execute"),
        "stderr must carry the execute parse error; got:\n{stderr}"
    );
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

/// The declared-args tap job shared by the dynamic-flag e2e tests: one
/// required string argument interpolated into the reply body through
/// `${arg:name}` (the tap route echoes the body back into the report).
fn write_declared_name_fixture(dir: &Path) {
    write_config(dir);
    std::fs::create_dir(dir.join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-tap"
    from: "direct:tap"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.join("job.job.yaml"),
        r#"args:
  name:
    required: true
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:tap
    body: "hello ${arg:name}"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");
}

/// A declared argument set through the dynamic-flag form and through
/// the legacy `--arg` form records the same run: both exit 0 and the
/// two JSON reports match on the evidence fields exactly (`duration_ms`
/// is timing noise and excluded).
#[test]
fn dynamic_flag_one_shot_matches_arg_form() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_declared_name_fixture(dir.path());

    let (flag_code, flag_stdout, flag_stderr) =
        run_job_args(dir.path(), &["job.job.yaml", "--name", "world"]);
    assert_eq!(
        flag_code, 0,
        "dynamic-flag run must complete;\nstdout:\n{flag_stdout}\nstderr:\n{flag_stderr}"
    );
    let (arg_code, arg_stdout, arg_stderr) =
        run_job_args(dir.path(), &["job.job.yaml", "--arg", "name=world"]);
    assert_eq!(
        arg_code, 0,
        "--arg run must complete;\nstdout:\n{arg_stdout}\nstderr:\n{arg_stderr}"
    );

    let flag_report: serde_json::Value = serde_json::from_str(flag_stdout.trim())
        .expect("stdout is the JSON report; got:\n{flag_stdout}");
    let arg_report: serde_json::Value = serde_json::from_str(arg_stdout.trim())
        .expect("stdout is the JSON report; got:\n{arg_stdout}");
    for field in ["outcome", "mode", "terminated_early", "reply"] {
        assert_eq!(
            flag_report[field], arg_report[field],
            "reports must match on `{field}`;\nflag form:\n{flag_report}\narg form:\n{arg_report}"
        );
    }
    assert_eq!(flag_report["outcome"], "Completed", "report: {flag_report}");
    assert_eq!(
        flag_report["reply"]["body"], "hello world",
        "the interpolated ${{arg:name}} body must carry; report: {flag_report}"
    );
}

/// A misspelled dynamic flag is clap's unknown-argument usage error:
/// exit 2, stderr carries clap's diagnostic with the `--name`
/// did-you-mean suggestion, and nothing executes (no report on stdout).
#[test]
fn dynamic_flag_unknown_exits_two() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_declared_name_fixture(dir.path());

    let (code, stdout, stderr) = run_job_args(dir.path(), &["job.job.yaml", "--nmae", "x"]);
    assert_eq!(
        code, 2,
        "unknown dynamic flag must exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("unexpected argument '--nmae' found"),
        "stderr must carry clap's unknown-argument text; got:\n{stderr}"
    );
    assert!(
        stderr.contains("similar argument exists: '--name'"),
        "stderr must suggest the declared `--name`; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "usage error is stderr-only; got:\n{stdout}"
    );
}

/// The `--` argv terminator survives neither phase intact: clap strips
/// it before tail capture but keeps it inside an already-started tail,
/// so the boundary is recomputed from RAW argv. Post-terminator tokens
/// are literals — never help, never a config re-anchor, never dynamic
/// or static flags — and the first one fails as an unexpected
/// positional (exit 2). A terminator with nothing after it is
/// harmless: the run proceeds (flags spelled before it keep their
/// meaning).
#[test]
fn binary_terminator_preserved() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_declared_name_fixture(dir.path());

    // Stripped terminator (`doc -- --name x` captures the tail raw):
    // the post-`--` `--name` is a literal, not a flag.
    let (code, stdout, stderr) = run_job_args(dir.path(), &["job.job.yaml", "--", "--name", "x"]);
    assert_eq!(
        code, 2,
        "post-terminator tokens are not flags;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("unexpected positional '--name'"),
        "the first literal is named;\nstderr:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "usage error is stderr-only; got:\n{stdout}"
    );

    // Surviving terminator (`doc --name w -- --literal`): the flags
    // before it lower normally; everything after it is literal.
    let (code, stdout, stderr) = run_job_args(
        dir.path(),
        &["job.job.yaml", "--name", "w", "--", "--literal"],
    );
    assert_eq!(
        code, 2,
        "post-terminator literals exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("unexpected positional"),
        "the literal is named;\nstderr:\n{stderr}"
    );

    // Terminator before the path (`-- doc.yaml --name w`): the whole
    // captured tail is post-terminator.
    let (code, stdout, stderr) = run_job_args(dir.path(), &["--", "job.job.yaml", "--name", "w"]);
    assert_eq!(
        code, 2,
        "a terminator before the path literalizes the tail;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("unexpected positional '--name'"),
        "the first literal is named;\nstderr:\n{stderr}"
    );

    // A terminator with NOTHING after it is harmless: the run proceeds
    // and flags spelled before the surviving `--` keep their meaning.
    let (code, stdout, stderr) = run_job_args(dir.path(), &["job.job.yaml", "--name", "w", "--"]);
    assert_eq!(
        code, 0,
        "a bare trailing `--` must not fail;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(
        report["reply"]["body"], "hello w",
        "pre-terminator `--name w` lowers normally; report: {report}"
    );

    // Post-terminator `--help` does NOT render help.
    let (code, stdout, stderr) = run_job_args(dir.path(), &["job.job.yaml", "--", "--help"]);
    assert_eq!(
        code, 2,
        "post-terminator help is a literal;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("unexpected positional '--help'"),
        "the literal is named;\nstderr:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "no help on stdout;\nstdout:\n{stdout}"
    );

    // A trailing stripped terminator on an args-less document runs
    // fine (empty tail, empty literals).
    let dir = tempfile::tempdir().expect("tempdir");
    write_bare_name_fixture(dir.path());
    let (code, stdout, stderr) = run_job_args(dir.path(), &["jobs/job.job.yaml", "--"]);
    assert_eq!(
        code, 0,
        "`doc --` alone must run;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("\"Completed\""),
        "the job executed; stdout:\n{stdout}"
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
    assert!(
        lines.contains(&"job — create things via direct:in"),
        "job row carries its description; got:\n{stdout}"
    );
    assert!(
        lines.contains(&"reindex — (no description)"),
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

// ── Ordered discovery roots (jobdiscovery Task 2.1) ────────────────────

/// Write the fixture config with a `[jobs]` table body (`dirs`, legacy
/// `dir`, or both), logs off for a clean listing stdout.
fn write_config_with_jobs(dir: &Path, jobs_table: &str) {
    std::fs::write(
        dir.join("Camel.toml"),
        format!(
            "[default]\nroutes = [\"routes/*.yaml\"]\nlog_level = \"off\"\nwatch = false\n\n[default.jobs]\n{jobs_table}\n"
        ),
    )
    .expect("write Camel.toml");
}

/// Write one metadata-only job document: a description plus a minimal
/// `execute:` block. Listing never parses the route source, so listing
/// fixtures need no route file.
fn write_listing_job(path: &Path, description: &str) {
    std::fs::write(
        path,
        format!(
            "description: {description}\nexecute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\n"
        ),
    )
    .expect("write job doc");
}

/// Roots are scanned in `[jobs].dirs` declaration order, not stem
/// order: `zulu` (team-a) must list before `alpha` (team-b) even
/// though a global name sort would print them the other way.
#[test]
fn configured_job_dirs_are_scanned_in_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"team-a\", \"team-b\"]");
    std::fs::create_dir_all(dir.path().join("team-a")).expect("mkdir team-a");
    std::fs::create_dir_all(dir.path().join("team-b")).expect("mkdir team-b");
    write_listing_job(&dir.path().join("team-a/zulu.job.yaml"), "team a job");
    write_listing_job(&dir.path().join("team-b/alpha.job.yaml"), "team b job");

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![
            "Jobs in team-a/:",
            "zulu — team a job",
            "Jobs in team-b/:",
            "alpha — team b job",
        ],
        "roots scan in declaration order (team-a before team-b); got:\n{stdout}"
    );
}

/// `dirs` wins over legacy `dir` when both exist: only `first` and
/// `second` are scanned, and the legacy-only file never appears.
#[test]
fn explicit_job_dirs_override_legacy_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(
        dir.path(),
        "dir = \"legacy\"\ndirs = [\"first\", \"second\"]",
    );
    std::fs::create_dir_all(dir.path().join("legacy")).expect("mkdir legacy");
    std::fs::create_dir_all(dir.path().join("first")).expect("mkdir first");
    std::fs::create_dir_all(dir.path().join("second")).expect("mkdir second");
    write_listing_job(
        &dir.path().join("legacy/legacy-only.job.yaml"),
        "legacy job",
    );
    write_listing_job(&dir.path().join("first/first.job.yaml"), "");

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.lines().any(|l| l == "first — (no description)"),
        "exact first-root line; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("legacy-only"),
        "legacy dir must not be scanned when dirs is present; got:\n{stdout}"
    );
}

/// No `[jobs]` setting at all keeps the built-in `jobs` root.
#[test]
fn default_jobs_root_is_used() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir_all(dir.path().join("jobs")).expect("mkdir jobs");
    write_listing_job(&dir.path().join("jobs/only.job.yaml"), "the default job");

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Jobs in jobs/:") && stdout.contains("only — the default job"),
        "default jobs root is scanned; got:\n{stdout}"
    );
}

/// A bare name found in only the SECOND root still resolves: the probe
/// walks every configured root in order, the single match is selected,
/// and the successful run leaves stderr completely empty.
#[test]
fn named_job_uses_later_matching_configured_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"first\", \"second\"]");
    std::fs::create_dir_all(dir.path().join("first")).expect("mkdir first");
    std::fs::create_dir_all(dir.path().join("second")).expect("mkdir second");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-transform"
    from: "direct:transform"
    steps:
      - set_body:
          value: "second-marker"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("second/report.job.yaml"),
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

    let (code, stdout, stderr) = run_job(dir.path(), "report");
    assert_eq!(
        code, 0,
        "second-root match resolves and completes;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.is_empty(),
        "no miss or ambiguity on stderr; got:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(report["reply"]["body"], "second-marker", "report: {report}");
    assert!(
        report["document"]
            .as_str()
            .is_some_and(|d| d.ends_with("second/report.job.yaml")),
        "the second-root document ran; report: {report}"
    );
}

/// Descriptions render on one line, a descriptionless `.job.yml` lists
/// as `(no description)`, and a bare lookup of that yml stem misses
/// naming the `.job.yaml` probe.
#[test]
fn listing_formats_descriptions_and_yml_is_display_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir_all(dir.path().join("jobs")).expect("mkdir jobs");
    std::fs::write(
        dir.path().join("jobs/multi.job.yaml"),
        "description: |\n  first line\n  second line\nexecute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\n",
    )
    .expect("write multi job");
    write_listing_job(&dir.path().join("jobs/plain.job.yml"), "");

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let multi_line = stdout
        .lines()
        .find(|l| l.contains("first line"))
        .expect("multi listed");
    assert!(
        multi_line.contains("first line") && multi_line.contains("second line"),
        "both parts on the listing row; got:\n{stdout}"
    );
    assert_eq!(
        stdout
            .lines()
            .filter(|l| l.contains("first line") || l.contains("second line"))
            .count(),
        1,
        "description renders on ONE line; got:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|l| l == "plain — (no description)"),
        "the .job.yml stem lists with no description; got:\n{stdout}"
    );

    let (code, _stdout, stderr) = run_job(dir.path(), "plain");
    assert_eq!(
        code, 2,
        "bare token does not resolve .job.yml; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("no job `plain`") && stderr.contains("plain.job.yaml"),
        "miss error names the probed .job.yaml; got:\n{stderr}"
    );
}

/// A malformed sibling renders `(unparseable)` while the valid job
/// stays listed and the exit stays 0.
#[test]
fn malformed_job_sibling_does_not_abort_listing() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir_all(dir.path().join("jobs")).expect("mkdir jobs");
    write_listing_job(&dir.path().join("jobs/good.job.yaml"), "good job");
    std::fs::write(dir.path().join("jobs/bad.job.yaml"), "{not yaml").expect("write broken");

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "unparseable sibling keeps listing at exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.lines().any(|l| l == "good — good job"),
        "valid job listed; got:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|l| l == "bad — (unparseable)"),
        "malformed sibling labeled; got:\n{stdout}"
    );
}

/// A configured root that does not exist behaves like `ls`: the
/// creation hint prints and the listing exits 0.
#[test]
fn missing_job_root_is_successful() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"missing\"]");

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "absent root is exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("No jobs found in missing/") && stdout.contains("<name>.job.yaml"),
        "creation hint on stdout; got:\n{stdout}"
    );
}

/// A missing leading root never hides the roots after it.
#[test]
fn missing_first_root_does_not_hide_second() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"missing\", \"second\"]");
    std::fs::create_dir_all(dir.path().join("second")).expect("mkdir second");
    write_listing_job(&dir.path().join("second/only.job.yaml"), "second job");

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.lines().any(|l| l == "only — second job"),
        "second-root job listed after missing first root; got:\n{stdout}"
    );
}

/// The walk is lexical and never follows directory symlinks: `a/`
/// lists before `b/`, and a symlinked directory's target stays absent.
#[test]
fn job_listing_is_lexical_and_does_not_follow_directory_symlinks() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    let jobs = dir.path().join("jobs");
    std::fs::create_dir_all(jobs.join("a")).expect("mkdir a");
    std::fs::create_dir_all(jobs.join("b")).expect("mkdir b");
    std::fs::create_dir_all(dir.path().join("outside")).expect("mkdir outside");
    write_listing_job(&jobs.join("a/aa.job.yaml"), "a dir job");
    write_listing_job(&jobs.join("b/bb.job.yaml"), "b dir job");
    write_listing_job(&dir.path().join("outside/outsider.job.yaml"), "outside job");
    std::os::unix::fs::symlink("../outside", jobs.join("linked")).expect("symlink linked");

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![
            "Jobs in jobs/:",
            "a/aa.job.yaml — a dir job",
            "b/bb.job.yaml — b dir job",
        ],
        "lexical nested order, no symlinked-directory traversal; got:\n{stdout}"
    );
}

/// `--help` on a nested job renders the same invocable spelling the
/// listing shows: the configured-root-relative path as the header,
/// not the bare canonicalized file stem.
#[test]
fn nested_job_help_header_shows_invocable_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"jobs\"]");
    std::fs::create_dir_all(dir.path().join("jobs/daily")).expect("mkdir daily");
    std::fs::write(
        dir.path().join("jobs/daily/ingest.job.yaml"),
        "description: Daily ingest\nexecute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\nroutes:\n  - id: \"ingest\"\n    from: \"direct:noop\"\n",
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job_args(dir.path(), &["daily/ingest", "--help"]);
    assert_eq!(
        code, 0,
        "--help exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.lines().next(),
        Some("daily/ingest.job.yaml"),
        "help header is the invocable path; got:\n{stdout}"
    );
    assert!(
        stdout.contains("Daily ingest"),
        "help carries the description; got:\n{stdout}"
    );
}

/// The listing row's display name IS the invocable spelling: taking
/// the exact text before the ` — ` descriptor and running it as the
/// document argument completes the job — display and resolution can
/// never drift.
#[test]
fn listing_display_is_invocable_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
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
    std::fs::create_dir_all(dir.path().join("jobs/daily")).expect("mkdir daily");
    std::fs::write(
        dir.path().join("jobs/daily/ingest.job.yaml"),
        r#"description: Daily ingest
execute:
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

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let display = stdout
        .lines()
        .find_map(|l| l.strip_suffix(" — Daily ingest"))
        .unwrap_or_else(|| panic!("nested listing row present; got:\n{stdout}"));
    assert_eq!(
        display, "daily/ingest.job.yaml",
        "nested row is the root-relative path; got:\n{stdout}"
    );

    let (code, stdout, stderr) = run_job(dir.path(), display);
    assert_eq!(
        code, 0,
        "the listed spelling resolves and completes;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
}

/// The same bare name in two roots is an exit-2 ambiguity naming every
/// matching path, before any execution.
#[test]
fn named_job_collision_reports_all_matching_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"first\", \"second\"]");
    std::fs::create_dir_all(dir.path().join("first")).expect("mkdir first");
    std::fs::create_dir_all(dir.path().join("second")).expect("mkdir second");
    std::fs::write(
        dir.path().join("first/report.job.yaml"),
        "description: first-marker\nexecute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\n",
    )
    .expect("write first job doc");
    std::fs::write(
        dir.path().join("second/report.job.yaml"),
        "description: second-marker\nexecute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\n",
    )
    .expect("write second job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "report");
    assert_eq!(
        code, 2,
        "cross-root collision is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let first = std::fs::canonicalize(dir.path().join("first/report.job.yaml"))
        .expect("first fixture exists");
    let second = std::fs::canonicalize(dir.path().join("second/report.job.yaml"))
        .expect("second fixture exists");
    assert!(
        stderr.contains(first.display().to_string().as_str())
            && stderr.contains(second.display().to_string().as_str()),
        "stderr names both matching paths; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "no report on a collision; got:\n{stdout}"
    );
}

/// Spec scenario "Explicit path and bare-name miss", run from a NESTED
/// working directory: an explicit `.job.yml` outside every configured
/// root loads as-is (root probing never runs for explicit paths), while
/// a bare-name miss names the exact `<name>.job.yaml` probe in EVERY
/// configured root — anchored at the Camel.toml root, never the CWD —
/// and exits 2.
#[test]
fn explicit_job_path_bypasses_roots_and_bare_miss_is_named() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"first\", \"second\"]");
    std::fs::create_dir_all(dir.path().join("first")).expect("mkdir first");
    std::fs::create_dir_all(dir.path().join("second")).expect("mkdir second");
    std::fs::create_dir_all(dir.path().join("ops")).expect("mkdir ops");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-transform"
    from: "direct:transform"
    steps:
      - set_body:
          value: "explicit-marker"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("ops/one-shot.job.yml"),
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
    let ops = dir.path().join("ops");

    // Explicit path from the nested CWD: the document is outside both
    // roots (a bare `one-shot` probe would miss both) and must load
    // as-is through its `.yml` suffix.
    let (code, stdout, stderr) =
        run_job_args(&ops, &["--config", "../Camel.toml", "one-shot.job.yml"]);
    assert_eq!(
        code, 0,
        "explicit path is used as-is;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(
        report["reply"]["body"], "explicit-marker",
        "the nested explicit document ran; report: {report}"
    );

    // Bare-name miss from the same nested CWD: both probes anchor at
    // the Camel.toml root, not the process directory.
    let (code, stdout, stderr) = run_job_args(&ops, &["--config", "../Camel.toml", "missing"]);
    assert_eq!(
        code, 2,
        "bare miss is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("first/missing.job.yaml") && stderr.contains("second/missing.job.yaml"),
        "stderr names every configured root's probe; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "no report on a miss; got:\n{stdout}"
    );
}

// ---- Relative document-path resolution ladder (jobpath, rc-r63b1) --

/// One transform route for the resolution fixtures: `from` target,
/// constant reply body. Written under `routes/` (or beside an outside
/// document) and referenced from the job document.
fn write_resolution_route(path: &Path, id: &str, target: &str, value: &str) {
    std::fs::write(
        path,
        format!(
            "routes:\n  - id: \"{id}\"\n    from: \"{target}\"\n    steps:\n      - set_body:\n          value: \"{value}\"\n"
        ),
    )
    .expect("write route");
}

/// One minimal one-shot job document with `capture-reply`: sends to
/// `to` and loads its route from `route` (`routeFilesFromRoot`
/// spelling, anchored at the Camel.toml root).
fn write_resolution_job(path: &Path, to: &str, route: &str) {
    std::fs::write(
        path,
        format!(
            "execute:\n  mode: one-shot\n  timeout: 60s\n  capture-reply: true\n  send:\n    to: {to}\n    body: \"ping\"\nrouteFilesFromRoot:\n  - {route}\n"
        ),
    )
    .expect("write job doc");
}

/// A stem path with a separator (`daily/ingest`) must probe
/// `<root>/daily/ingest.job.yaml`: the report's `document` field ends
/// with the nested path and stderr stays empty.
#[test]
fn nested_stem_path_resolves_across_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"jobs\"]");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::create_dir_all(dir.path().join("jobs/daily")).expect("mkdir jobs/daily");
    write_resolution_route(
        &dir.path().join("routes/job-route.yaml"),
        "job-transform",
        "direct:transform",
        "job-done",
    );
    write_resolution_job(
        &dir.path().join("jobs/daily/ingest.job.yaml"),
        "direct:transform",
        "routes/job-route.yaml",
    );

    let (code, stdout, stderr) = run_job(dir.path(), "daily/ingest");
    assert_eq!(
        code, 0,
        "nested stem path must resolve;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.is_empty(), "stderr must be empty; got:\n{stderr}");
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    let document = report["document"].as_str().expect("document field");
    assert!(
        document.ends_with("jobs/daily/ingest.job.yaml"),
        "document field must name the nested document; got: {document}"
    );
}

/// The suffixed spelling of the same nested document probes verbatim
/// (`jobs/daily/ingest.job.yaml`) and completes.
#[test]
fn nested_document_path_resolves_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"jobs\"]");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::create_dir_all(dir.path().join("jobs/daily")).expect("mkdir jobs/daily");
    write_resolution_route(
        &dir.path().join("routes/job-route.yaml"),
        "job-transform",
        "direct:transform",
        "job-done",
    );
    write_resolution_job(
        &dir.path().join("jobs/daily/ingest.job.yaml"),
        "direct:transform",
        "routes/job-route.yaml",
    );

    let (code, stdout, stderr) = run_job(dir.path(), "daily/ingest.job.yaml");
    assert_eq!(
        code, 0,
        "suffixed nested path must probe verbatim;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
}

/// An explicit-class argument that exists relative to the CWD wins
/// before any root probing: the CWD copy runs, the same-named root
/// document never does.
#[test]
fn cwd_relative_existence_wins_over_root_probe() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"jobs\"]");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::create_dir_all(dir.path().join("local")).expect("mkdir local");
    std::fs::create_dir_all(dir.path().join("jobs/local")).expect("mkdir jobs/local");
    write_resolution_route(
        &dir.path().join("routes/cwd-route.yaml"),
        "cwd-transform",
        "direct:cwd-echo",
        "cwd-marker",
    );
    write_resolution_route(
        &dir.path().join("routes/root-route.yaml"),
        "root-transform",
        "direct:root-echo",
        "root-marker",
    );
    write_resolution_job(
        &dir.path().join("local/echo.job.yaml"),
        "direct:cwd-echo",
        "routes/cwd-route.yaml",
    );
    write_resolution_job(
        &dir.path().join("jobs/local/echo.job.yaml"),
        "direct:root-echo",
        "routes/root-route.yaml",
    );

    let (code, stdout, stderr) = run_job(dir.path(), "local/echo.job.yaml");
    assert_eq!(
        code, 0,
        "CWD-relative explicit path must win;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(
        report["reply"]["body"], "cwd-marker",
        "the CWD copy must run, not the root copy; report: {report}"
    );
}

/// A stem path matching nested documents in two roots is ambiguous:
/// exit 2 and stderr names EVERY matching path.
#[test]
fn nested_relative_path_collision_names_every_match() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"first\", \"second\"]");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::create_dir_all(dir.path().join("first/daily")).expect("mkdir first/daily");
    std::fs::create_dir_all(dir.path().join("second/daily")).expect("mkdir second/daily");
    write_resolution_route(
        &dir.path().join("routes/job-route.yaml"),
        "job-transform",
        "direct:transform",
        "job-done",
    );
    write_resolution_job(
        &dir.path().join("first/daily/ingest.job.yaml"),
        "direct:transform",
        "routes/job-route.yaml",
    );
    write_resolution_job(
        &dir.path().join("second/daily/ingest.job.yaml"),
        "direct:transform",
        "routes/job-route.yaml",
    );

    let (code, stdout, stderr) = run_job(dir.path(), "daily/ingest");
    assert_eq!(
        code, 2,
        "nested collision is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("ambiguous"),
        "stderr must name the ambiguity; got:\n{stderr}"
    );
    assert!(
        stderr.contains("first/daily/ingest.job.yaml")
            && stderr.contains("second/daily/ingest.job.yaml"),
        "stderr must name every matching path; got:\n{stderr}"
    );
}

/// A stem-path miss names every probed path, one per root.
#[test]
fn nested_relative_path_miss_names_probes() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"first\", \"second\"]");
    std::fs::create_dir_all(dir.path().join("first")).expect("mkdir first");
    std::fs::create_dir_all(dir.path().join("second")).expect("mkdir second");

    let (code, stdout, stderr) = run_job(dir.path(), "daily/missing");
    assert_eq!(
        code, 2,
        "nested miss is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("no job `daily/missing`"),
        "stderr must carry the miss error; got:\n{stderr}"
    );
    assert!(
        stderr.contains("first/daily/missing.job.yaml")
            && stderr.contains("second/daily/missing.job.yaml"),
        "stderr must name every probed path; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "no report on a miss; got:\n{stdout}"
    );
}

/// Bare names probe root level only: a nested document is invisible to
/// the bare-name probe (`jobs/ingest.job.yaml` is named, the nested
/// path never appears).
#[test]
fn bare_name_does_not_descend_into_subdirectories() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"jobs\"]");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::create_dir_all(dir.path().join("jobs/daily")).expect("mkdir jobs/daily");
    write_resolution_route(
        &dir.path().join("routes/job-route.yaml"),
        "job-transform",
        "direct:transform",
        "job-done",
    );
    write_resolution_job(
        &dir.path().join("jobs/daily/ingest.job.yaml"),
        "direct:transform",
        "routes/job-route.yaml",
    );

    let (code, stdout, stderr) = run_job(dir.path(), "ingest");
    assert_eq!(
        code, 2,
        "bare name must not resolve a nested document;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("jobs/ingest.job.yaml"),
        "stderr must name the root-level probe; got:\n{stderr}"
    );
    assert!(
        !stderr.contains("daily/ingest.job.yaml"),
        "bare name must never descend; got:\n{stderr}"
    );
}

/// A bare name never consults the CWD: a decoy file named exactly
/// `report` in the CWD is ignored; the root document runs.
#[test]
fn bare_name_ignores_cwd_entries() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"jobs\"]");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::create_dir_all(dir.path().join("jobs")).expect("mkdir jobs");
    write_resolution_route(
        &dir.path().join("routes/job-route.yaml"),
        "job-transform",
        "direct:transform",
        "report-done",
    );
    write_resolution_job(
        &dir.path().join("jobs/report.job.yaml"),
        "direct:transform",
        "routes/job-route.yaml",
    );
    std::fs::write(dir.path().join("report"), "decoy").expect("write decoy");

    let (code, stdout, stderr) = run_job(dir.path(), "report");
    assert_eq!(
        code, 0,
        "bare name must resolve from the root only;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(
        report["reply"]["body"], "report-done",
        "the root document must run; the CWD decoy is never parsed; report: {report}"
    );
}

/// Probes join as spelled: `../outside/ingest` under root `first`
/// names the verbatim joined probe `first/../outside/ingest.job.yaml`
/// — no normalization, no confinement.
#[test]
fn probe_is_joined_as_spelled_without_normalization() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"first\"]");
    std::fs::create_dir_all(dir.path().join("first")).expect("mkdir first");

    let (code, stdout, stderr) = run_job(dir.path(), "../outside/ingest");
    assert_eq!(
        code, 2,
        "unresolvable relative path is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("first/../outside/ingest.job.yaml"),
        "stderr must name the verbatim joined probe; got:\n{stderr}"
    );
}

/// Absolute arguments are used as-is: an existing document outside all
/// roots runs; a nonexistent absolute path fails with that path named
/// and never mentions root probing.
#[test]
fn absolute_argument_is_used_as_is_without_probing() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"jobs\"]");
    std::fs::create_dir_all(dir.path().join("jobs")).expect("mkdir jobs");

    let outside = tempfile::tempdir().expect("tempdir");
    write_resolution_route(
        &outside.path().join("job-route.yaml"),
        "outside-transform",
        "direct:outside-transform",
        "outside-done",
    );
    std::fs::write(
        outside.path().join("job.job.yaml"),
        "execute:\n  mode: one-shot\n  timeout: 60s\n  capture-reply: true\n  send:\n    to: direct:outside-transform\n    body: \"ping\"\nrouteFiles:\n  - job-route.yaml\n",
    )
    .expect("write job doc");
    let doc_arg = outside.path().join("job.job.yaml").display().to_string();

    let (code, stdout, stderr) = run_job(dir.path(), &doc_arg);
    assert_eq!(
        code, 0,
        "absolute path is used as-is;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(
        report["reply"]["body"], "outside-done",
        "the outside document must run; report: {report}"
    );

    let missing = dir.path().join("nope/missing.job.yaml");
    let missing_arg = missing.display().to_string();
    let (code, stdout, stderr) = run_job(dir.path(), &missing_arg);
    assert_eq!(
        code, 2,
        "nonexistent absolute path is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&missing_arg),
        "stderr must name the absolute path; got:\n{stderr}"
    );
    assert!(
        !stderr.contains("in any configured root"),
        "absolute arguments must never reach root probing; got:\n{stderr}"
    );
}

/// A trailing-separator directory argument (`local/`) is explicit-class
/// by its SPELLING and must be used as-is: the CWD directory fails loud
/// at read (exit 2, `Is a directory`), and the valid hidden document
/// `jobs/local/.job.yaml` is NEVER probed. Regression lock: the old
/// classification used `Path::components().count() > 1`, which
/// normalizes the trailing separator away and misclassified `local/` as
/// a bare name.
#[test]
fn trailing_separator_dir_arg_fails_loud_not_probed() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"jobs\"]");
    std::fs::create_dir_all(dir.path().join("local")).expect("mkdir local (CWD dir)");
    std::fs::create_dir_all(dir.path().join("jobs/local")).expect("mkdir jobs/local");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    write_resolution_route(
        &dir.path().join("routes/probe-route.yaml"),
        "probe-transform",
        "direct:probe",
        "probe-marker",
    );
    std::fs::write(
        dir.path().join("jobs/local/.job.yaml"),
        "execute:\n  mode: one-shot\n  timeout: 60s\n  capture-reply: true\n  send:\n    to: direct:probe\n    body: \"ping\"\nrouteFilesFromRoot:\n  - routes/probe-route.yaml\n",
    )
    .expect("write hidden probe doc");

    let (code, stdout, stderr) = run_job(dir.path(), "local/");
    assert_eq!(
        code, 2,
        "trailing-separator dir must fail loud at read;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let local_dir = std::fs::canonicalize(dir.path().join("local")).expect("local dir exists");
    assert!(
        stderr.contains(&format!("{}: Is a directory", local_dir.display())),
        "stderr must be the loud read failure for the directory; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "the probed hidden document must never run; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("probe-marker"),
        "the probe-marker route output must never appear; got:\n{stdout}"
    );
    assert!(
        !stderr.contains("in any configured root"),
        "explicit-class args must never reach root probing; got:\n{stderr}"
    );
}

/// A trailing `.` directory argument (`local/.`) behaves like the
/// trailing-separator spelling: explicit-class by spelling, used as-is,
/// loud read failure for the directory, no root probing, no hidden
/// document run.
#[test]
fn trailing_dot_dir_arg_fails_loud_not_probed() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config_with_jobs(dir.path(), "dirs = [\"jobs\"]");
    std::fs::create_dir_all(dir.path().join("local")).expect("mkdir local (CWD dir)");
    std::fs::create_dir_all(dir.path().join("jobs/local")).expect("mkdir jobs/local");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    write_resolution_route(
        &dir.path().join("routes/probe-route.yaml"),
        "probe-transform",
        "direct:probe",
        "probe-marker",
    );
    std::fs::write(
        dir.path().join("jobs/local/.job.yaml"),
        "execute:\n  mode: one-shot\n  timeout: 60s\n  capture-reply: true\n  send:\n    to: direct:probe\n    body: \"ping\"\nrouteFilesFromRoot:\n  - routes/probe-route.yaml\n",
    )
    .expect("write hidden probe doc");

    let (code, stdout, stderr) = run_job(dir.path(), "local/.");
    assert_eq!(
        code, 2,
        "trailing-dot dir must fail loud at read;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let local_dir = std::fs::canonicalize(dir.path().join("local")).expect("local dir exists");
    assert!(
        stderr.contains(&format!("{}: Is a directory", local_dir.display())),
        "stderr must be the loud read failure for the directory; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "the probed hidden document must never run; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("probe-marker"),
        "the probe-marker route output must never appear; got:\n{stdout}"
    );
    assert!(
        !stderr.contains("in any configured root"),
        "explicit-class args must never reach root probing; got:\n{stderr}"
    );
}

/// Listing is metadata-only: sentinel values that would fail route
/// interpolation or security compilation never reach a pipeline — the
/// description prints and the exit is 0.
#[test]
fn job_listing_does_not_boot_route_pipeline() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir_all(dir.path().join("jobs")).expect("mkdir jobs");
    std::fs::write(
        dir.path().join("jobs/sentinel.job.yaml"),
        "description: safe listing\nexecute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\n    body: \"${env:JOB_DISCOVERY_MUST_NOT_RUN}\"\nrouteFilesFromRoot:\n  - __invalid_listing_sentinel__.yaml\n",
    )
    .expect("write sentinel job doc");

    let (code, stdout, stderr) = run_job_args(dir.path(), &[]);
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("sentinel") && stdout.contains("safe listing"),
        "description listed; got:\n{stdout}"
    );
    assert!(
        !stderr.contains("JOB_DISCOVERY_MUST_NOT_RUN")
            && !stderr.contains("__invalid_listing_sentinel__"),
        "no interpolation or security sentinel errors; got:\n{stderr}"
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

// ── Declared `args:` end to end (jobargs Task 4.1) ─────────────────────

/// The field-matrix fixture: four declared arguments (`target`, `text`,
/// `header`, `wait`) referenced in the four send surfaces. The route
/// echoes the interpolated header and body into the reply body, so one
/// capture-reply assertion observes `to`, `body`, and `headers` at once;
/// a successful run with `timeout: "${arg:wait}"` proves the timeout
/// interpolated to a duration BEFORE the humantime check rejected it.
fn write_args_matrix_fixture(dir: &Path, mode: &str) {
    write_config(dir);
    std::fs::create_dir(dir.join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-args-matrix"
    from: "direct:in"
    steps:
      - transform: {simple: "${header.X-Env}:${body}"}
"#,
    )
    .expect("write route");
    let doc = format!(
        r#"args:
  target: {{default: "direct:in"}}
  text: {{default: "hello"}}
  header: {{default: "gold"}}
  wait: {{default: "30s"}}
execute:
  mode: {mode}
  timeout: "${{arg:wait}}"
  capture-reply: true
  send:
    to: "${{arg:target}}"
    body: "${{arg:text}}"
    headers:
      X-Env: "${{arg:header}}"
routeFiles:
  - routes/job-route.yaml
"#
    );
    std::fs::write(dir.join("job.job.yaml"), doc).expect("write job doc");
}

/// The four-field interpolation matrix in BOTH execution modes: one-shot
/// resolves embedded defaults for `to`, `body`, `headers`, and `timeout`;
/// batch resolves explicit `--arg` pairs over the same defaults. Exit 0
/// and a `Completed` report whose reply body carries the interpolated
/// header and body (`${header.X-Env}:${body}`).
#[test]
fn job_args_end_to_end_field_matrix() {
    // One-shot: defaults only, no `--arg` pairs.
    let dir = tempfile::tempdir().expect("tempdir");
    write_args_matrix_fixture(dir.path(), "one-shot");
    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(report["mode"], "one-shot", "report: {report}");
    assert_eq!(
        report["reply"]["body"], "gold:hello",
        "declared defaults must resolve in headers and body; report: {report}"
    );

    // Batch: explicit pairs win over the same defaults; the direct
    // target keeps the drain empty-queue immediate.
    let dir = tempfile::tempdir().expect("tempdir");
    write_args_matrix_fixture(dir.path(), "batch");
    let (code, stdout, stderr) = run_job_args(
        dir.path(),
        &[
            "job.job.yaml",
            "--arg",
            "text=hi",
            "--arg",
            "header=silver",
            "--arg",
            "wait=45s",
        ],
    );
    assert_eq!(
        code, 0,
        "expected exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(report["mode"], "batch", "report: {report}");
    assert_eq!(
        report["reply"]["body"], "silver:hi",
        "explicit pairs must win over defaults in batch mode; report: {report}"
    );
}

/// Declared-argument validation happens before boot and exits 2 naming
/// the offending argument: an `--arg` naming an undeclared argument, and
/// an omitted required argument with no default. Early exit-2 classes
/// are stderr-only — no JSON report reaches stdout.
#[test]
fn job_args_validation_exit_two() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-args-validation"
    from: "direct:transform"
    steps:
      - set_body:
          value: "unreached"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"args:
  name: {required: true}
execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    // Unknown name: exit 2 before boot, stderr names `tier`.
    let (code, stdout, stderr) = run_job_args(dir.path(), &["job.job.yaml", "--arg", "tier=gold"]);
    assert_eq!(
        code, 2,
        "unknown declared argument is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("unknown argument `tier`"),
        "stderr must name the undeclared argument; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "early exit-2 class is stderr-only; got:\n{stdout}"
    );

    // Missing required: exit 2 before boot, stderr names `name`.
    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
    assert_eq!(
        code, 2,
        "missing required argument is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("missing required argument `name`"),
        "stderr must name the required argument; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "early exit-2 class is stderr-only; got:\n{stdout}"
    );
}

/// An `${arg:ghost}` reference without a `ghost` declaration fails
/// interpolation at the same stage and scanner as `${env:}`: exit 2
/// before boot with a diagnostic naming `ghost`. The `arg:` namespace is
/// dispatched before lookup, so no environment fallthrough can rescue
/// the run (the fixture sets no such variable anyway).
#[test]
fn job_args_interpolation_failure_exit_two() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-args-interp"
    from: "direct:transform"
    steps:
      - set_body:
          value: "unreached"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"args:
  known: {default: "x"}
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
    body: "ghost says ${arg:ghost}"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
    assert_eq!(
        code, 2,
        "unresolved ${{arg:}} token is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("unresolved argument `ghost`"),
        "stderr must name the unresolved argument; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "early exit-2 class is stderr-only; got:\n{stdout}"
    );
}

/// Run `camel job <args...>` in `dir` with explicit environment overrides
/// (`envs`) and removals (`env_remove`), returning
/// `(exit_code, stdout, stderr)`. Unlike [`common::run_binary`], this can
/// scrub a variable from the child's inherited environment, so a
/// `${env:}` probe distinguishes "set" from "unset" without depending on
/// the ambient test-runner environment.
fn run_job_args_with_env(
    dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    env_remove: &[&str],
) -> (i32, String, String) {
    let mut full: Vec<&str> = vec!["job"];
    full.extend(args.iter().copied());
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_camel"));
    command
        .args(&full)
        .envs(envs.iter().copied())
        .current_dir(dir)
        .stdin(std::process::Stdio::null());
    for key in env_remove {
        command.env_remove(key);
    }
    let output = command.output().expect("spawn `camel job`");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// `${env:VAR}` inside a DECLARED document's `execute.send` surface
/// resolves through the ambient environment at the same interpolation
/// stage as `${arg:}`. With the variable set, the substituted value
/// reaches the captured reply body; with it scrubbed from the child
/// explicitly, the run is the same early exit-2 class that names the
/// unresolved token. Regression lock for the holistic-review finding I1.
#[test]
fn job_env_in_declared_send_fields() {
    const PROBE: &str = "JOBARGS_ENV_PROBE";
    const VALUE: &str = "env-probe-ok-7f3a";

    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-env-send"
    from: "direct:env-send"
    steps:
      - transform: {simple: "${body}"}
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        format!(
            r#"args:
  probe: {{default: "declared"}}
execute:
  mode: one-shot
  timeout: 30s
  capture-reply: true
  send:
    to: direct:env-send
    body: "${{env:{PROBE}}}"
routeFiles:
  - routes/job-route.yaml
"#
        ),
    )
    .expect("write job doc");

    // Set: the ambient value substitutes into the send body and reaches
    // the captured reply.
    let (code, stdout, stderr) =
        run_job_args_with_env(dir.path(), &["job.job.yaml"], &[(PROBE, VALUE)], &[]);
    assert_eq!(
        code, 0,
        "resolved ${{env:}} token must run to completion;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(
        report["reply"]["body"], VALUE,
        "capture-reply body must carry the substituted ${{env:}} value; report: {report}"
    );

    // Unset: scrub the variable from the child explicitly — same early
    // exit-2 class as the `${arg:}` scanner failure, stderr names it.
    let (code, stdout, stderr) =
        run_job_args_with_env(dir.path(), &["job.job.yaml"], &[], &[PROBE]);
    assert_eq!(
        code, 2,
        "unresolved ${{env:}} token is exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("unresolved argument `JOBARGS_ENV_PROBE`"),
        "stderr must name the unresolved environment token; got:\n{stderr}"
    );
    assert!(
        stdout.trim().is_empty(),
        "early exit-2 class is stderr-only; got:\n{stdout}"
    );
}
