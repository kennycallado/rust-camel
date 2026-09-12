//! Integration tests for compiled-artifact runtime (openspec change
//! `cli-compile`, Tasks 2.2 and 2.3). The suite compiles each distinct
//! document ONCE with the real `camel compile` into a shared immutable
//! fixture — the compile step copies the full ~283 MB `camel` binary into
//! every artifact, so per-test compiles would write gigabytes under
//! parallel execution and exhaust the disk (ENOSPC). Every test then
//! deploys the fixture artifact into its own source-free directory and
//! runs it through `run_embedded_document` — the same entry the binary
//! self-detect path (Task 2.3) calls.
//!
//! The artifact runtime executes in a harness CHILD of this test binary:
//! the parent re-spawns `current_exe()` with `--exact <test>` and the
//! artifact argv after `--`, plus [`CHILD_ENV`] naming the artifact. The
//! child branch decodes the trailer, parses `ArtifactArgs`, runs the
//! embedded document, and exits with its code — exercising the library
//! seam end to end (boot, signals, report) without a self-detecting main.
//!
//! The Task 2.3 tests below exercise the REAL self-detect entry instead:
//! they spawn the artifact binary itself (a trailer-bearing copy of
//! `camel`), whose `main` probes the trailer before Clap and consumes
//! the artifact argv surface on its own.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use camel_cli::compile::runtime::{ArtifactArgs, EmbeddedRequest};
use camel_cli::compile::trailer;

use common::{KillOnDrop, drain_to_buffer, send_signal};

/// Env var that marks a harness child: its value is the artifact path.
const CHILD_ENV: &str = "CAMEL_COMPILED_ARTIFACT_CHILD";

/// A minimal timer→log route document (long-running; signal shutdown).
/// `period` is plain milliseconds (the timer component parses a number).
const ROUTE_DOC: &str = "\
routes:
  - id: demo
    from: timer:tick?period=300
    steps:
      - to: log:demo
";

/// A one-shot job document with inline routes (self-contained).
const JOB_DOC: &str = "\
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: ping
routes:
  - id: job-transform
    from: direct:transform
    steps:
      - set_body:
          value: job-done
";

/// A one-shot job document whose route pipeline fails (send to a
/// `direct:` endpoint with no consumer).
const FAILING_JOB_DOC: &str = "\
execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:boom
routes:
  - id: job-fail
    from: direct:boom
    steps:
      - to: direct:missing-consumer
";

/// A one-shot job document whose route resolves `${env:NAME}` from the
/// deployment environment at run time. The fixture compiles it WITH a
/// compile-time value present: that value must never enter the artifact.
const ENV_DOC: &str = "\
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: ping
routes:
  - id: job-transform
    from: direct:transform
    steps:
      - set_body:
          value: ${env:DEPLOY_GREETING}
";

/// Compile `doc` into `artifact` inside `dir` with a clean environment.
fn compile(dir: &Path, doc: &str, artifact: &str, envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear()
        .envs(envs.iter().copied())
        .current_dir(dir)
        .args(["compile", doc, "-o", artifact]);
    cmd.output().expect("spawn `camel compile`")
}

/// Distinct documents compiled once per test process. `camel compile`
/// copies the full ~283 MB `camel` binary into every artifact, so
/// compiling per test would write gigabytes under parallel execution and
/// exhaust the disk (ENOSPC). The fixture compiles each document exactly
/// once — serialized by the `OnceLock` — and tests share the immutable
/// artifacts; only mutation tests copy (see [`deploy_artifact`]).
///
/// The artifacts live in a single cache directory keyed by this test
/// process (`camel-compiled-fixture-<pid>` under the OS temp dir, the
/// repo's `camel-test-*` convention). A detached reaper child removes
/// that directory once this process dies — normal exit or crash — and
/// the next run sweeps any leftover, so repeated runs never accumulate
/// the ~1.13 GB of compiled artifacts.
struct Fixture {
    /// `ROUTE_DOC` artifact (timer→log route).
    route: PathBuf,
    /// `JOB_DOC` artifact (one-shot job, happy path).
    job: PathBuf,
    /// `FAILING_JOB_DOC` artifact (one-shot job, failing pipeline).
    failing_job: PathBuf,
    /// `ENV_DOC` artifact, compiled with a compile-time env value.
    env: PathBuf,
}

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

