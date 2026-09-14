//! Unit tests for the `camel job` report, exit-code, and
//! shutdown-budget contracts, split out of `mod.rs` for file-size
//! hygiene. Behavior and test names are unchanged.

// ---- jobargs Task 3.1 harness -------------------------------------------
//
// The declared-argument and legacy-header tests act through the real
// `camel job` execution (boot, send, report) because the contract under
// test spans argv parsing, pre-boot validation, and the send path. The
// subprocess boundary is also the only way to observe stderr (the
// deprecation note and pre-boot diagnostics).

/// Locate the built `camel` binary. `CARGO_BIN_EXE_camel` is NOT set
/// for unit tests (Cargo only sets it for integration-test targets),
/// so fall back to the package target dir, probing the dev profile
/// first and the release profile second.
///
/// The harness probes `target/debug/camel`; a plain `cargo test --lib`
/// does not rebuild the binary, so run `cargo test -p camel-cli
/// commands::job::tests` or build first.
fn job_test_binary() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_camel") {
        return std::path::PathBuf::from(path);
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")
        });
    let dev = target.join("debug").join("camel");
    if dev.exists() {
        return dev;
    }
    let release = target.join("release").join("camel");
    assert!(
        release.exists(),
        "camel binary not built: run `cargo build -p camel-cli` (probed {} and {})",
        dev.display(),
        release.display()
    );
    release
}

/// Run `camel job` in `dir` to completion and return
/// `(exit_code, stdout, stderr)`. Blocks until the child exits; the
/// job fixtures use short one-shot runs. The ergonomic no-env default
/// over [`run_camel_job_env`].
fn run_camel_job(dir: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    run_camel_job_env(dir, args, &[])
}

/// The fixture config: logs off so stderr carries only diagnostics the
/// tests assert on.
fn write_job_fixture_config(dir: &std::path::Path) {
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

/// A direct: tap route with no steps: the reply echoes the input
/// exchange (body and headers survive the empty pipeline), so
/// `capture-reply` reports what the send actually carried.
fn write_tap_route(dir: &std::path::Path) {
    std::fs::create_dir(dir.join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.join("routes/job-route.yaml"),
        r#"routes:
  - id: "job-tap"
    from: "direct:tap"
"#,
    )
    .expect("write route");
}

/// Parse the JSON report written to `path`.
fn read_report(path: &std::path::Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path).expect("report file exists");
    serde_json::from_str(text.trim()).expect("report is JSON")
}

/// Unknown `--arg` names and omitted required arguments fail at the
/// load-time validation stage with exit 2 and a diagnostic naming the
/// argument — BEFORE boot. The fixture's route file is missing, so a
/// post-boot run would fail with the route-discovery error instead:
/// the arg diagnostic winning proves validation precedes boot (the
/// control case with valid args still shows the route error).
#[test]
fn declared_args_validate_before_boot() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"args:
  name:
    required: true
  tier:
    default: gold
execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:tap
    body: "ping"
routeFiles:
  - routes/missing.yaml
"#,
    )
    .expect("write job doc");

    // Unknown name: exit 2, named diagnostic, no boot failure text.
    let (code, _stdout, stderr) = run_camel_job(dir.path(), &["job.job.yaml", "--arg", "other=x"]);
    assert_eq!(code, 2, "unknown --arg must exit 2; stderr:\n{stderr}");
    assert!(
        stderr.contains("unknown argument"),
        "diagnostic must name the class; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("other"),
        "diagnostic must name the offending argument; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("camel-cli job failed"),
        "validation must fail before boot; stderr:\n{stderr}"
    );

    // Missing required: exit 2, named diagnostic.
    let (code, _stdout, stderr) = run_camel_job(dir.path(), &["job.job.yaml"]);
    assert_eq!(
        code, 2,
        "missing required arg must exit 2; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("missing required argument"),
        "diagnostic must name the class; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("`name`"),
        "diagnostic must name the missing argument; stderr:\n{stderr}"
    );

    // Control: valid args pass validation and the run proceeds to the
    // (post-boot) route-discovery failure — proving the two failures
    // come from different stages.
    let (code, _stdout, stderr) = run_camel_job(dir.path(), &["job.job.yaml", "--arg", "name=x"]);
    assert_eq!(code, 2, "missing route file exits 2; stderr:\n{stderr}");
    assert!(
        !stderr.contains("unknown argument") && !stderr.contains("missing required argument"),
        "valid args must pass validation; stderr:\n{stderr}"
    );
}

