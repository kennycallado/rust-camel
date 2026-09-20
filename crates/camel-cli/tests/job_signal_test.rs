//! End-to-end integration tests for the `camel job` signal contract
//! (jobsignals Task 2.1).
//!
//! Subprocess regression coverage for the Task 1.1 contract: the first
//! SIGINT/SIGTERM during boot, a held send, or a batch drain yields an
//! `Interrupted` report and exit 2; a second signal during the
//! interruption teardown force-exits 1. Both signal streams are armed
//! at the first lines of `run_job` (BEFORE config load), so a signal
//! landing mid-boot is buffered by the runtime and consumed by the
//! send/drain race instead of hitting the default disposition.
//!
//! The harness mirrors `run_signal_test.rs`: spawn the real `camel`
//! binary with piped stdout/stderr (`common::spawn_camel_job*`),
//! drain both pipes (`common::spawn_drained`), poll the captured
//! buffers for a flushed marker (`common::wait_for_marker`), then
//! deliver signals with `kill` (`common::send_signal`). The two
//! synchronization markers are the pre-config-load
//! `signal streams armed` stderr line (boot-buffer placement) and the
//! mid-boot CWD-trust INFO note (post-boot placement: discovery, route
//! start, and the send race all follow it). Reports are asserted
//! through `--report=<file>` so stdout stays clean. The kill-on-drop
//! child guard plus a bounded exit wait keep every failure path from
//! leaking the process; a shell signal death (130/143) surfaces here
//! as the `-1` sentinel from `wait_exit_code_bounded`.

mod common;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use common::{
    send_signal, spawn_camel_job, spawn_camel_job_with_args, spawn_drained, wait_exit_code_bounded,
    wait_for_marker,
};

/// Flushed stderr marker from the first lines of `run_job`, BEFORE
/// config load: proves the signal streams are registered while boot is
/// still in flight (the boot-buffer window of the armed marker).
const ARMED_MARKER: &str = "camel job: signal streams armed";

/// Mid-boot marker: the CWD-trust INFO note fires after context configure
/// and before discovery, route start, and the send race — sending here
/// lands the signal against a booted, in-flight job.
const TRUST_MARKER: &str = "trusts the current working directory";

/// Observation and exit ceiling, 30 s per the `common/mod.rs`
/// convention: the ~466 MB binary boots slowly under a saturated
/// whole-workspace `cargo test`, and the interrupted teardown is
/// bounded (one-shot 5 s floor; batch remaining budget), so the happy
/// path still returns in seconds.
const WAIT: Duration = Duration::from_secs(30);

/// Ceiling for the second-signal force exit: the force-exit guard is
/// spawned the moment the first signal is consumed and its select is
/// ready on the already-buffered second signal, so the exit is
/// scheduler-latency fast. 15 s only buys headroom under load.
const FORCE_EXIT_BOUND: Duration = Duration::from_secs(15);

/// Write the standard job-fixture config. `log_level = "INFO"` (not
/// the `"off"` of `job_one_shot_test.rs`): the mid-boot CWD-trust INFO
/// note the tests synchronize on must be visible at `log_level = "INFO"`.
fn write_config(dir: &Path) {
    std::fs::write(
        dir.join("Camel.toml"),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "INFO"
watch = false
"#,
    )
    .expect("write Camel.toml");
}

