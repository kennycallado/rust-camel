//! End-to-end integration tests for the `camel run` signal contract.
//!
//! Regression coverage for the boot-window race fixed by rc-z5zch (SIGTERM)
//! and its SIGINT twin rc-ukwlt: both signal streams are armed at the very
//! start of `run`, BEFORE boot. A signal arriving while config load, the
//! bundle cascade, discovery, or `ctx.start()` still run used to hit the
//! default disposition and kill the process; the entry-registered streams
//! buffer it instead, and the shutdown select consumes it as soon as it
//! awaits, so the run finishes boot and then shuts down gracefully.
//!
//! The harness mirrors `run_empty_discovery_test.rs`: spawn the real `camel`
//! binary with piped stdout/stderr, poll the captured buffer for a marker,
//! then send signals with `kill`. The mid-boot marker is the CWD-trust WARN:
//! it is flushed AFTER the signal handlers are armed (step 0) and while boot
//! is still in flight (bundle cascade, discovery, context start all follow),
//! so a signal sent on it lands inside the covered boot stretch with zero
//! risk of racing the handler arming itself. The shared subprocess plumbing
//! lives in `tests/common`.

mod common;

use std::path::Path;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::{send_signal, spawn_camel_run, spawn_drained, wait_exit_code_bounded};

/// Write the zero-routes fixture: a `Camel.toml` whose glob matches nothing
/// (no `routes/` directory is created) so boot keeps running past discovery
/// with no routes, exactly like `run_empty_discovery_test.rs`.
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
}

/// Poll `buffers` until `marker` appears on any of them, the child dies on
/// its own, or `timeout` elapses. Same contract as `common::wait_for_marker`
/// but with a 5 ms step: the signal tests must observe the boot marker while
/// boot is still in flight, so marker-observation staleness has to stay
/// well under the boot stretch, not just under the process lifetime.
fn wait_for_marker_tight(
    child: &mut Child,
    buffers: &[Arc<Mutex<String>>],
    marker: &str,
    timeout: Duration,
) -> bool {
    let start = Instant::now();
    let step = Duration::from_millis(5);
    loop {
        if buffers
            .iter()
            .any(|buf| buf.lock().expect("buffer lock poisoned").contains(marker))
        {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        if let Ok(Some(_)) = child.try_wait() {
            return false;
        }
        thread::sleep(step);
    }
}

/// rc-ukwlt: a SIGINT arriving mid-boot must NOT default-kill the process.
/// The entry-registered SIGINT stream buffers it; the shutdown select
/// consumes it once boot completes, and the run exits gracefully with 0.
#[test]
fn sigint_during_boot_shuts_down_gracefully() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());

    let mut child = spawn_camel_run(dir.path());
    let drained = spawn_drained(&mut child);

    // Earliest mid-boot marker that proves the step-0 handlers are armed:
    // the CWD-trust WARN prints right after the context is configured and
    // BEFORE the component bundle cascade, discovery, and ctx.start(), the
    // whole stretch the shutdown select only arms after. The 5 ms poll keeps
    // marker-observation staleness small so the signal lands inside that
    // stretch rather than past it.
    let booting = wait_for_marker_tight(
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

    // Exactly ONE SIGINT inside the boot window.
    send_signal(&child, "-INT");

    let exit_code = wait_exit_code_bounded(&mut child, Duration::from_secs(30));
    let output = drained.finish();

    assert_eq!(
        exit_code, 0,
        "expected graceful shutdown (exit 0) after a mid-boot SIGINT; \
         a default-disposition kill would surface as -1;\n{output}\n--- end ---"
    );
    assert!(
        output.contains("Received Ctrl+C"),
        "expected the shutdown select to consume the buffered SIGINT \
         (missing `Received Ctrl+C`);\n{output}\n--- end ---"
    );
}

/// rc-kz85m: a second stop signal must force-exit the run with code 1.
///
/// The test sends an INT and a TERM pair while boot is still in flight,
/// after the step-0 handlers are armed. Each signal type buffers in its own
/// stream permit slot (tokio coalesces repeats of the SAME signal, so a
/// same-signal double-tap would deliver once and be lost): the shutdown
/// select consumes whichever buffered signal resolves first, and the
/// leftover permit in the other stream makes the teardown force-exit select
/// ready on its first poll, so the process exits 1 via `std::process::exit`
/// (a default-disposition kill would surface as -1, a missing force-exit
/// arm as the graceful 0). Which of the pair the shutdown select consumes is
/// tokio's random pick among ready arms, so the test asserts the exit code
/// and the `forcing exit` WARN, not which signal logged it.
///
/// Honesty note: this exercises the second-signal WINDOW (the force-exit
/// select is armed and a second signal is consumable during teardown), not
/// a deterministically HUNG teardown — the harness has no hook to stall
/// `BootHandle::shutdown`.
#[test]
fn second_sigterm_during_teardown_force_exits() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());

    let mut child = spawn_camel_run(dir.path());
    let drained = spawn_drained(&mut child);

    // Same mid-boot marker as the rc-ukwlt test: handlers armed, boot in
    // flight. Sending here means both signals buffer before the shutdown
    // select arms, so one is consumed as the graceful first signal and the
    // other is the consumable second signal during teardown.
    let booting = wait_for_marker_tight(
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

    // The escape-hatch pair: systemd / docker stop resend the stop signal
    // after the grace period — modeled by the TERM; the INT is the paired
    // second signal that survives coalescing. One `sh -c` sends both so the
    // pair lands well before the shutdown select arms (`kill` is a shell
    // builtin; two separate spawn(2)s would leave a multi-ms exec gap that
    // can push the second signal past teardown under load).
    let pair = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "kill -INT {pid}; kill -TERM {pid}",
            pid = child.id()
        ))
        .status()
        .expect("failed to spawn signal pair");
    assert!(pair.success(), "signal pair returned non-zero: {pair:?}");

    let exit_code = wait_exit_code_bounded(&mut child, Duration::from_secs(30));
    let output = drained.finish();

    assert_eq!(
        exit_code, 1,
        "expected the second stop signal to force-exit with code 1; exit 0 \
         means the force-exit arm never fired, -1 means a \
         default-disposition kill;\n{output}\n--- end ---"
    );
    assert!(
        output.contains("forcing exit"),
        "expected a `Second ... — forcing exit` WARN;\n{output}\n--- end ---"
    );
    assert!(
        output.contains("Received Ctrl+C") || output.contains("Received SIGTERM"),
        "expected the graceful first-signal log before the force exit;\
         \n{output}\n--- end ---"
    );
}
