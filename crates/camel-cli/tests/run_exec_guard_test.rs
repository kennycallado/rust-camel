//! End-to-end integration tests for the `camel run` exec startup-guard.
//!
//! These tests spawn the actual `camel` binary (via `CARGO_BIN_EXE_camel`)
//! against a fresh `tempfile::TempDir` fixture per test. They validate the
//! four scenarios in the `exec-cli-startup-guard` proposal:
//!
//! 1. Non-exec route starts when no exec is configured.
//! 2. Exec route with no profiles aborts (fail-closed).
//! 3. Explicit empty `[components.exec]` aborts even without a route
//!    referencing `exec:`.
//! 4. Exec route with a valid profile starts.
//!
//! Design notes:
//!
//! - The binary is launched with `--no-watch` so it does not spawn a hot-reload
//!   file watcher (which would keep the process alive on its own).
//! - `stdout` and `stderr` are both captured via `Stdio::piped()`. Without
//!   concurrent drain, the OS pipe buffer fills (64 KiB on Linux) and the
//!   child blocks on its next write — deadlock. We spawn two reader threads
//!   that append into a single shared `Arc<Mutex<String>>` buffer. The main
//!   thread polls `child.try_wait()` and the buffer; both reader threads are
//!   always joined before the helper returns so pipes are drained.
//! - Success cases send exactly ONE SIGTERM. The CLI's `tokio::select!` arm
//!   handles SIGTERM → graceful shutdown → `Ok(())` → exit 0. A second signal
//!   would force `exit(1)`, so we must not double-tap.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Drain `reader` (a pipe from a child process) into `buffer` line by line
/// until EOF. Designed to be called inside a `std::thread::spawn` closure.
fn drain_to_buffer<R: Read + Send + 'static>(mut reader: R, buffer: Arc<Mutex<String>>) {
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return, // EOF
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

/// Build the `Command` that launches the `camel` binary against `dir`'s
/// `Camel.toml`, with both stdout and stderr piped. The working dir is set
/// to `dir` so that `camel run` resolves `routes = ["routes/*.yaml"]` and
/// `[components.exec].workspace_root = "."` relative to the fixture root.
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

/// Poll the child for up to `timeout`, returning the exit code if it
/// terminated on its own. Returns `None` if still alive at the deadline.
fn try_wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<i32> {
    let start = Instant::now();
    let step = Duration::from_millis(25);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    return None;
                }
                thread::sleep(step);
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }
}

/// Wait for the child to exit, but at most `timeout`. If the child is still
/// alive at the deadline, send SIGKILL and reap. Returns the exit code, or
/// `-1` if the process had to be force-killed.
fn wait_bounded(child: &mut Child, timeout: Duration) -> i32 {
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

/// Run `camel run` against `dir` and wait for it to exit on its own,
/// bounded by `timeout`. If it does not exit in time, kill it and return
/// `-1`. Drains stdout/stderr into the returned buffer.
fn run_expect_exit(dir: &Path, timeout: Duration) -> (i32, String) {
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

    let exit_code = try_wait_with_timeout(&mut child, timeout).unwrap_or_else(|| {
        // Still alive at the timeout — kill + reap, then propagate -1.
        let _ = child.kill();
        let _ = child.wait();
        -1
    });

    let _ = out_handle.join();
    let _ = err_handle.join();

    let captured = buffer.lock().expect("buffer lock poisoned").clone();
    (exit_code, captured)
}

/// Run `camel run` against `dir`, poll the captured output until it contains
/// `observe` (capped at `timeout`), then send exactly ONE SIGTERM. Wait for
/// graceful exit with a 10 s ceiling; force-kill on deadline. Returns
/// `(exit_code, captured_output)`.
fn run_observe_then_signal(dir: &Path, observe: &str, timeout: Duration) -> (i32, String) {
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

    // Poll the buffer for the expected observation.
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
        // Also bail out early if the child has already died — no point in
        // waiting for an observation that will never come.
        if let Ok(Some(_)) = child.try_wait() {
            break false;
        }
        thread::sleep(step);
    };

    assert!(
        observed,
        "did not observe {observe:?} within {timeout:?}; captured so far:\n{}",
        buffer.lock().expect("buffer lock poisoned")
    );

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

    let exit_code = wait_bounded(&mut child, Duration::from_secs(10));

    let _ = out_handle.join();
    let _ = err_handle.join();

    let captured = buffer.lock().expect("buffer lock poisoned").clone();
    (exit_code, captured)
}

/// Write `Camel.toml` to `dir/Camel.toml`.
fn write_camel_toml(dir: &Path, body: &str) {
    std::fs::write(dir.join("Camel.toml"), body).expect("write Camel.toml");
}