/// Declaration defaults apply when the CLI omits the argument, and an
/// explicit `--arg` value wins over the default — resolved through
/// `${arg:}` interpolation in the send body.
#[test]
fn declared_defaults_and_explicit_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    write_tap_route(dir.path());
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"args:
  tier:
    default: gold
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:tap
    body: "value=${arg:tier}"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");
    let report = dir.path().join("report.json");

    // Default applies.
    let (code, _stdout, stderr) = run_camel_job(
        dir.path(),
        &["job.job.yaml", "--report", report.to_str().expect("utf8")],
    );
    assert_eq!(code, 0, "default run must complete; stderr:\n{stderr}");
    let json = read_report(&report);
    assert_eq!(json["outcome"], "Completed", "report: {json}");
    assert_eq!(json["reply"]["body"], "value=gold", "report: {json}");

    // Explicit value wins over the default.
    let (code, _stdout, stderr) = run_camel_job(
        dir.path(),
        &[
            "job.job.yaml",
            "--report",
            report.to_str().expect("utf8"),
            "--arg",
            "tier=silver",
        ],
    );
    assert_eq!(code, 0, "explicit run must complete; stderr:\n{stderr}");
    let json = read_report(&report);
    assert_eq!(json["outcome"], "Completed", "report: {json}");
    assert_eq!(json["reply"]["body"], "value=silver", "report: {json}");
}

/// On documents without `args:`, repeated `--arg` pairs stay raw
/// string headers applied after document headers — last occurrence
/// wins and overrides the colliding document header — and stderr
/// carries the deprecation note identifying the legacy behavior.
#[test]
fn legacy_args_remain_headers_with_deprecation() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    write_tap_route(dir.path());
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:tap
    body: "ping"
    headers:
      name: Doc
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");
    let report = dir.path().join("report.json");

    let (code, _stdout, stderr) = run_camel_job(
        dir.path(),
        &[
            "job.job.yaml",
            "--report",
            report.to_str().expect("utf8"),
            "--arg",
            "name=First",
            "--arg",
            "name=Last",
        ],
    );
    assert_eq!(code, 0, "legacy run must complete; stderr:\n{stderr}");
    let json = read_report(&report);
    assert_eq!(json["outcome"], "Completed", "report: {json}");
    // Last occurrence wins and overrides the document header; the
    // value stays the raw string (no interpolation, no typing).
    assert_eq!(json["reply"]["headers"]["name"], "Last", "report: {json}");
    assert!(
        stderr.contains("--arg header injection"),
        "stderr must carry the deprecation note; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("deprecated"),
        "stderr must mark the legacy behavior deprecated; stderr:\n{stderr}"
    );
}

/// A declared argument resolves through `${arg:}` interpolation and is
/// NEVER injected as an implicit header: the body receives the value
/// while the recorded reply headers do not contain the argument name
/// (the marker header proves the reply headers are visible at all).
#[test]
fn declared_arg_is_not_implicit_header() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    write_tap_route(dir.path());
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"args:
  name:
    required: true
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:tap
    body: "${arg:name}"
    headers:
      X-Marker: m
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");
    let report = dir.path().join("report.json");

    let (code, _stdout, stderr) = run_camel_job(
        dir.path(),
        &[
            "job.job.yaml",
            "--report",
            report.to_str().expect("utf8"),
            "--arg",
            "name=John",
        ],
    );
    assert_eq!(code, 0, "declared run must complete; stderr:\n{stderr}");
    let json = read_report(&report);
    assert_eq!(json["outcome"], "Completed", "report: {json}");
    assert_eq!(json["reply"]["body"], "John", "report: {json}");
    assert!(
        json["reply"]["headers"].get("name").is_none(),
        "declared args must not become implicit headers: {json}"
    );
    assert_eq!(
        json["reply"]["headers"]["X-Marker"], "m",
        "marker header must be present so the negative assert is not vacuous: {json}"
    );
    assert!(
        !stderr.contains("deprecated"),
        "declared documents are not the legacy path; stderr:\n{stderr}"
    );
}

