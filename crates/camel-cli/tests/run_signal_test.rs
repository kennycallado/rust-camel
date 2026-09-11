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

use common::{drain_to_buffer, send_signal, spawn_camel_run};

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

/// Pipe-drained capture for the child's output streams.
struct Drained {
    out_handle: thread::JoinHandle<()>,
    err_handle: thread::JoinHandle<()>,
    out_buf: Arc<Mutex<String>>,
    err_buf: Arc<Mutex<String>>,
}

impl Drained {
    /// Both captured streams, labeled, for failure messages.
    fn captured(&self) -> String {
        format!(
            "stdout:\n{}\nstderr:\n{}",
            self.out_buf.lock().expect("stdout buffer lock poisoned"),
            self.err_buf.lock().expect("stderr buffer lock poisoned")
        )
    }
}

/// Take the child's piped stdout/stderr and spawn a drain thread per stream
/// so the OS pipe buffer (64 KiB) never fills and deadlocks the child.
fn spawn_drained(child: &mut Child) -> Drained {
    let out_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let err_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let stdout = child
        .stdout
        .take()
        .expect("child stdout was configured as piped");
    let stderr = child
        .stderr
        .take()
        .expect("child stderr was configured as piped");
    let out_handle = thread::spawn({
        let buf = Arc::clone(&out_buf);
        move || drain_to_buffer(stdout, buf)
    });
    let err_handle = thread::spawn({
        let buf = Arc::clone(&err_buf);
        move || drain_to_buffer(stderr, buf)
    });
    Drained {
        out_handle,
        err_handle,
        out_buf,
        err_buf,
    }
}

/// Wait for the child to exit, at most `timeout`. If it is still alive at
/// the deadline, force-kill and reap, returning `-1` (same sentinel as
/// `run_exec_guard_test.rs`).
fn wait_exit_code_bounded(child: &mut Child, timeout: Duration) -> i32 {
    let start = Instant::now();
    let step = Duration::from_millis(25);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code().unwrap_or(-1),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return -1;
                }
                thread::sleep(step);
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }
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
        &[Arc::clone(&drained.out_buf), Arc::clone(&drained.err_buf)],
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
    let Drained {
        out_handle,
        err_handle,
        out_buf,
        err_buf,
    } = drained;
    let _ = out_handle.join();
    let _ = err_handle.join();
    let output = format!(
        "stdout:\n{}\nstderr:\n{}",
        out_buf.lock().expect("stdout buffer lock poisoned"),
        err_buf.lock().expect("stderr buffer lock poisoned")
    );

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
