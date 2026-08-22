//! Integration test for the watch hot path over colocated camel test docs.
//!
//! Regression for the former crash path: a `Camel.toml` with EXPLICIT
//! `routes = ["routes/*.yaml"]` plus `watch = true` used to re-glob and parse
//! `*.test.yaml` sidecars as routes on every reload pass, killing or erroring
//! the run the moment a test doc was saved. Since camel-dsl discovery took
//! ownership of the reserved suffix, wildcard globs skip test documents on
//! every pass (initial load AND reload), so a test-doc save is a no-op.
//!
//! The test spawns the real `camel` binary (via `CARGO_BIN_EXE_camel`)
//! against a fresh `tempfile::TempDir` fixture, waits until the file watcher
//! reports itself armed, rewrites the colocated test doc (identical body plus
//! one trailing newline), and asserts the run is still alive with no error
//! naming the test doc. Design notes mirror `run_exec_guard_test.rs`:
//!
//! - stdout/stderr are piped and drained by two reader threads into separate
//!   buffers so the OS pipe (64 KiB) never fills and deadlocks the child.
//! - The child is wrapped in a kill-on-drop guard: a failed assertion mid-test
//!   cannot leak the process.
//! - Exactly ONE SIGTERM terminates the run; the CLI's `tokio::select!` arm
//!   handles it gracefully (a second signal would force `exit(1)`).
//! - The post-save sleep is 2 s against the default `watch_debounce_ms` of
//!   300 ms: comfortably past the debounce window plus the reload pass.

use std::io::Read;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Drain `reader` (a pipe from a child process) into `buffer` until EOF.
/// Designed to be called inside a `std::thread::spawn` closure.
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

/// Wrapper that force-kills the child on drop so a failed assertion mid-test
/// cannot leak the process. After the child has been reaped, `Child::kill` is
/// a no-op (std refuses to signal a possibly-recycled pid), so the guard is
/// harmless on the normal exit path.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // Best-effort cleanup: ignore errors (child may have exited already).
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Deref for KillOnDrop {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

/// Build the `Command` that launches the `camel` binary against `dir`'s
/// `Camel.toml`, with both stdout and stderr piped. No `--no-watch` flag:
/// watching comes from `watch = true` in the fixture config (the exact
/// former-crash configuration).
fn spawn_camel_run_watch(dir: &Path) -> KillOnDrop {
    let config_path = dir.join("Camel.toml");
    let child = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("run")
        .arg("--config")
        .arg(&config_path)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .expect("failed to spawn `camel` binary");
    KillOnDrop(child)
}

/// Poll both captured buffers until `marker` appears on either stream, the
/// child dies, or `timeout` elapses. Returns `false` when the child died or
/// the deadline hit; callers assert and print the captured output.
fn wait_for_marker(
    child: &mut Child,
    out_buf: &Arc<Mutex<String>>,
    err_buf: &Arc<Mutex<String>>,
    marker: &str,
    timeout: Duration,
) -> bool {
    let start = Instant::now();
    let step = Duration::from_millis(25);
    loop {
        {
            let out = out_buf.lock().expect("stdout buffer lock poisoned");
            let err = err_buf.lock().expect("stderr buffer lock poisoned");
            if out.contains(marker) || err.contains(marker) {
                return true;
            }
        }
        if start.elapsed() >= timeout {
            return false;
        }
        // Bail out early if the child already died: the marker will never
        // arrive and the caller's assert should show what was captured.
        if let Ok(Some(_)) = child.try_wait() {
            return false;
        }
        thread::sleep(step);
    }
}

/// Wait for the child to exit, but at most `timeout`. If it is still alive
/// at the deadline, force-kill and reap, returning `false`.
fn wait_exit_bounded(child: &mut Child, timeout: Duration) -> bool {
    let start = Instant::now();
    let step = Duration::from_millis(25);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                thread::sleep(step);
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }
}

/// Write `Camel.toml` to `dir/Camel.toml`.
fn write_camel_toml(dir: &Path, body: &str) {
    std::fs::write(dir.join("Camel.toml"), body).expect("write Camel.toml");
}