/// `${arg:}` resolves in all four field positions — `to`, `body`,
/// `headers`, and `timeout` — at the shared interpolation stage before
/// field validation (the raw `timeout: "${arg:wait}"` would fail the
/// duration grammar without the pre-validation resolution).
#[test]
fn declared_args_interpolate_all_field_positions() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    write_tap_route(dir.path());
    std::fs::write(
        dir.path().join("job.job.yaml"),
        r#"args:
  target:
    default: direct:tap
  text:
    default: hello
  header:
    default: gold
  wait:
    default: 60s
execute:
  mode: one-shot
  timeout: "${arg:wait}"
  capture-reply: true
  send:
    to: "${arg:target}"
    body: "${arg:text}"
    headers:
      X-Tier: "${arg:header}"
routeFiles:
  - routes/job-route.yaml
"#,
    )
    .expect("write job doc");
    let report = dir.path().join("report.json");

    let (code, _stdout, stderr) = run_camel_job(
        dir.path(),
        &["job.job.yaml", "--report", report.to_str().expect("utf8")],
    );
    assert_eq!(code, 0, "all-field run must complete; stderr:\n{stderr}");
    let json = read_report(&report);
    assert_eq!(json["outcome"], "Completed", "report: {json}");
    assert_eq!(json["reply"]["body"], "hello", "report: {json}");
    assert_eq!(json["reply"]["headers"]["X-Tier"], "gold", "report: {json}");
}

// ---- jobhelp Task 1.3: `--help` wiring ----------------------------------
//
// The `--help` contract spans argv parsing, bare-name resolution, the
// help projection parse, and the pre-boot early return, so the tests
// act through the same subprocess harness as the jobargs family.

/// Run `camel job` in `dir` with extra environment entries set on the
/// child and return `(exit_code, stdout, stderr)`. The single spawn
/// path — [`run_camel_job`] delegates here with no extra environment
/// (e.g. `CAMEL_JOB_SIGNAL_MARKER` opt-in).
fn run_camel_job_env(
    dir: &std::path::Path,
    args: &[&str],
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> (i32, String, String) {
    let mut full: Vec<&str> = vec!["job"];
    full.extend(args.iter().copied());
    let output = std::process::Command::new(job_test_binary())
        .args(full)
        .envs(env.iter().cloned())
        .current_dir(dir)
        .output()
        .expect("spawn camel binary");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Write one job document under the default `[jobs]` root
/// (`jobs/<name>.job.yaml`): bare-name resolution probes exactly this
/// spelling through the default config (no `[jobs]` table). The
/// jobargs fixtures use explicit paths, so this writer is new.
fn write_jobs_root_document(dir: &std::path::Path, name: &str, document: &str) {
    let jobs = dir.join("jobs");
    std::fs::create_dir_all(&jobs).expect("mkdir jobs");
    std::fs::write(jobs.join(format!("{name}.job.yaml")), document).expect("write job doc");
}

/// A minimal valid job document with the given description, args
/// block, and send target: one route source (inline `routes:`), a
/// valid `execute:` grammar, and no external route files — the help
/// path never loads routes, so the fixture stays self-contained.
fn jobs_root_help_document(description: &str, args: &str, send_to: &str) -> String {
    format!(
        r#"description: {description}
{args}execute:
  mode: one-shot
  timeout: 60s
  send:
    to: {send_to}
    body: "ping"
routes: |
  routes:
    - id: "job-tap"
      from: "direct:tap"
"#
    )
}

/// `--help` with a job name renders the declared interface: the stem,
/// description, the mode/sends-to pair, and one row per declared
/// argument — never clap's own help (no `Usage:` line).
#[test]
fn help_with_name_renders_declared_interface() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    let document = jobs_root_help_document(
        "Ingest the daily feed",
        r#"args:
  target:
    required: true
    description: Where to send the feed
  retries:
    default: "3"
    description: How many attempts to make
"#,
        "direct:tap",
    );
    write_jobs_root_document(dir.path(), "daily-sync", &document);

    let (code, stdout, stderr) = run_camel_job(dir.path(), &["daily-sync", "--help"]);
    assert_eq!(code, 0, "--help must exit 0; stderr:\n{stderr}");
    assert!(
        stdout.starts_with("daily-sync"),
        "stdout must start with the stem; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("Ingest the daily feed"),
        "description must render; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("Mode:      one-shot"),
        "mode row must render; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("Sends to:"),
        "send target row must render; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("  retries  string  optional  default=3  How many attempts to make"),
        "optional argument row must render; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("  target   string  required  Where to send the feed"),
        "required argument row must render; stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("Usage:"),
        "the interface is not clap help; stdout:\n{stdout}"
    );
}

/// The help render shows the RAW `${arg:}` send target without pair
/// validation: a required argument missing its `--arg` value, which
/// the execution path rejects pre-boot, never blocks `--help`.
#[test]
fn help_with_name_reports_required_args_without_pairs() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    let document = jobs_root_help_document(
        "Ingest the daily feed",
        r#"args:
  target:
    required: true
"#,
        r#""${arg:target}""#,
    );
    write_jobs_root_document(dir.path(), "daily-sync", &document);

    // No --arg: the execution path would fail pair validation; help
    // must still render.
    let (code, stdout, stderr) = run_camel_job(dir.path(), &["daily-sync", "--help"]);
    assert_eq!(
        code, 0,
        "--help must exit 0 without pairs; stderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Sends to:  ${arg:target}"),
        "raw ${{arg:}} token must survive verbatim; stdout:\n{stdout}"
    );
}