/// Fixture whose one-shot send is HELD: the runner forces
/// `waitForTaskToComplete=Always` on seda targets, so the send to
/// `seda:work` blocks until the worker's 30 s `delay` pipeline
/// completes — far past the signal and the 60 s overall timeout.
fn write_held_one_shot_fixture(dir: &Path) {
    std::fs::create_dir(dir.join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.join("routes/held-route.yaml"),
        r#"routes:
  - id: "job-held"
    from: "seda:work"
    steps:
      - delay: 30000
      - set_body:
          value: "never-reached"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.join("job.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  send:
    to: seda:work
routeFiles:
  - routes/held-route.yaml
"#,
    )
    .expect("write job doc");
}

/// Fixture whose batch drain stays ACTIVE: the trigger send to
/// `direct:fan` returns immediately (fire-and-forget seda hop), then
/// the worker claims the envelope for its whole 30 s pipeline — the
/// seda forwarder holds its `DepthGuard` claim until forwarding
/// completes, so the queue depth stays >= 1 and the drain's
/// zero-depth streak can never complete before the signal.
fn write_batch_drain_fixture(dir: &Path) {
    std::fs::create_dir(dir.join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.join("routes/job-route.yaml"),
        r#"routes:
  - id: "fan"
    from: "direct:fan"
    steps:
      - to: "seda:w1"
  - id: "w1"
    from: "seda:w1"
    steps:
      - delay: 30000
      - set_body:
          value: "never-reached"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.join("job.job.yaml"),
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
}

/// Read and parse the `--report` JSON file (written before the exit
/// code is returned, so it exists once the child has exited).
fn read_report(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("--report file {} must exist: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("--report file must hold one JSON report: {e}; got: {text}"))
}

/// The first SIGTERM against a held one-shot send: the biased
/// send-vs-signal race resolves to the signal, the in-flight send is
/// cancelled, the bounded teardown runs, and the `--report` JSON
/// carries `Interrupted` with an error mentioning the signal — exit 2,
/// never the 143 of a default-disposition kill.
#[test]
fn job_signal_interrupts_one_shot() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    write_held_one_shot_fixture(dir.path());
    let report_path = dir.path().join("report.json");
    let report_arg = format!("--report={}", report_path.display());

    let mut child = spawn_camel_job_with_args(
        dir.path(),
        Path::new("job.job.yaml"),
        &[report_arg.as_str()],
    );
    let drained = spawn_drained(&mut child);

    // Post-boot marker: boot finished and the Always-forced seda send
    // is held by the 30 s worker, so the signal lands on the send
    // race deterministically.
    let reached = wait_for_marker(&mut child, &drained.markers(), TRUST_MARKER, WAIT);
    assert!(
        reached,
        "camel job never booted to the trust warning;\n{}",
        drained.captured()
    );

    send_signal(&child, "-TERM");

    let code = wait_exit_code_bounded(&mut child, WAIT);
    let output = drained.finish();
    assert_eq!(
        code, 2,
        "first SIGTERM during a held send must report Interrupted and exit 2; \
         -1 means the process died by signal (a shell would report 143);\n{output}"
    );
    let report = read_report(&report_path);
    assert_eq!(report["outcome"], "Interrupted", "report: {report}");
    assert!(
        report["error"]
            .as_str()
            .is_some_and(|e| e.contains("signal")),
        "the Interrupted report error must mention the signal; report: {report}"
    );
}

/// The first SIGINT against an active batch drain: the fire-and-forget
/// fan-out returns instantly while the claimed envelope keeps the seda
/// queue non-empty, so the signal interrupts the drain — `Interrupted`
/// report (mode `batch`), exit 2, not the 130 of a default kill.
#[test]
fn job_signal_interrupts_batch_drain() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    write_batch_drain_fixture(dir.path());
    let report_path = dir.path().join("report.json");
    let report_arg = format!("--report={}", report_path.display());

    let mut child = spawn_camel_job_with_args(
        dir.path(),
        Path::new("job.job.yaml"),
        &[report_arg.as_str()],
    );
    let drained = spawn_drained(&mut child);

    // The trust warning precedes the send and drain; the signal sent
    // here is buffered by the armed streams and consumed by whichever
    // of the two races is (or becomes) active — either way the verdict
    // is Interrupted, and the 30 s worker keeps the drain from ever
    // completing first.
    let reached = wait_for_marker(&mut child, &drained.markers(), TRUST_MARKER, WAIT);
    assert!(
        reached,
        "camel job never booted to the trust warning;\n{}",
        drained.captured()
    );

    send_signal(&child, "-INT");

    let code = wait_exit_code_bounded(&mut child, WAIT);
    let output = drained.finish();
    assert_eq!(
        code, 2,
        "first SIGINT during a batch drain must report Interrupted and exit 2; \
         -1 means the process died by signal (a shell would report 130);\n{output}"
    );
    let report = read_report(&report_path);
    assert_eq!(report["outcome"], "Interrupted", "report: {report}");
    assert_eq!(report["mode"], "batch", "report: {report}");
}

/// The INT-then-TERM pair (both signal streams, un-coalesceable across
/// types): the send race consumes the first, the force-exit guard —
/// spawned at interruption, before teardown — sees the still-buffered
/// second on its first poll and force-exits 1 within a bounded window.
/// No completed report is required on this path: the guard exits before
/// the report write.
#[test]
fn job_signal_second_signal_forces_exit() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    write_held_one_shot_fixture(dir.path());

    let mut child = spawn_camel_job(dir.path(), Path::new("job.job.yaml"));
    let drained = spawn_drained(&mut child);

    let reached = wait_for_marker(&mut child, &drained.markers(), TRUST_MARKER, WAIT);
    assert!(
        reached,
        "camel job never booted to the trust warning;\n{}",
        drained.captured()
    );

    // One `sh -c` sends both signals so the pair lands well before the
    // teardown starts (`kill` is a shell builtin; two separate spawn(2)
    // calls would leave a multi-ms exec gap that can push the second
    // signal past the force-exit arm under load). Same rationale as
    // `run_signal_test.rs`.
    let pair_started = Instant::now();
    let pair = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "kill -INT {pid}; kill -TERM {pid}",
            pid = child.id()
        ))
        .status()
        .expect("failed to spawn signal pair");
    assert!(pair.success(), "signal pair returned non-zero: {pair:?}");

    let code = wait_exit_code_bounded(&mut child, WAIT);
    let elapsed = pair_started.elapsed();
    let output = drained.finish();
    assert_eq!(
        code, 1,
        "the second stop signal must force-exit 1; exit 0/2 means the \
         force-exit arm never fired, -1 means a default-disposition kill;\n{output}"
    );
    assert!(
        elapsed < FORCE_EXIT_BOUND,
        "the force exit must stay bounded; took {elapsed:?};\n{output}"
    );
    assert!(
        output.contains("forcing exit"),
        "expected the `second stop signal — forcing exit` WARN;\n{output}"
    );
}