/// Write `routes/<name>` to `dir/routes/<name>` (creating `routes/` as
/// needed). Used for both the route file and the colocated test doc.
fn write_routes_file(dir: &Path, name: &str, body: &str) {
    let routes_dir = dir.join("routes");
    std::fs::create_dir_all(&routes_dir).expect("create routes/");
    std::fs::write(routes_dir.join(name), body).expect("write routes file");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Saving a colocated `*.test.yaml` document while `camel run` watches with
/// EXPLICIT `Camel.toml` routes must be a no-op: the reload pass re-globs
/// through discovery, which skips the reserved suffix, so the run stays alive
/// and no error names the test doc.
#[test]
fn watch_reload_test_doc_save_is_noop() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Explicit config routes + watch enabled: the former crash path.
    write_camel_toml(
        dir.path(),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "INFO"
watch = true
"#,
    );
    write_routes_file(
        dir.path(),
        "demo.yaml",
        r#"routes:
  - id: "demo"
    from: "direct:start"
    steps:
      - to: "mock:result"
"#,
    );
    // Minimal valid test doc colocated with the route it tests.
    let test_doc_body =
        "routeFiles:\n  - demo.yaml\ninputs: []\nexpects:\n  mock:result:\n    count: 1\n";
    write_routes_file(dir.path(), "demo.test.yaml", test_doc_body);

    let mut child = spawn_camel_run_watch(dir.path());
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

    let out_thread_buf = Arc::clone(&out_buf);
    let err_thread_buf = Arc::clone(&err_buf);
    let out_handle = thread::spawn(move || drain_to_buffer(stdout, out_thread_buf));
    let err_handle = thread::spawn(move || drain_to_buffer(stderr, err_thread_buf));

    // Wait until the watcher is truly armed. The core watcher logs
    // "hot-reload: watching <dir>" only AFTER `watcher.watch()` succeeded,
    // which is a stronger signal than the CLI banner (that races the spawned
    // watcher task and can print before the watch handles are registered).
    let armed = wait_for_marker(
        &mut child,
        &out_buf,
        &err_buf,
        "hot-reload: watching",
        Duration::from_secs(10),
    );
    let captured_so_far = || {
        format!(
            "stdout:\n{}\nstderr:\n{}",
            out_buf.lock().expect("stdout buffer lock poisoned"),
            err_buf.lock().expect("stderr buffer lock poisoned")
        )
    };
    assert!(
        armed,
        "file watcher did not report itself armed within 10 s;\n{}",
        captured_so_far()
    );

    // Save the test doc: identical body plus one trailing newline. Touch
    // semantics without truncating to empty first, so the watcher sees a
    // plain modify event for a `.test.yaml` file.
    std::fs::write(
        dir.path().join("routes/demo.test.yaml"),
        format!("{test_doc_body}\n"),
    )
    .expect("rewrite demo.test.yaml");

    // Sleep past the debounce window (default watch_debounce_ms = 300 ms) so
    // the reload pass has settled before we probe liveness.
    thread::sleep(Duration::from_secs(2));

    // The run must still be alive: the reload pass re-globbed, discovery
    // skipped the test doc, and no route changed.
    let status = child.try_wait().expect("try_wait after test-doc save");
    assert!(
        status.is_none(),
        "camel run died after the colocated test-doc save (status: {status:?}); \
         the reload pass must skip *.test.yaml, not parse it;\n{}",
        captured_so_far()
    );

    // Send exactly ONE SIGTERM; the CLI handles it as graceful shutdown.
    let kill_status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("failed to spawn `kill -TERM`");
    assert!(
        kill_status.success(),
        "`kill -TERM` returned non-zero: {kill_status:?}"
    );

    let exited = wait_exit_bounded(&mut child, Duration::from_secs(10));

    let _ = out_handle.join();
    let _ = err_handle.join();

    assert!(
        exited,
        "camel run did not exit within 10 s after SIGTERM;\n{}",
        captured_so_far()
    );

    // No discovery/parse error may name the test doc. The watcher never logs
    // file names on happy paths (change/reload log lines carry no paths), so
    // plain containment on both captured streams is precise: on the former
    // crash path the discovery error text names demo.test.yaml.
    let stdout_text = out_buf.lock().expect("stdout buffer lock poisoned").clone();
    let stderr_text = err_buf.lock().expect("stderr buffer lock poisoned").clone();
    assert!(
        !stderr_text.contains("demo.test.yaml"),
        "stderr names the colocated test doc; the reload pass must skip it:\n{stderr_text}"
    );
    assert!(
        !stdout_text.contains("demo.test.yaml"),
        "stdout names the colocated test doc; the reload pass must skip it:\n{stdout_text}"
    );
}