/// A document without `args:` renders the `Arguments:` section with
/// the explicit `(no arguments)` row.
#[test]
fn help_no_args_block_prints_no_arguments() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    let document = jobs_root_help_document("Do the thing quietly", "", "direct:tap");
    write_jobs_root_document(dir.path(), "quiet-sync", &document);

    let (code, stdout, stderr) = run_camel_job(dir.path(), &["quiet-sync", "--help"]);
    assert_eq!(code, 0, "--help must exit 0; stderr:\n{stderr}");
    assert!(
        stdout.contains("Arguments:\n  (no arguments)"),
        "absent args block must render the placeholder row; stdout:\n{stdout}"
    );
}

/// `--help` returns before any boot, report write, or signal-stream
/// arming: with the marker env opting in, no marker line is printed,
/// and the `--report` path is never touched.
#[test]
fn help_writes_no_report_and_boots_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    write_tap_route(dir.path());
    // A fully runnable document: if the help path wrongly booted, the
    // run would complete and write the report — the missing file
    // proves the early return.
    write_jobs_root_document(
        dir.path(),
        "daily-sync",
        r#"description: Ingest the daily feed
execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:tap
    body: "ping"
routeFiles:
  - ../routes/job-route.yaml
"#,
    );
    let report = dir.path().join("out.json");

    let (code, stdout, stderr) = run_camel_job_env(
        dir.path(),
        &["daily-sync", "--help", "--report", "out.json"],
        &[(
            std::ffi::OsString::from("CAMEL_JOB_SIGNAL_MARKER"),
            std::ffi::OsString::from("1"),
        )],
    );
    assert_eq!(code, 0, "--help must exit 0; stderr:\n{stderr}");
    assert!(
        !report.exists(),
        "help must not write the report file; stdout:\n{stdout}"
    );
    assert!(
        stdout.starts_with("daily-sync"),
        "stdout is the declared interface; stdout:\n{stdout}"
    );
    assert!(
        !stderr.contains("signal streams armed"),
        "help installs no signal streams; stderr:\n{stderr}"
    );
}

/// An unknown bare name under `--help` fails with the existing
/// bare-name resolution diagnostic (exit 2), not clap help.
#[test]
fn help_unknown_name_fails_loud() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    let other = jobs_root_help_document("Unrelated job", "", "direct:tap");
    write_jobs_root_document(dir.path(), "other", &other);

    let (code, _stdout, stderr) = run_camel_job(dir.path(), &["ghost", "--help"]);
    assert_eq!(code, 2, "unknown name must exit 2; stderr:\n{stderr}");
    assert!(
        stderr.contains("ghost"),
        "diagnostic must name the job; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("no job `ghost` in any configured root"),
        "bare-name resolution diagnostic must carry; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("Usage:"),
        "failure is not clap help; stderr:\n{stderr}"
    );
}