/// Write `routes/<name>.yaml` to `dir/routes/<name>.yaml` (creating
/// `routes/` as needed).
fn write_route(dir: &Path, name: &str, body: &str) {
    let routes_dir = dir.join("routes");
    std::fs::create_dir_all(&routes_dir).expect("create routes/");
    std::fs::write(routes_dir.join(name), body).expect("write route yaml");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A `timer -> log` route with no `[components.exec]` block starts, processes
/// exchanges, and shuts down gracefully. Regression for the original bug
/// where default-features `camel run` aborted any non-exec route.
#[test]
fn non_exec_route_starts_without_exec_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_camel_toml(
        dir.path(),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "INFO"
watch = false
"#,
    );
    write_route(
        dir.path(),
        "hello.yaml",
        r#"routes:
  - id: "hello"
    from: "timer:tick?period=300"
    steps:
      - log: "non-exec-tick-ok"
"#,
    );

    let (exit_code, output) =
        run_observe_then_signal(dir.path(), "non-exec-tick-ok", Duration::from_secs(4));

    assert_eq!(
        exit_code, 0,
        "expected graceful shutdown (exit 0) for non-exec route; \
         got exit code {exit_code}\n--- captured output ---\n{output}\n--- end ---"
    );
    assert!(
        output.contains("non-exec-tick-ok"),
        "expected captured output to contain `non-exec-tick-ok`; got:\n{output}"
    );
    assert!(
        !output.contains("no profiles configured"),
        "did not expect fail-closed exec error for non-exec route; got:\n{output}"
    );
}

/// A route that references `exec:` but has no configured profiles aborts
/// startup with the fail-closed error. This is the ADR-0033 invariant:
/// declaring exec usage without a profile is a configuration error.
#[test]
fn exec_route_without_profiles_aborts() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_camel_toml(
        dir.path(),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "INFO"
watch = false
"#,
    );
    write_route(
        dir.path(),
        "exec.yaml",
        r#"routes:
  - id: "exec"
    from: "timer:tick?period=300"
    steps:
      - to: "exec:echo"
"#,
    );

    let (exit_code, output) = run_expect_exit(dir.path(), Duration::from_secs(4));

    assert_ne!(
        exit_code, 0,
        "expected non-zero exit for exec route without profiles; got 0.\n--- captured output ---\n{output}\n--- end ---"
    );
    assert!(
        exit_code != -1,
        "expected the CLI to abort on its own, but it was force-killed at the timeout (hung instead of aborting)"
    );
    assert!(
        output.contains("no profiles configured"),
        "expected fail-closed `no profiles configured` error; got:\n{output}"
    );
}

/// An explicit `[default.components.exec]` block with zero profiles aborts
/// startup even when no route references `exec:`. The operator declared exec
/// intent; we honour it by failing closed.
#[test]
fn explicit_empty_exec_config_aborts() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_camel_toml(
        dir.path(),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "INFO"
watch = false

[default.components.exec]
workspace_root = "."
"#,
    );
    write_route(
        dir.path(),
        "hello.yaml",
        r#"routes:
  - id: "hello"
    from: "timer:tick?period=300"
    steps:
      - log: "non-exec-tick-ok"
"#,
    );

    let (exit_code, output) = run_expect_exit(dir.path(), Duration::from_secs(4));

    assert_ne!(
        exit_code, 0,
        "expected non-zero exit for empty exec config; got 0.\n--- captured output ---\n{output}\n--- end ---"
    );
    assert!(
        exit_code != -1,
        "expected the CLI to abort on its own, but it was force-killed at the timeout (hung instead of aborting)"
    );
    assert!(
        output.contains("no profiles configured"),
        "expected fail-closed `no profiles configured` error; got:\n{output}"
    );
}

/// A route that references `exec:` and a fully-defined profile starts and
/// reaches the "context started" state. This is the positive control: when
/// exec is both used and configured, startup proceeds.
#[test]
fn exec_route_with_profile_starts() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_camel_toml(
        dir.path(),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "INFO"
watch = false

[default.components.exec]
workspace_root = "."

[[default.components.exec.profiles]]
name = "echo"
executable = "echo"
args = { allow = "any" }
timeout_secs = 5
accepted_exit_codes = [0]
"#,
    );
    write_route(
        dir.path(),
        "exec.yaml",
        r#"routes:
  - id: "exec"
    from: "timer:tick?period=300"
    steps:
      - to: "exec:echo"
"#,
    );

    let (exit_code, output) =
        run_observe_then_signal(dir.path(), "context started", Duration::from_secs(4));

    assert_eq!(
        exit_code, 0,
        "expected graceful shutdown (exit 0) for valid exec profile; \
         got exit code {exit_code}\n--- captured output ---\n{output}\n--- end ---"
    );
    assert!(
        output.contains("context started"),
        "expected captured output to contain `context started`; got:\n{output}"
    );
    assert!(
        !output.contains("no profiles configured"),
        "did not expect fail-closed exec error with valid profile; got:\n{output}"
    );
}
