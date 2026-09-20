//! End-to-end regression guard for startup log quietness (mission 149,
//! bd rc-k56el): a minimal `camel run` boot must emit ZERO WARN-level
//! lines. The CWD-trust disclosure is a benign dev-tool trust-model note
//! (ADR-0012/0037/0052/0075 territory; e_opus ruling
//! `bd rc-k56el` Q3), so it logs at INFO,
//! and the PART A commit already quieted the alias-scheme warns — the
//! trust note was the only remaining startup WARN this test guards.
//!
//! The harness mirrors `run_signal_test.rs`: spawn the real `camel`
//! binary with piped stdout/stderr (`common::spawn_camel_run`), drain
//! both pipes (`common::spawn_drained`), wait for the trust note as the
//! boot marker (proving the fmt subscriber renders levels to stderr),
//! then shut down gracefully with exactly ONE SIGINT and assert the full
//! capture contains no WARN-level line. The tracing fmt layer renders
//! levels as uppercase tokens (`WARN`), which is what the assertion
//! scans for.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{
    send_signal, spawn_camel_run, spawn_drained, wait_exit_code_bounded, wait_for_marker,
};

/// Minimal boot fixture: one trivial no-op route (`direct:start` →
/// `mock:result`, both in the ADR-0064 lean set) so discovery matches a
/// file — a zero-route glob would itself emit the `matched zero route
/// files` WARN and defeat the assertion for the wrong reason.
/// `log_level = "INFO"` keeps the (INFO-level) trust note visible as the
/// boot marker; `watch = false` makes the run a single-shot process that
/// waits for the stop signal.
fn write_fixture(dir: &Path) {
    std::fs::write(
        dir.join("Camel.toml"),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "INFO"
watch = false
"#,
    )
    .expect("write Camel.toml");
    let routes = dir.join("routes");
    std::fs::create_dir(&routes).expect("mkdir routes/");
    std::fs::write(
        routes.join("noop.yaml"),
        r#"routes:
  - id: "noop"
    from: "direct:start"
    steps:
      - to: "mock:result"
"#,
    )
    .expect("write routes/noop.yaml");
}

/// A minimal `camel run` boot must emit ZERO WARN-level lines: startup
/// notices that are trust-model disclosures rather than
/// misconfiguration signals log at INFO.
#[test]
fn run_boot_emits_no_warn_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());

    let mut child = spawn_camel_run(dir.path());
    let drained = spawn_drained(&mut child);

    // Boot marker: the CWD-trust note proves the fmt subscriber is live
    // and rendering levels to stderr, and that boot got past context
    // configure — the same placement the signal tests synchronize on.
    let booting = wait_for_marker(
        &mut child,
        &drained.markers(),
        "trusts the current working directory",
        Duration::from_secs(30),
    );
    assert!(
        booting,
        "camel run never reached mid-boot;\n{}",
        drained.captured()
    );

    // Exactly ONE SIGINT: graceful shutdown (exit 0), so every buffered
    // startup line is flushed before the pipes hit EOF.
    send_signal(&child, "-INT");

    let exit_code = wait_exit_code_bounded(&mut child, Duration::from_secs(30));
    let output = drained.finish();

    assert_eq!(
        exit_code, 0,
        "expected graceful shutdown (exit 0);\n{output}\n--- end ---"
    );
    let warn_lines: Vec<&str> = output.lines().filter(|l| l.contains("WARN")).collect();
    assert!(
        warn_lines.is_empty(),
        "minimal `camel run` boot emitted WARN-level lines: {warn_lines:?};\n{output}\n--- end ---"
    );
}