/// The single cache directory for this test process's compiled fixture
/// (repo convention: `camel-test-*` under the OS temp dir, keyed by the
/// current test process).
fn fixture_dir() -> PathBuf {
    std::env::temp_dir().join(format!("camel-compiled-fixture-{}", std::process::id()))
}

/// Spawn a detached reaper that removes `dir` once this test process
/// dies — normal exit or crash. `kill -0` probes the parent; when it
/// fails the parent is gone, so the reaper deletes the fixture. The
/// reaper is reparented to init and reaped there; it never blocks the
/// test.
fn spawn_reaper(dir: &Path) {
    let dir = dir.to_string_lossy().into_owned();
    let pid = std::process::id().to_string();
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "while kill -0 {pid} 2>/dev/null; do sleep 1; done; rm -rf -- '{dir}'"
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Remove fixture directories left by previous test runs whose process
/// is no longer alive (crashed runs, or reapers that have not fired
/// yet). Concurrent runs keep their own PID-keyed directory.
fn sweep_stale_fixtures() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(pid_str) = name.strip_prefix("camel-compiled-fixture-") else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let alive = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !alive {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Compile every distinct document once, into the process-keyed fixture
/// directory. A stale directory from a previous run with the same PID
/// (PID reuse after a crash) is removed first.
fn fixture() -> &'static Fixture {
    FIXTURE.get_or_init(|| {
        sweep_stale_fixtures();
        let dir = fixture_dir();
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("remove stale fixture dir");
        }
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        spawn_reaper(&dir);
        let compile_one =
            |doc_name: &str, doc: &str, artifact: &str, envs: &[(&str, &str)]| -> PathBuf {
                std::fs::write(dir.join(doc_name), doc).expect("write document");
                let output = compile(&dir, doc_name, artifact, envs);
                assert_eq!(
                    output.status.code(),
                    Some(0),
                    "document must compile: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                dir.join(artifact)
            };
        let route = compile_one("app.yaml", ROUTE_DOC, "route.bin", &[]);
        let job = compile_one("ingest.job.yaml", JOB_DOC, "job.bin", &[]);
        let failing_job = compile_one("fail.job.yaml", FAILING_JOB_DOC, "fail.bin", &[]);
        let env = compile_one(
            "greet.job.yaml",
            ENV_DOC,
            "env.bin",
            &[("DEPLOY_GREETING", "compile-secret-value")],
        );
        Fixture {
            route,
            job,
            failing_job,
            env,
        }
    })
}

/// Deploy a shared fixture artifact into a fresh source-free directory
/// (no source document, no Camel.toml, no routes tree) under the
/// canonical `app.bin` name. The artifact is hardlinked — zero-copy
/// sharing of the immutable fixture — with a copy fallback for
/// filesystems that refuse hard links. The deployed artifact is shared
/// with the fixture: never mutate it in place. Tests that need to alter
/// artifact bytes must copy first (see `artifact_rejects_marked_corruption`).
fn deploy_artifact(artifact: &Path) -> (tempfile::TempDir, PathBuf) {
    let deploy_dir = tempfile::tempdir().expect("deploy tempdir");
    let target = deploy_dir.path().join("app.bin");
    if std::fs::hard_link(artifact, &target).is_err() {
        std::fs::copy(artifact, &target).expect("copy artifact");
    }
    (deploy_dir, target)
}

/// Harness-child branch: decode the artifact named by [`CHILD_ENV`], parse
/// the artifact argv (after `--`), run the embedded document, and exit
/// with its code.
fn run_child() -> i32 {
    let artifact = std::env::var(CHILD_ENV).expect("child env names the artifact");
    let argv: Vec<String> = std::env::args().skip_while(|a| a != "--").skip(1).collect();
    let bytes = std::fs::read(&artifact).expect("child reads the artifact");
    let decoded = trailer::decode(&bytes)
        .expect("trailer must decode")
        .expect("artifact must carry the terminal marker");
    let args = match ArtifactArgs::parse(&argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let request = EmbeddedRequest::from_trailer(decoded, args).expect("embedded request");
    tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(async { camel_cli::compile::runtime::run_embedded_document_code(request).await })
}

/// Run the child branch if this process is a harness child; returns when
/// the child has exited.
fn child_guard() {
    if std::env::var(CHILD_ENV).is_ok() {
        std::process::exit(run_child());
    }
}

/// Spawn the artifact runtime as a harness child that runs to
/// completion (job sends are self-terminating): `output()` waits and
/// drains both pipes, so no pipe buffer can deadlock the child. Returns
/// the `(exit_code, stdout, stderr)` triple.
fn spawn_child_output(
    test: &str,
    dir: &Path,
    artifact: &Path,
    argv: &[&str],
    envs: &[(&str, &str)],
) -> (i32, String, String) {
    let mut cmd = Command::new(std::env::current_exe().expect("current test exe"));
    cmd.env(CHILD_ENV, artifact)
        .envs(envs.iter().copied())
        .current_dir(dir)
        .args(["--exact", test, "--nocapture", "--"])
        .args(argv)
        .stdin(Stdio::null())
        .output()
        .expect("spawn harness child (to completion)")
        .into_code_and_strings()
}

/// Exit code plus both captured streams of a finished child.
trait CodeAndStrings {
    fn into_code_and_strings(self) -> (i32, String, String);
}

impl CodeAndStrings for std::process::Output {
    fn into_code_and_strings(self) -> (i32, String, String) {
        (
            self.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&self.stdout).into_owned(),
            String::from_utf8_lossy(&self.stderr).into_owned(),
        )
    }
}

/// Spawn the artifact runtime as a harness child: `current_exe()` with
/// `--exact <test> --nocapture -- <argv>`, [`CHILD_ENV`] pointing at the
/// artifact, working directory `dir`, and extra environment entries.
fn spawn_child(
    test: &str,
    dir: &Path,
    artifact: &Path,
    argv: &[&str],
    envs: &[(&str, &str)],
) -> KillOnDrop {
    let mut cmd = Command::new(std::env::current_exe().expect("current test exe"));
    cmd.env(CHILD_ENV, artifact)
        .envs(envs.iter().copied())
        .current_dir(dir)
        .args(["--exact", test, "--nocapture", "--"])
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    KillOnDrop(cmd.spawn().expect("spawn harness child"))
}

/// Pipe-drained capture of both child streams (see `tests/common`).
struct Drained {
    out: Arc<Mutex<String>>,
    err: Arc<Mutex<String>>,
}

impl Drained {
    fn captured(&self) -> String {
        format!(
            "stdout:\n{}\nstderr:\n{}",
            self.out.lock().expect("stdout lock").clone(),
            self.err.lock().expect("stderr lock").clone()
        )
    }
}

fn spawn_drained(child: &mut Child) -> Drained {
    let out = Arc::new(Mutex::new(String::new()));
    let err = Arc::new(Mutex::new(String::new()));
    let stdout = child.stdout.take().expect("child stdout piped");
    let stderr = child.stderr.take().expect("child stderr piped");
    thread::spawn({
        let buf = Arc::clone(&out);
        move || drain_to_buffer(stdout, buf)
    });
    thread::spawn({
        let buf = Arc::clone(&err);
        move || drain_to_buffer(stderr, buf)
    });
    Drained { out, err }
}

/// Poll the captured buffers for `marker` until it appears, the child
/// dies, or `timeout` elapses (generous deadlines: see `tests/common`).
fn wait_for_marker(drained: &Drained, marker: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if drained.out.lock().expect("stdout lock").contains(marker)
            || drained.err.lock().expect("stderr lock").contains(marker)
        {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Wait for the child to exit at most `timeout`; returns the exit code,
/// or `-1` after a force kill at the deadline.
fn wait_exit_code(child: &mut KillOnDrop, timeout: Duration) -> i32 {
    let start = Instant::now();
    loop {
        match child.0.try_wait() {
            Ok(Some(status)) => return status.code().unwrap_or(-1),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.0.kill();
                    let _ = child.0.wait();
                    return -1;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }
}

/// SIGTERM after boot, then a graceful exit 0.
fn graceful_shutdown(child: &mut KillOnDrop, drained: &Drained, test: &str) -> i32 {
    assert!(
        wait_for_marker(drained, "context started", Duration::from_secs(60)),
        "artifact must boot through the embedded document: {}",
        drained.captured()
    );
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(child, Duration::from_secs(30));
    assert_eq!(code, 0, "SIGTERM must shut down gracefully: {}", test);
    code
}

/// A compiled route boots and serves from the embedded text alone: the
/// deploy directory holds only the artifact — no source document, no
/// Camel.toml, no routes tree.
#[test]
fn compiled_route_runs_without_source_tree() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().route);
    assert!(!deploy.path().join("app.yaml").exists(), "no source doc");
    assert!(!deploy.path().join("Camel.toml").exists(), "no config");

    let mut child = spawn_child(
        "compiled_route_runs_without_source_tree",
        deploy.path(),
        &artifact,
        &[],
        &[],
    );
    let drained = spawn_drained(&mut child);
    graceful_shutdown(
        &mut child,
        &drained,
        "compiled_route_runs_without_source_tree",
    );
}

/// A compiled one-shot job runs the existing job outcome/report
/// lifecycle: Completed exits 0 with the existing report schema; a
/// failing pipeline exits 1 with a Failed report (exit precedence).
#[test]
fn compiled_job_uses_existing_outcome_report() {
    child_guard();

    // Happy path: exit 0, Completed, existing report schema, virtual
    // document identity, captured reply.
    let (deploy, artifact) = deploy_artifact(&fixture().job);
    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_uses_existing_outcome_report",
        deploy.path(),
        &artifact,
        &["--report", "report.json"],
        &[],
    );
    assert_eq!(
        code, 0,
        "completed job must exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("report.json"))
            .expect("job report must be written"),
    )
    .expect("job report is JSON");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(
        report["document"], "compiled://ingest.job.yaml",
        "report: {report}"
    );
    assert_eq!(report["mode"], "one-shot", "report: {report}");
    assert_eq!(report["terminated_early"], false, "report: {report}");
    assert_eq!(report["reply"]["body"], "job-done", "report: {report}");

    // Failure precedence: a failing route pipeline exits 1 with Failed.
    let (deploy, artifact) = deploy_artifact(&fixture().failing_job);
    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_uses_existing_outcome_report",
        deploy.path(),
        &artifact,
        &["--report", "fail-report.json"],
        &[],
    );
    assert_eq!(
        code, 1,
        "pipeline failure must exit 1;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("fail-report.json"))
            .expect("failed job report must be written"),
    )
    .expect("job report is JSON");
    assert_eq!(report["outcome"], "Failed", "report: {report}");
    assert!(
        report["error"].as_str().is_some_and(|e| !e.is_empty()),
        "report: {report}"
    );
}

