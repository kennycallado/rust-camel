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

use common::drain_to_buffer;

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

/// Load-time validation: `mode: batch` is parsed then rejected with the
/// reserved-mode error and exit 2 (no boot).
#[test]
fn batch_mode_is_rejected_at_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    std::fs::write(
        dir.path().join("job.job.yaml"),
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

    let (code, stdout, stderr) = run_job(dir.path(), "job.job.yaml");
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

// ── Bare-name resolution + optional document (job-ux-reshape) ──────────

/// Run `camel job <args...>` in `dir` with arbitrary args (no Path
/// coercion) and return `(exit_code, stdout, stderr)`.
fn run_job_args(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("job")
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .spawn()
        .expect("spawn camel job");
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