/// A single SIGTERM sent right after the flushed `signal streams
/// armed` marker — before config load and the trust warning — is
/// buffered by the entry-registered stream and consumed by the send
/// race once boot completes: exit 2 with an `Interrupted` report, not
/// the 143 of a default-disposition kill mid-boot. The placement
/// (armed marker, 25 ms poll reaction, against a config-load-plus-
/// context boot stretch) is a biased race toward the boot window; the
/// outcome assertions are the deterministic regression net — a process
/// without armed streams dies by the signal and cannot produce either.
#[test]
fn job_signal_during_boot_is_buffered() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    write_held_one_shot_fixture(dir.path());
    let report_path = dir.path().join("report.json");
    let report_arg = format!("--report={}", report_path.display());

    let mut child = spawn_camel_job_with_args(
        dir.path(),
        Path::new("job.job.yaml"),
        &[report_arg.as_str()],
    );
    let drained = spawn_drained(&mut child);

    // The armed marker is flushed to stderr at the very first lines of
    // run_job — BEFORE config load — so a signal sent on it lands
    // while boot is still in flight.
    let armed = wait_for_marker(&mut child, &drained.markers(), ARMED_MARKER, WAIT);
    assert!(
        armed,
        "camel job never flushed the signal streams armed marker;\n{}",
        drained.captured()
    );

    send_signal(&child, "-TERM");

    let code = wait_exit_code_bounded(&mut child, WAIT);
    let output = drained.finish();
    assert_eq!(
        code, 2,
        "a SIGTERM during boot must be buffered and end as Interrupted \
         exit 2; -1 means the process died by signal (a shell would \
         report 143);\n{output}"
    );
    let report = read_report(&report_path);
    assert_eq!(report["outcome"], "Interrupted", "report: {report}");
}