/// `${env:NAME}` survives compilation as an expression and resolves from
/// the deployment environment at run time.
#[test]
fn compiled_artifact_resolves_deploy_environment() {
    child_guard();
    // The fixture compiled ENV_DOC WITH a compile-time value present: it
    // must never enter the artifact.
    let (deploy, artifact) = deploy_artifact(&fixture().env);
    let artifact_bytes = std::fs::read(&artifact).expect("artifact exists");
    assert!(
        artifact_bytes
            .windows(b"${env:DEPLOY_GREETING}".len())
            .any(|w| w == b"${env:DEPLOY_GREETING}"),
        "artifact must keep the env expression"
    );
    assert!(
        !artifact_bytes
            .windows(b"compile-secret-value".len())
            .any(|w| w == b"compile-secret-value"),
        "artifact must not embed the compile-time value"
    );

    let (code, stdout, stderr) = spawn_child_output(
        "compiled_artifact_resolves_deploy_environment",
        deploy.path(),
        &artifact,
        &["--report", "env-report.json"],
        &[("DEPLOY_GREETING", "deploy-value")],
    );
    assert_eq!(
        code, 0,
        "job must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("env-report.json")).expect("report written"),
    )
    .expect("report is JSON");
    assert_eq!(
        report["reply"]["body"], "deploy-value",
        "route must observe the deployment value: {report}"
    );
}