/// A malformed document under `--help` fails with the parse
/// diagnostic (exit 2), not clap help.
#[test]
fn help_malformed_document_fails_loud() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    write_jobs_root_document(
        dir.path(),
        "broken",
        r#"description: Broken on purpose
totallyUnknownField: yes
execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:tap
routes: |
  routes:
    - id: "job-tap"
      from: "direct:tap"
"#,
    );

    let (code, _stdout, stderr) = run_camel_job(dir.path(), &["broken", "--help"]);
    assert_eq!(code, 2, "malformed document must exit 2; stderr:\n{stderr}");
    assert!(
        stderr.contains("unknown field in job document"),
        "parse diagnostic must carry; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("Usage:"),
        "failure is not clap help; stderr:\n{stderr}"
    );
}

/// `--help` without a name prints `camel job` usage; a bare
/// `camel job` invocation still prints the discovery listing.
#[test]
fn help_without_name_prints_usage() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    let document = jobs_root_help_document("Ingest the daily feed", "", "direct:tap");
    write_jobs_root_document(dir.path(), "daily-sync", &document);

    let (code, stdout, _stderr) = run_camel_job(dir.path(), &["--help"]);
    assert_eq!(code, 0, "usage help must exit 0");
    assert!(
        stdout.contains("Usage: camel job"),
        "usage line must carry; stdout:\n{stdout}"
    );

    // No flags: the A1 discovery listing still runs.
    let (code, stdout, _stderr) = run_camel_job(dir.path(), &[]);
    assert_eq!(code, 0, "bare listing must stay exit 0");
    assert!(
        stdout.contains("Jobs in jobs/:"),
        "bare invocation still lists; stdout:\n{stdout}"
    );
}

/// `-h` behaves exactly like `--help`: same exit code, same stdout.
#[test]
fn help_short_flag_behaves_like_long() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_job_fixture_config(dir.path());
    let document = jobs_root_help_document(
        "Ingest the daily feed",
        r#"args:
  target:
    required: true
    description: Where to send the feed
  retries:
    default: "3"
    description: How many attempts to make
"#,
        "direct:tap",
    );
    write_jobs_root_document(dir.path(), "daily-sync", &document);

    let (long_code, long_stdout, _long_stderr) =
        run_camel_job(dir.path(), &["daily-sync", "--help"]);
    let (short_code, short_stdout, short_stderr) = run_camel_job(dir.path(), &["daily-sync", "-h"]);
    assert_eq!(short_code, 0, "-h must exit 0; stderr:\n{short_stderr}");
    assert_eq!(long_code, 0, "--help must exit 0");
    assert_eq!(
        short_stdout, long_stdout,
        "-h and --help must render identically"
    );
}

mod exit_code_tests {
    use crate::commands::job::{JobReport, exit_code_for};

    /// An `Interrupted` report maps to exit code 2 (apparatus class,
    /// same as load/boot/timeout/shutdown errors).
    #[test]
    fn job_exit_code_interrupted() {
        let report = JobReport {
            document: "doc".to_string(),
            mode: "one-shot".to_string(),
            outcome: "Interrupted",
            terminated_early: false,
            duration_ms: 1,
            reply: None,
            error: Some("interrupted by signal (SIGINT/SIGTERM)".to_string()),
            shutdown_error: None,
        };
        assert_eq!(exit_code_for(report.outcome), 2);
    }
}

mod report_tests {
    use crate::commands::job::{
        JobReport, MIN_SHUTDOWN_BUDGET, exit_code_for, record_shutdown_failure,
    };