// ── Bounded metadata listing (jobdiscovery Task 2.1) ───────────────────
//
// The listing-walk tests live in this binary per the jobdiscovery task
// map (`job_listing_stops_at_` filter): they reuse `common::run_binary`
// for run-to-completion output and share nothing with the signal
// stretches above — listing never installs the signal streams.

/// Write the fixture config with `[jobs].dirs`, `log_level = "off"` so
/// stdout carries ONLY the listing rows (the general tracing layer
/// writes to stdout).
fn write_listing_config(dir: &Path, jobs_table: &str) {
    std::fs::write(
        dir.join("Camel.toml"),
        format!(
            "[default]\nroutes = [\"routes/*.yaml\"]\nlog_level = \"off\"\nwatch = false\n\n[default.jobs]\n{jobs_table}\n"
        ),
    )
    .expect("write Camel.toml");
}

/// Write one metadata-only job document (listing never parses the
/// route source, so no route file is needed).
fn write_listing_job(path: &Path, description: &str) {
    std::fs::write(
        path,
        format!(
            "description: {description}\nexecute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:transform\n"
        ),
    )
    .expect("write job doc");
}

/// Run no-argument `camel job` in `dir` to completion and return
/// `(exit_code, stdout, stderr)`.
fn run_job_listing(dir: &Path) -> (i32, String, String) {
    common::run_binary(dir, Path::new(env!("CARGO_BIN_EXE_camel")), &["job"], &[])
}

/// The listing recurses: a nested `.job.yaml` lists under its
/// configured-root-relative path while a sibling `.test.yaml` document
/// is silently skipped.
#[test]
fn job_listing_recurses_and_skips_test_documents() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_listing_config(dir.path(), "dirs = [\"jobs\"]");
    std::fs::create_dir_all(dir.path().join("jobs/domain")).expect("mkdir domain");
    write_listing_job(
        &dir.path().join("jobs/domain/report.job.yaml"),
        "nested job",
    );
    write_listing_job(
        &dir.path().join("jobs/domain/skip.test.yaml"),
        "must not list",
    );

    let (code, stdout, stderr) = run_job_listing(dir.path());
    assert_eq!(
        code, 0,
        "listing exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l == "domain/report.job.yaml: report — nested job"),
        "exact nested line; got:\n{stdout}"
    );
    assert!(
        !stdout.contains(".test.yaml"),
        "test document silently skipped; got:\n{stdout}"
    );
}

/// Depth cap: a job at depth 8 (eight nested directories below the
/// root) is listed, a depth-9 job is excluded, exactly ONE warning
/// names the root with the exact truncation phrase, and the SECOND
/// root still scans.
#[test]
fn job_listing_stops_at_depth_eight() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_listing_config(dir.path(), "dirs = [\"first\", \"second\"]");
    let mut deep = dir.path().join("first");
    for i in 1..=8 {
        deep = deep.join(format!("d{i}"));
    }
    std::fs::create_dir_all(&deep).expect("mkdir depth-8 chain");
    std::fs::create_dir_all(deep.join("d9")).expect("mkdir depth-9 dir");
    write_listing_job(&deep.join("eight.job.yaml"), "depth eight");
    write_listing_job(&deep.join("d9/nine.job.yaml"), "depth nine");
    std::fs::create_dir_all(dir.path().join("second")).expect("mkdir second");
    write_listing_job(&dir.path().join("second/second.job.yaml"), "second job");

    let (code, stdout, stderr) = run_job_listing(dir.path());
    assert_eq!(
        code, 0,
        "truncated listing still exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("d1/d2/d3/d4/d5/d6/d7/d8/eight.job.yaml: eight — depth eight"),
        "depth-8 job listed; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("nine"),
        "depth-9 job excluded; got:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|l| l == "second — second job"),
        "second root continues after the truncated first; got:\n{stdout}"
    );
    assert_eq!(
        stderr.matches("listing truncated").count(),
        1,
        "exactly one truncation warning; got:\n{stderr}"
    );
    assert!(
        stderr.contains("listing truncated at 8; narrow [jobs].dirs") && stderr.contains("first"),
        "warning names the root and the cap; got:\n{stderr}"
    );
}