/// A compiled artifact runs on a read-only root: no temporary
/// extraction, no watcher activation, and the only directory content
/// stays the artifact itself.
#[test]
fn compiled_artifact_does_not_extract_or_watch() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().route);

    // Read-only deploy root (owner r-x): any extraction would fail here.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(deploy.path(), std::fs::Permissions::from_mode(0o555))
            .expect("chmod read-only");
    }

    let mut child = spawn_child(
        "compiled_artifact_does_not_extract_or_watch",
        deploy.path(),
        &artifact,
        &[],
        &[],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(&drained, "context started", Duration::from_secs(60)),
        "artifact must boot on a read-only root: {}",
        drained.captured()
    );
    let all_output = format!(
        "{}{}",
        drained.out.lock().expect("stdout lock"),
        drained.err.lock().expect("stderr lock")
    );
    assert!(
        !all_output.contains("hot-reload watching"),
        "the watcher must never activate: {all_output}"
    );
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(&mut child, Duration::from_secs(30));
    assert_eq!(code, 0, "graceful shutdown on read-only root");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(deploy.path(), std::fs::Permissions::from_mode(0o755))
            .expect("restore writable for cleanup");
    }
    let mut entries: Vec<String> = std::fs::read_dir(deploy.path())
        .expect("read deploy dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec!["app.bin".to_string()],
        "no extraction or other writes: {entries:?}"
    );
}

