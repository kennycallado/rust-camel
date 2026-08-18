//! End-to-end integration test for the `camel run` empty-discovery warning.
//!
//! When the route discovery patterns match zero files (e.g. the operator has
//! `routes.yaml` at the workspace root while the default pattern is
//! `routes/*.yaml`), `camel run` must emit a WARN naming the patterns before
//! starting with no routes — otherwise the process starts silently, no server
//! opens, and the operator concludes the feature is broken (bd rc-1110).
//!
//! The harness mirrors `run_exec_guard_test.rs`: spawn the real `camel`
//! binary with piped stdout/stderr, poll the captured buffer for the
//! observation, then send exactly one SIGTERM.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn drain_to_buffer<R: Read + Send + 'static>(mut reader: R, buffer: Arc<Mutex<String>>) {
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => {
                let text = String::from_utf8_lossy(&chunk[..n]);
                let mut guard = buffer.lock().expect("buffer lock poisoned");
                guard.push_str(&text);
            }
            Err(e) => {
                eprintln!("reader thread io error: {e}");
                return;
            }
        }
    }
}

/// Launch `camel run --no-watch` against `dir`'s `Camel.toml` with piped
/// output. The working dir is `dir` so the default discovery pattern
/// `routes/*.yaml` resolves relative to the fixture root.
fn spawn_camel_run(dir: &Path) -> Child {
    let config_path = dir.join("Camel.toml");
    Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("run")
        .arg("--no-watch")
        .arg("--config")
        .arg(&config_path)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .expect("failed to spawn `camel` binary")
}

/// Run `camel run` against `dir`, poll the captured output until it contains
/// `observe` (capped at `timeout`), then send exactly ONE SIGTERM and wait
/// for a graceful exit (10 s ceiling, force-kill past it). Returns
/// `(exit_code, captured_output, observed)`.
fn run_observe_then_signal(dir: &Path, observe: &str, timeout: Duration) -> (i32, String, bool) {
    let mut child = spawn_camel_run(dir);
    let buffer: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    let stdout = child
        .stdout
        .take()
        .expect("child stdout was configured as piped");
    let stderr = child
        .stderr
        .take()
        .expect("child stderr was configured as piped");

    let out_buf = Arc::clone(&buffer);
    let err_buf = Arc::clone(&buffer);
    let out_handle = thread::spawn(move || drain_to_buffer(stdout, out_buf));
    let err_handle = thread::spawn(move || drain_to_buffer(stderr, err_buf));

    let start = Instant::now();
    let step = Duration::from_millis(25);
    let observed = loop {
        {
            let guard = buffer.lock().expect("buffer lock poisoned");
            if guard.contains(observe) {
                break true;
            }
        }
        if start.elapsed() >= timeout {
            break false;
        }
        if let Ok(Some(_)) = child.try_wait() {
            break false;
        }
        thread::sleep(step);
    };

    if observed {
        // Send exactly ONE SIGTERM. The CLI's `tokio::select!` arm handles
        // SIGTERM → graceful shutdown → `Ok(())` → exit 0. A second signal
        // would force `exit(1)`, so we must not double-tap.
        let kill_status = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status()
            .expect("failed to spawn `kill -TERM`");
        assert!(
            kill_status.success(),
            "`kill -TERM` returned non-zero: {kill_status:?}"
        );
    }

    // Bounded graceful-exit wait.
    let exit_code = 'wait: loop {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break 'wait status.code().unwrap_or(-1),
                Ok(None) => {
                    if start.elapsed() >= Duration::from_secs(10) + timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        break 'wait -1;
                    }
                    thread::sleep(step);
                }
                Err(e) => panic!("try_wait failed: {e}"),
            }
        }
    };

    let _ = out_handle.join();
    let _ = err_handle.join();
    let captured = buffer.lock().expect("buffer lock poisoned").clone();
    (exit_code, captured, observed)
}

/// A discovery pattern that matches zero files (no `routes/` directory is
/// created) must produce a WARN naming the patterns, and the process must
/// still start and shut down gracefully (exit 0).
#[test]
fn empty_discovery_emits_warn_and_starts() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Camel.toml"),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "INFO"
watch = false
"#,
    )
    .expect("write Camel.toml");
    // Intentionally NO routes/ directory: the glob matches nothing.

    let (exit_code, output, observed) = run_observe_then_signal(
        dir.path(),
        "matched zero route files",
        Duration::from_secs(30),
    );

    assert!(
        observed,
        "expected a WARN containing `matched zero route files` when the \
         discovery glob matches nothing; got:\n{output}"
    );
    assert!(
        output.contains("routes/*.yaml"),
        "expected the WARN to name the discovery patterns; got:\n{output}"
    );
    assert_eq!(
        exit_code, 0,
        "expected graceful shutdown (exit 0) after the WARN; got {exit_code}\n--- captured ---\n{output}\n--- end ---"
    );
}