    /// An `Interrupted` report with a shutdown detail serializes both:
    /// `outcome` stays `Interrupted` and `shutdown_error` is present.
    #[test]
    fn job_report_interrupted_serializes() {
        let report = JobReport {
            document: "doc".to_string(),
            mode: "one-shot".to_string(),
            outcome: "Interrupted",
            terminated_early: false,
            duration_ms: 1,
            reply: None,
            error: Some("interrupted by signal (SIGINT/SIGTERM)".to_string()),
            shutdown_error: Some("shutdown failure: x".to_string()),
        };
        let json = serde_json::to_value(&report).expect("report must serialize");
        assert_eq!(
            json["outcome"],
            serde_json::json!("Interrupted"),
            "outcome must stay Interrupted: {json}"
        );
        assert!(
            json["shutdown_error"].is_string(),
            "shutdown detail must serialize: {json}"
        );
    }

    /// A shutdown failure after an interruption finalizes without
    /// replacing the verdict: `shutdown_error` is recorded for a
    /// non-zero budget, the outcome stays `Interrupted`, and the exit
    /// code is 2.
    #[test]
    fn job_interrupted_shutdown_failure_preserves_verdict() {
        let mut report = JobReport {
            document: "doc".to_string(),
            mode: "one-shot".to_string(),
            outcome: "Interrupted",
            terminated_early: false,
            duration_ms: 1,
            reply: None,
            error: Some("interrupted by signal (SIGINT/SIGTERM)".to_string()),
            shutdown_error: None,
        };
        record_shutdown_failure(
            &mut report,
            "shutdown failure: x".to_string(),
            MIN_SHUTDOWN_BUDGET,
        );
        assert_eq!(report.outcome, "Interrupted");
        assert_eq!(
            report.shutdown_error.as_deref(),
            Some("shutdown failure: x"),
            "non-zero-budget teardown detail must be recorded"
        );
        assert_eq!(exit_code_for(report.outcome), 2);
    }

    /// A shutdown failure after a recorded verdict serializes alongside
    /// the verdict error: `error` keeps the pipeline/timeout detail and
    /// `shutdown_error` carries the teardown detail.
    #[test]
    fn shutdown_error_serializes_alongside_error() {
        let report = JobReport {
            document: "doc".to_string(),
            mode: "one-shot".to_string(),
            outcome: "Failed",
            terminated_early: false,
            duration_ms: 1,
            reply: None,
            error: Some("pipeline failed".to_string()),
            shutdown_error: Some("shutdown failure: x".to_string()),
        };
        let json = serde_json::to_string(&report).expect("report must serialize");
        assert!(
            json.contains("pipeline failed"),
            "verdict error must serialize: {json}"
        );
        assert!(
            json.contains("shutdown failure: x"),
            "shutdown detail must serialize: {json}"
        );
    }

    /// Without a shutdown failure the `shutdown_error` key is omitted
    /// from the JSON report.
    #[test]
    fn shutdown_error_omitted_when_absent() {
        let report = JobReport {
            document: "doc".to_string(),
            mode: "one-shot".to_string(),
            outcome: "Failed",
            terminated_early: false,
            duration_ms: 1,
            reply: None,
            error: Some("pipeline failed".to_string()),
            shutdown_error: None,
        };
        let json = serde_json::to_string(&report).expect("report must serialize");
        assert!(
            !json.contains("shutdown_error"),
            "absent shutdown_error must be omitted: {json}"
        );
    }
}

mod shutdown_budget_tests {
    use std::time::{Duration, Instant};

    use crate::commands::job::document::JobMode;
    use crate::commands::job::{MIN_SHUTDOWN_BUDGET, shutdown_budget};

    /// Batch: the budget is the remaining wall clock, uncapped below.
    #[test]
    fn shutdown_budget_batch_is_remaining() {
        let deadline = Instant::now() + Duration::from_secs(3);
        let budget = shutdown_budget(JobMode::Batch, deadline);
        assert!(
            budget <= Duration::from_secs(3) && budget > Duration::from_secs(2),
            "expected ~3s remaining, got {budget:?}"
        );
    }

    /// Batch with a spent deadline: zero budget, no floor.
    #[test]
    fn shutdown_budget_batch_zero_when_past() {
        let deadline = Instant::now() - Duration::from_secs(1);
        assert_eq!(shutdown_budget(JobMode::Batch, deadline), Duration::ZERO);
    }