/// A route artifact writes the exact RouteReport status JSON on graceful
/// shutdown and exits 0.
#[test]
fn compiled_route_report_writes_status_json() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().route);

    let mut child = spawn_child(
        "compiled_route_report_writes_status_json",
        deploy.path(),
        &artifact,
        &["--report", "status.json"],
        &[],
    );
    let drained = spawn_drained(&mut child);
    graceful_shutdown(
        &mut child,
        &drained,
        "compiled_route_report_writes_status_json",
    );

    let report = std::fs::read_to_string(deploy.path().join("status.json"))
        .expect("route status report must be written");
    assert_eq!(
        report.trim(),
        r#"{"kind":"route","status":"completed","error":null}"#,
        "exact RouteReport JSON"
    );
}

// ---------------------------------------------------------------------------
// Task 2.3: self-detection before CLI parsing. These tests spawn the
// artifact binary itself, so `main` runs the trailer probe before Clap.
// ---------------------------------------------------------------------------

/// Make `path` executable (artifacts written by hand in the tests below).
#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod executable");
}

/// A trailer-free binary keeps the normal Clap CLI: the probe returns
/// `None` and standard commands behave exactly as before. Clap
/// fingerprints: `--version` exits 0 printing `camel <version>`, and an
/// unknown flag is a Clap `error:` with exit 2.
#[test]
fn trailer_free_binary_keeps_normal_cli() {
    let camel = PathBuf::from(env!("CARGO_BIN_EXE_camel"));
    let dir = tempfile::tempdir().expect("tempdir");

    let (code, stdout, stderr) = common::run_binary(dir.path(), &camel, &["--version"], &[]);
    assert_eq!(
        code, 0,
        "plain `--version` exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.trim().starts_with("camel "),
        "Clap version output: {stdout}"
    );

    let (code, stdout, stderr) = common::run_binary(dir.path(), &camel, &["--watch"], &[]);
    assert_eq!(
        code, 2,
        "unknown flag is Clap misuse;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.starts_with("error:"),
        "Clap error fingerprint: {stderr}"
    );
}

/// `--manifest` prints the operational manifest and exits 0 without
/// booting the embedded route.
#[test]
fn artifact_manifest_exits_without_boot() {
    let (deploy, artifact) = deploy_artifact(&fixture().route);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    assert_eq!(
        code, 0,
        "--manifest exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is manifest JSON");
    assert_eq!(manifest["kind"], "route", "manifest: {manifest}");
    assert_eq!(manifest["source_name"], "app.yaml", "manifest: {manifest}");
    assert_eq!(
        manifest["runtime_version"],
        camel_cli::compile::manifest::RUNTIME_VERSION,
        "manifest: {manifest}"
    );
    assert!(
        manifest["components"]
            .as_array()
            .is_some_and(|c| c.iter().any(|s| s.as_str() == Some("timer"))),
        "embedded components listed: {manifest}"
    );
    assert!(
        manifest["env_names"].as_array().is_some(),
        "required env names listed: {manifest}"
    );
    assert!(
        manifest["listeners"].as_array().is_some(),
        "listener declarations listed: {manifest}"
    );
    let all = format!("{stdout}{stderr}");
    assert!(!all.contains("context started"), "no route boot: {all}");
}