/// File cap: 512 encountered files per root — after 511 `.txt` files
/// the 512th file (`511.job.yaml`) is the last processed entry, the
/// 513th (`512.job.yaml`) is excluded with exactly ONE warning naming
/// the root, and the second root still scans.
#[test]
fn job_listing_stops_at_512_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_listing_config(dir.path(), "dirs = [\"first\", \"second\"]");
    let first = dir.path().join("first");
    std::fs::create_dir_all(&first).expect("mkdir first");
    for i in 0..=510 {
        std::fs::write(first.join(format!("{i:03}.txt")), "filler").expect("write filler");
    }
    write_listing_job(&first.join("511.job.yaml"), "file cap job");
    write_listing_job(&first.join("512.job.yaml"), "beyond cap job");
    std::fs::create_dir_all(dir.path().join("second")).expect("mkdir second");
    write_listing_job(&dir.path().join("second/second.job.yaml"), "second job");

    let (code, stdout, stderr) = run_job_listing(dir.path());
    assert_eq!(
        code, 0,
        "truncated listing still exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.lines().any(|l| l == "511 — file cap job"),
        "the 512th encountered file is listed; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("beyond cap"),
        "the 513th file is excluded; got:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|l| l == "second — second job"),
        "second root continues after the truncated first; got:\n{stdout}"
    );
    assert_eq!(
        stderr.matches("listing truncated").count(),
        1,
        "exactly one truncation warning; got:\n{stderr}"
    );
    assert!(
        stderr.contains("listing truncated at 512; narrow [jobs].dirs") && stderr.contains("first"),
        "warning names the root and the cap; got:\n{stderr}"
    );
}

/// A directory holding more entries than any historical per-directory
/// buffer bound (1030 subdirectories) is still walked whole: with three
/// files — far under the 512-file cap — every job is listed in lexical
/// order and NO truncation warning is emitted. Only depth 8 and 512
/// encountered files ever truncate a scan.
#[test]
fn job_listing_walks_directory_with_many_subdirs() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_listing_config(dir.path(), "dirs = [\"first\"]");
    let first = dir.path().join("first");
    std::fs::create_dir_all(&first).expect("mkdir first");
    for i in 0..1030 {
        std::fs::create_dir(first.join(format!("d{i:04}"))).expect("mkdir wide dir");
    }
    write_listing_job(&first.join("d0000/early.job.yaml"), "early job");
    write_listing_job(&first.join("d1029/late.job.yaml"), "late job");
    write_listing_job(&first.join("top.job.yaml"), "top job");

    let (code, stdout, stderr) = run_job_listing(dir.path());
    assert_eq!(
        code, 0,
        "wide directory still exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let rows: Vec<&str> = stdout.lines().filter(|l| l.contains(" — ")).collect();
    assert_eq!(
        rows,
        vec![
            "d0000/early.job.yaml: early — early job",
            "d1029/late.job.yaml: late — late job",
            "top — top job",
        ],
        "every job of the wide directory is listed in lexical order; got:\n{stdout}"
    );
    assert!(
        !stderr.contains("listing truncated"),
        "a wide directory never truncates; got:\n{stderr}"
    );
}
