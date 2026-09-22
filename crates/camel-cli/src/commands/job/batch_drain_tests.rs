//! Drainclaim-era batch-drain tests, split out of `tests.rs` for
//! file-size hygiene. Behavior and test names are unchanged.

use super::*;

/// A barrier-parked self-feeding route cannot false-complete: the
/// route `seda:a -> delay 3000 -> seda:a` cycles one exchange forever,
/// so the context-global in-flight counter never reads zero and the
/// drain must run to the overall deadline. Runs IN-PROCESS through
/// [`super::run_job`] (the entry the clap handler dispatches to) under
/// `start_paused = true`: the route delay and the drain naps are
/// virtual tokio timers that auto-advance, so the run is deterministic
/// — several drain polls occur while the cycled exchange is parked on
/// the virtual delay — and the verdict is `Timeout` (exit 2), never
/// `Completed`.
#[tokio::test(start_paused = true)]
async fn batch_self_feed_cannot_false_complete() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes/job-route.yaml"),
        r#"routes:
  - id: "self-feed"
    from: "seda:a"
    steps:
      - delay: 3000
      - to: "seda:a"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: batch
  timeout: 6s
  send:
    to: "seda:a"
    body: "m"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");
    let report_path = dir.path().join("report.json");
    // Absolute paths throughout: the in-process entry shares this
    // process's CWD with every other lib test, so no chdir — the
    // document and config are passed absolute and routeFiles resolve
    // against the document's directory.
    let args = super::JobArgs {
        document: Some(dir.path().join("job.job.yaml")),
        help: false,
        report: Some(report_path.clone()),
        config: dir.path().join("Camel.toml").display().to_string(),
        args: Vec::new(),
        dynamic: Vec::new(),
    };

    let code = super::run_job(&args).await;

    assert_eq!(
        code, 2,
        "a forever-cycling exchange must hit the overall deadline (Timeout exit 2)"
    );
    let json = read_report(&report_path);
    assert_eq!(json["mode"], "batch", "report: {json}");
    assert_eq!(
        json["outcome"], "Timeout",
        "a parked cycled exchange must never let the drain report Completed; report: {json}"
    );
}

/// Settled batch work completes: the trigger fans out to two seda
/// workers whose short delays keep exchanges in flight when the
/// trigger send returns; the drain holds until every worker has
/// recorded to its sink, then reports `Completed` on the first zero
/// read — no fixed quiescence window (the `batch_typed_arg` fixture
/// shape).
#[test]
fn batch_settled_work_completes() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    let base = dir.path().display().to_string();
    let routes = format!(
        r#"routes:
  - id: "fan"
    from: "direct:fan"
    steps:
      - to: "seda:w1"
      - to: "seda:w2"
  - id: "w1"
    from: "seda:w1"
    steps:
      - delay: 200
      - to: "file:{base}?fileName=w1.txt"
  - id: "w2"
    from: "seda:w2"
    steps:
      - delay: 300
      - to: "file:{base}?fileName=w2.txt"
"#,
    );
    std::fs::write(dir.path().join("routes/job-route.yaml"), routes).expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: batch
  timeout: 60s
  send:
    to: "direct:fan"
    body: "m"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    let (code, stdout, stderr) = run_camel_job(dir.path(), &["job.job.yaml"]);
    assert_eq!(
        code, 0,
        "settled batch work must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["mode"], "batch", "report: {report}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    // Exit happened only after the in-flight workers settled: both
    // sinks carry their record.
    assert!(
        read_file_eventually(dir.path(), "w1.txt").contains('m'),
        "w1 must record before exit"
    );
    assert!(
        read_file_eventually(dir.path(), "w2.txt").contains('m'),
        "w2 must record before exit"
    );
}

/// A batch job's declared defaults resolve before the trigger send:
/// `tier: {default: gold}` substitutes into the send body with no
/// `--arg tier` pair, the worker records `tier-gold` to its sink, and
/// the batch drains normally (the `batch_typed_arg` fixture shape).
#[test]
fn batch_applies_declared_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    std::fs::create_dir(dir.path().join("routes")).expect("mkdir routes");
    let base = dir.path().display().to_string();
    let routes = format!(
        r#"routes:
  - id: "fan"
    from: "direct:fan"
    steps:
      - to: "seda:w1"
  - id: "w1"
    from: "seda:w1"
    steps:
      - to: "file:{base}?fileName=tier.txt"
"#,
    );
    std::fs::write(dir.path().join("routes/job-route.yaml"), routes).expect("write route");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"args:
  tier:
    default: gold
execute:
  mode: batch
  timeout: 60s
  send:
    to: "direct:fan"
    body: "tier-${arg:tier}"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");

    // No `--arg tier`: the declared default must fill the send body.
    let (code, stdout, stderr) = run_camel_job(dir.path(), &["job.job.yaml"]);
    assert_eq!(
        code, 0,
        "batch run with declared default must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is the JSON report; got:\n{stdout}");
    assert_eq!(report["mode"], "batch", "report: {report}");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    let recorded = read_file_eventually(dir.path(), "tier.txt");
    assert!(
        recorded.contains("tier-gold"),
        "the worker must record the DEFAULT value (gold); got: {recorded}"
    );
}
