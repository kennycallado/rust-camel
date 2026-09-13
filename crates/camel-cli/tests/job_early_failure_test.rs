//! End-to-end regression tests for the `camel job` post-boot teardown
//! border (jobteardown Task 1.2, bd rc-7wl19): every failure path after
//! the boot handle is acquired must run the bounded context shutdown
//! before the exit-2 return. The opt-in `CAMEL_JOB_SHUTDOWN_MARKER`
//! stderr line (`camel job: shutdown complete`, emitted exactly once
//! per shutdown invocation by the shared shutdown helper) makes the
//! teardown observable without touching production output.
//!
//! Both tests wait for the marker BEFORE collecting the final exit
//! status, so the assertions distinguish a real bounded teardown from a
//! bare exit 2 — a pre-fix child exits 2 without any marker and the
//! marker wait times out.
//!
//! The harness mirrors `job_signal_test.rs`: `common::spawn_camel_job_with_env`,
//! `common::spawn_drained`, `common::wait_for_marker`, and
//! `common::wait_exit_code_bounded`. Diagnostics under test are plain
//! `eprintln!` lines on stderr (unbuffered), independent of the tracing
//! level.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{spawn_camel_job_with_env, spawn_drained, wait_exit_code_bounded, wait_for_marker};

/// Exact shutdown-completion marker emitted by the shared shutdown
/// helper, once per shutdown invocation, when `CAMEL_JOB_SHUTDOWN_MARKER`
/// is set.
const SHUTDOWN_MARKER: &str = "camel job: shutdown complete";

/// Observation and exit ceiling, 30 s per the `common/mod.rs`
/// convention: the binary boots slowly under a saturated whole-workspace
/// `cargo test`, and the teardown is bounded (one-shot 5 s floor). The
/// transport fixture additionally spends the 3 s send retry window
/// before the transport branch fires, well under the ceiling.
const WAIT: Duration = Duration::from_secs(30);

/// Write the standard job-fixture config (`log_level = "off"`: the
/// diagnostics under test are `eprintln!` lines, not tracing events, so
/// the tracing layer stays silent).
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

/// Write the ambiguous-target fixture: two consumer routes share the
/// `direct:work` base, so the load-time target gate rejects the send as
/// ambiguous AFTER the boot handle is acquired and BEFORE any route is
/// registered or started. Pre-fix the command exits 2 without any
/// teardown; post-fix the teardown border shuts the booted context down
/// first.
fn write_ambiguous_target_fixture(dir: &Path) {
    std::fs::create_dir(dir.join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.join("routes/ambig-route.yaml"),
        r#"routes:
  - id: "work-a"
    from: "direct:work"
    steps:
      - set_body:
          value: "a"
  - id: "work-b"
    from: "direct:work"
    steps:
      - set_body:
          value: "b"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.join("job.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:work
routeFiles:
  - routes/ambig-route.yaml
"#,
    )
    .expect("write job doc");
}

/// Write the deterministic transport fixture. The send target
/// `direct:go?block=true` matches the consumer route's base for the
/// target gate, but the direct endpoint parser rejects the `block`
/// option unconditionally, so `create_endpoint` fails persistently and
/// the outer send failure survives the bounded retry window as
/// `SendError::Transport` — the existing transport branch (shutdown,
/// then exit 2). The failure needs no network, listener, or peer
/// process, which keeps the test deterministic; a closed-localhost
/// route hop cannot reach this branch because a `to:`-hop failure is
/// the route pipeline's verdict (`SendError::Pipeline`), not the
/// producer apparatus error.
fn write_transport_failure_fixture(dir: &Path) {
    std::fs::create_dir(dir.join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.join("routes/go-route.yaml"),
        r#"routes:
  - id: "go"
    from: "direct:go"
    steps:
      - set_body:
          value: "ok"
"#,
    )
    .expect("write route");
    std::fs::write(
        dir.join("job.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  send:
    to: "direct:go?block=true"
routeFiles:
  - routes/go-route.yaml
"#,
    )
    .expect("write job doc");
}

/// The captured stderr, labeled with stdout, for failure messages.
/// `Drained::markers` pairs `[stdout, stderr]`; locking the second
/// buffer yields the pure stderr stream.
fn stderr_of(drained: &common::Drained) -> String {
    drained.markers()[1]
        .lock()
        .expect("stderr buffer lock poisoned")
        .clone()
}

/// An ambiguous-target rejection — the earliest post-boot failure —
/// must still run the bounded context shutdown before the exit-2
/// return: stderr carries exactly one shutdown marker plus the existing
/// ambiguity diagnostic, and the child exits 2. Pre-fix behavior times
/// out here waiting for the marker (the child exits 2 without any
/// teardown).
#[test]
fn job_early_failure_shutdown_is_observed() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    write_ambiguous_target_fixture(dir.path());

    let mut child = spawn_camel_job_with_env(
        dir.path(),
        Path::new("job.job.yaml"),
        &[],
        &[("CAMEL_JOB_SHUTDOWN_MARKER", "1")],
    );
    let drained = spawn_drained(&mut child);

    let observed = wait_for_marker(&mut child, &drained.markers(), SHUTDOWN_MARKER, WAIT);
    let stderr = stderr_of(&drained);
    let output = drained.finish();
    assert!(
        observed,
        "the early failure must shut the booted context down before \
         exiting (marker never observed);\n{output}"
    );
    assert_eq!(
        stderr.matches(SHUTDOWN_MARKER).count(),
        1,
        "exactly one shutdown marker expected;\n{output}"
    );
    assert!(
        stderr.contains("is ambiguous: 2 consumer routes share its base"),
        "the existing ambiguity diagnostic must be preserved;\n{output}"
    );

    let code = wait_exit_code_bounded(&mut child, WAIT);
    assert_eq!(
        code, 2,
        "the early failure must keep its exit-2 outcome; -1 means the \
         process died by signal;\n{output}"
    );
}

/// A transport-class send failure (persistent outer producer-apparatus
/// error after the retry window) reaches the existing transport branch,
/// which must keep performing exactly one shutdown before exit 2 —
/// the refactor must not add a second teardown to the paths that
/// already shut down.
#[test]
fn job_transport_failure_shutdown_is_observed_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(dir.path());
    write_transport_failure_fixture(dir.path());

    let mut child = spawn_camel_job_with_env(
        dir.path(),
        Path::new("job.job.yaml"),
        &[],
        &[("CAMEL_JOB_SHUTDOWN_MARKER", "1")],
    );
    let drained = spawn_drained(&mut child);

    let observed = wait_for_marker(&mut child, &drained.markers(), SHUTDOWN_MARKER, WAIT);
    let stderr = stderr_of(&drained);
    let output = drained.finish();
    assert!(
        observed,
        "the transport failure must shut the booted context down before \
         exiting (marker never observed);\n{output}"
    );
    assert_eq!(
        stderr.matches(SHUTDOWN_MARKER).count(),
        1,
        "exactly one shutdown marker expected on the transport path;\n{output}"
    );
    assert!(
        stderr.contains("failed to create endpoint"),
        "the stable transport diagnostic must name the endpoint \
         creation failure;\n{output}"
    );
    assert!(
        stderr.contains("direct:go?block=true"),
        "the transport diagnostic must name the rejected send target;\n{output}"
    );

    let code = wait_exit_code_bounded(&mut child, WAIT);
    assert_eq!(
        code, 2,
        "the transport failure must exit 2; -1 means the process died \
         by signal;\n{output}"
    );
}