/// Duplicate/exclusive flags, a missing `--report` value, an unknown
/// flag, and a positional argument each exit 2 and name the rejected
/// argument, without booting.
#[test]
fn artifact_rejects_unknown_and_positional_args() {
    let (deploy, artifact) = deploy_artifact(&fixture().route);
    let cases: &[(&[&str], &str)] = &[
        (&["--help", "--version"], "--version"),
        (&["--report", "a.json", "--report", "b.json"], "--report"),
        (&["--report"], "--report"),
        (&["--watch"], "--watch"),
        (&["routes.yaml"], "routes.yaml"),
    ];
    for (argv, named) in cases {
        let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, argv, &[]);
        assert_eq!(
            code, 2,
            "argv {argv:?} must exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let combined = format!("{stdout}{stderr}");
        assert!(
            combined.contains(named),
            "argv {argv:?} must name the rejected argument: {combined}"
        );
        assert!(!combined.contains("context started"), "no boot: {combined}");
    }
}

/// Marked corruption (terminal magic retained) fails closed: nonzero
/// integrity diagnostic and no boot. One case mutates the last
/// embedded-data byte (payload/manifest region, past the executable
/// image); the other mutates a footer checksum byte. Both break the
/// BLAKE3 checksum while the terminal magic stays intact.
#[test]
fn artifact_rejects_marked_corruption() {
    let (deploy, artifact) = deploy_artifact(&fixture().route);
    let valid = std::fs::read(&artifact).expect("artifact bytes");

    let mut corrupt_data = valid.clone();
    let data_end = corrupt_data.len() - trailer::FOOTER_LEN;
    corrupt_data[data_end - 1] ^= 0xFF;
    let mut corrupt_footer = valid.clone();
    corrupt_footer[data_end + 28] ^= 0xFF;

    for (name, bytes) in [("data", corrupt_data), ("footer", corrupt_footer)] {
        let path = deploy.path().join(format!("corrupt-{name}.bin"));
        std::fs::write(&path, bytes).expect("write corrupt artifact");
        #[cfg(unix)]
        make_executable(&path);
        let (code, stdout, stderr) = common::run_binary(deploy.path(), &path, &[], &[]);
        assert_eq!(
            code, 2,
            "corrupt {name} must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let combined = format!("{stdout}{stderr}");
        assert!(
            combined.contains("integrity error"),
            "corrupt {name} must carry an integrity diagnostic: {combined}"
        );
        assert!(!combined.contains("context started"), "no boot: {combined}");
    }
}

/// Truncation through the terminal magic leaves no recognizable trailer,
/// so the image is indistinguishable from a plain executable and falls
/// back to the unchanged Clap path.
#[test]
fn artifact_truncated_without_marker_keeps_clap_fallback() {
    let (deploy, artifact) = deploy_artifact(&fixture().route);
    let mut bytes = std::fs::read(&artifact).expect("artifact bytes");
    // Cut exactly the terminal magic: decode would report an absent
    // trailer, so argv must reach Clap unchanged.
    bytes.truncate(bytes.len() - trailer::MAGIC.len());
    assert_eq!(
        trailer::decode(&bytes),
        Ok(None),
        "truncation must remove the marker"
    );
    let path = deploy.path().join("truncated.bin");
    std::fs::write(&path, bytes).expect("write truncated artifact");
    #[cfg(unix)]
    make_executable(&path);

    // A normal CLI argument: Clap rejects the unknown flag with its own
    // `error:` fingerprint and exit 2 (the artifact argv guard would not
    // print that prefix).
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &path, &["--watch"], &[]);
    assert_eq!(
        code, 2,
        "Clap misuse exits 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.starts_with("error:"),
        "unchanged Clap fallback: {stderr}"
    );
}

/// `--help` and `--version` each exit 0 without booting.
#[test]
fn artifact_help_and_version_exit_zero() {
    let (deploy, artifact) = deploy_artifact(&fixture().route);

    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--help"], &[]);
    assert_eq!(
        code, 0,
        "--help exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("camel compiled artifact usage"),
        "artifact usage text: {stdout}"
    );

    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--version"], &[]);
    assert_eq!(
        code, 0,
        "--version exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.trim(),
        format!("camel {}", camel_cli::compile::manifest::RUNTIME_VERSION),
        "artifact version line"
    );

    for stream in [&stdout, &stderr] {
        assert!(!stream.contains("context started"), "no boot: {stream}");
    }
}