    /// One-shot with a spent deadline: floored to MIN_SHUTDOWN_BUDGET.
    #[test]
    fn shutdown_budget_one_shot_floored() {
        let deadline = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            shutdown_budget(JobMode::OneShot, deadline),
            MIN_SHUTDOWN_BUDGET
        );
    }

    /// One-shot with ample remaining time: the remaining clock wins over
    /// the floor.
    #[test]
    fn shutdown_budget_one_shot_is_remaining_when_large() {
        let deadline = Instant::now() + Duration::from_secs(10);
        let budget = shutdown_budget(JobMode::OneShot, deadline);
        assert!(
            budget <= Duration::from_secs(10) && budget > MIN_SHUTDOWN_BUDGET,
            "expected ~10s remaining, got {budget:?}"
        );
    }

    /// Interruption teardown budget by mode with matching deadlines:
    /// an interrupted one-shot gets at least `MIN_SHUTDOWN_BUDGET`
    /// (the floor lifts a spent or short deadline), while an
    /// interrupted batch gets only the remaining deadline with no
    /// floor (zero once the deadline is spent).
    #[test]
    fn job_interrupted_shutdown_budget_by_mode() {
        // Spent deadline: one-shot floored, batch zero.
        let spent = Instant::now() - Duration::from_secs(1);
        assert!(
            shutdown_budget(JobMode::OneShot, spent) >= MIN_SHUTDOWN_BUDGET,
            "interrupted one-shot teardown keeps the floor"
        );
        assert_eq!(
            shutdown_budget(JobMode::Batch, spent),
            Duration::ZERO,
            "interrupted batch teardown has no floor"
        );
        // Live deadline below the floor: one-shot is lifted to the
        // floor, batch keeps the raw remaining clock.
        let soon = Instant::now() + Duration::from_secs(3);
        assert_eq!(shutdown_budget(JobMode::OneShot, soon), MIN_SHUTDOWN_BUDGET);
        let batch = shutdown_budget(JobMode::Batch, soon);
        assert!(
            batch <= Duration::from_secs(3) && batch > Duration::from_secs(2),
            "interrupted batch teardown gets the remaining deadline, got {batch:?}"
        );
    }
}

mod store_plan_tests {
    use crate::commands::job::filter_store_source_plan;
    use crate::compile::store::{StoreDocument, StoreEntryKind, VirtualDocumentStore};

    /// A store with the entry job document, one route document, and a
    /// SECOND job document; `plan` selects the source-plan references.
    fn store(plan: &[&str]) -> VirtualDocumentStore {
        let job_text =
            "execute:\n  mode: one-shot\n  timeout: 30s\n  send:\n    to: direct:start\n";
        VirtualDocumentStore::build(
            "job.job.yaml",
            &[
                StoreDocument {
                    path: "job.job.yaml".to_string(),
                    kind: StoreEntryKind::Job,
                    bytes: job_text.as_bytes().to_vec(),
                },
                StoreDocument {
                    path: "other.job.yaml".to_string(),
                    kind: StoreEntryKind::Job,
                    bytes: job_text.as_bytes().to_vec(),
                },
                StoreDocument {
                    path: "routes/a.yaml".to_string(),
                    kind: StoreEntryKind::Route,
                    bytes: "routes:\n  - id: a\n    from: timer:a\n"
                        .as_bytes()
                        .to_vec(),
                },
            ],
            &[],
            &plan.iter().map(|p| (*p).to_string()).collect::<Vec<_>>(),
        )
        .expect("valid store builds")
    }

    /// The entry job document is dropped from the plan (it is parsed
    /// separately); route references are kept in declared order.
    #[test]
    fn store_plan_entry_dropped_routes_kept() {
        let mut store = store(&["job.job.yaml", "routes/a.yaml"]);
        assert_eq!(filter_store_source_plan(&mut store), None);
        assert_eq!(store.index.source_plan.references, vec!["routes/a.yaml"]);
    }

    /// A SECOND job-kind plan reference is named and rejected, never
    /// silently dropped: only the entry point may leave the plan.
    #[test]
    fn store_plan_extra_job_reference_named() {
        let mut store = store(&["job.job.yaml", "routes/a.yaml", "other.job.yaml"]);
        assert_eq!(
            filter_store_source_plan(&mut store),
            Some("other.job.yaml".to_string())
        );
    }
}
