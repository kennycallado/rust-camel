//! Shared subprocess plumbing for the `camel-cli` integration tests that
//! spawn the real `camel` binary: pipe drainers, a drained-capture wrapper,
//! a kill-on-drop child guard, a bounded marker wait, bounded exit waits
//! (bool and exit-code), a single-SIGTERM sender, and the standard
//! `camel run`/`camel job` spawns. Observation and exit deadlines here are
//! deliberately generous (30 s): under a whole-workspace `cargo test` run the
//! OS is saturated by hundreds of peer processes and subprocess startup slows
//! roughly 100x, while the happy path still returns the moment the marker is
//! seen or the child exits, so the headroom never slows the fast path.

use std::io::Read;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Capture buffer written by a reader thread and polled by the test.
pub type SharedBuf = Arc<Mutex<String>>;

/// Drain `reader` (a pipe from a child process) into `buffer` until EOF.
/// Designed to be called inside a `std::thread::spawn` closure. Without a
/// concurrent drain the OS pipe buffer fills (64 KiB on Linux) and the child
/// blocks on its next write, deadlocking the test.
pub fn drain_to_buffer<R: Read + Send + 'static>(mut reader: R, buffer: SharedBuf) {
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

/// Pipe-drained capture for a child's piped output streams: one reader
/// thread per stream (`drain_to_buffer`) feeding a shared buffer the test
/// can poll for markers (`Drained::markers`) and print on failure
/// (`Drained::captured` / `Drained::finish`).
// Only the signal test binaries call it; the test binaries that include
// `common` without calling it would otherwise warn dead_code (each
// compilation unit gets its own copy of the module).
#[allow(dead_code)]
pub struct Drained {
    out_handle: thread::JoinHandle<()>,
    err_handle: thread::JoinHandle<()>,
    out_buf: SharedBuf,
    err_buf: SharedBuf,
}

// All methods are used only by the signal test binaries; see the struct
// comment.
#[allow(dead_code)]
impl Drained {
    /// Both captured streams, labeled, for failure messages.
    pub fn captured(&self) -> String {
        format!(
            "stdout:\n{}\nstderr:\n{}",
            self.out_buf.lock().expect("stdout buffer lock poisoned"),
            self.err_buf.lock().expect("stderr buffer lock poisoned")
        )
    }

    /// Join the drain threads (the child has exited, so both hit EOF)
    /// and return the labeled capture.
    pub fn finish(self) -> String {
        let Drained {
            out_handle,
            err_handle,
            out_buf,
            err_buf,
        } = self;
        let _ = out_handle.join();
        let _ = err_handle.join();
        format!(
            "stdout:\n{}\nstderr:\n{}",
            out_buf.lock().expect("stdout buffer lock poisoned"),
            err_buf.lock().expect("stderr buffer lock poisoned")
        )
    }

    /// Both buffers as a `wait_for_marker` slice.
    pub fn markers(&self) -> [SharedBuf; 2] {
        [Arc::clone(&self.out_buf), Arc::clone(&self.err_buf)]
    }
}

/// Take the child's piped stdout/stderr and spawn a drain thread per
/// stream so the OS pipe buffer (64 KiB) never fills and deadlocks the
/// child. The child stays kill-on-drop via the `KillOnDrop` returned by
/// the spawn helpers; the `Drained` guard only joins the readers.
// Shared by the signal test binaries; the test binaries that include
// `common` without calling it would otherwise warn dead_code (each
// compilation unit gets its own copy of the module).
#[allow(dead_code)]
pub fn spawn_drained(child: &mut Child) -> Drained {
    let out_buf: SharedBuf = Arc::new(Mutex::new(String::new()));
    let err_buf: SharedBuf = Arc::new(Mutex::new(String::new()));
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

/// Wrapper that force-kills the child on drop so a failed assertion mid-test
/// cannot leak the process. After the child has been reaped, `Child::kill` is
/// a no-op (std refuses to signal a possibly-recycled pid), so the guard is
/// harmless on the normal exit path.
pub struct KillOnDrop(pub Child);

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

/// Poll `buffers` until `marker` appears on any of them, the child dies on
/// its own, or `timeout` elapses. Returns `false` when the child died or the
/// deadline hit; callers assert and print the captured output.
///
/// Observation deadlines are 30 s, not a tight value: these tests spawn the
/// full ~283 MB `camel` binary, which initializes ~15 always-on component
/// bundles. Under a whole-workspace `cargo test` run the OS is saturated by
/// hundreds of peer processes and subprocess startup slows ~100x. The poll
/// short-circuits the moment the marker is seen, so the generous ceiling
/// only buys headroom under load; it never slows the fast path.
// Not every test binary that includes `common` calls it (each compilation
// unit gets its own copy of the module); mirror `spawn_camel_run`.
#[allow(dead_code)]
pub fn wait_for_marker(
    child: &mut Child,
    buffers: &[SharedBuf],
    marker: &str,
    timeout: Duration,
) -> bool {
    let start = Instant::now();
    let step = Duration::from_millis(25);
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
        // Bail out early if the child already died: the marker will never
        // arrive and the caller's assert should show what was captured.
        if let Ok(Some(_)) = child.try_wait() {
            return false;
        }
        thread::sleep(step);
    }
}

/// Send exactly ONE SIGTERM to the child. The CLI's `tokio::select!` arm
/// handles SIGTERM as a graceful shutdown that exits 0; a second signal
/// would force `exit(1)`, so callers must not double-tap.
// Not every test binary that includes `common` calls it (each compilation
// unit gets its own copy of the module); mirror `spawn_camel_run`.
#[allow(dead_code)]
pub fn send_term(child: &Child) {
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("failed to spawn `kill -TERM`");
    assert!(
        status.success(),
        "`kill -TERM` returned non-zero: {status:?}"
    );
}

/// Send exactly one signal to the child, named in `kill` syntax
/// (e.g. `-INT`, `-TERM`). Like `send_term`, one delivery is a graceful
/// shutdown; a second is the force-exit escape hatch (rc-kz85m).
// Shared by the run signal tests; the test binaries that include `common`
// without calling it would otherwise warn dead_code (each compilation unit
// gets its own copy of the module).
#[allow(dead_code)]
pub fn send_signal(child: &Child, signal: &str) {
    let status = Command::new("kill")
        .arg(signal)
        .arg(child.id().to_string())
        .status()
        .expect("failed to spawn `kill`");
    assert!(
        status.success(),
        "`kill {signal}` returned non-zero: {status:?}"
    );
}

/// Build the `Command` that launches the `camel` binary against `dir`'s
/// `Camel.toml`, with both stdout and stderr piped. No `--no-watch` flag is
/// passed: watching is controlled by the fixture config (`watch = true`
/// enables it, otherwise the run is single-shot). The child is wrapped in a
/// kill-on-drop guard so a failed assertion mid-test cannot leak the process.
// Shared by run_watch_test_doc_test.rs and test_intercepts.rs only; the
// test binaries that include `common` without calling these would otherwise
// warn dead_code (each compilation unit gets its own copy of the module).
#[allow(dead_code)]
pub fn spawn_camel_run(dir: &Path) -> KillOnDrop {
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
        .expect("failed to spawn `camel` binary"); // allow-unwrap
    KillOnDrop(child)
}

/// Build the `Command` that runs one job document through the `camel`
/// binary: `camel job <doc>` inside `dir` (whose `Camel.toml` feeds the
/// real boot composition), with both stdout and stderr piped. The child
/// is wrapped in a kill-on-drop guard so a failed assertion mid-test
/// cannot leak the process.
// Shared by job_one_shot_test.rs and job_signal_test.rs; the test
// binaries that include `common` without calling it would otherwise
// warn dead_code (each compilation unit gets its own copy of the
// module).
#[allow(dead_code)]
pub fn spawn_camel_job(dir: &Path, doc: &Path) -> KillOnDrop {
    spawn_camel_job_with_args(dir, doc, &[])
}

/// `spawn_camel_job` with extra trailing CLI arguments appended after
/// the document (e.g. `--report=<path>`, which keeps stdout clean and
/// routes the JSON report to a file the test asserts on). Same kill-on-
/// drop guard and piped stdio as `spawn_camel_job`.
// Shared by job_signal_test.rs; the test binaries that include `common`
// without calling it would otherwise warn dead_code (each compilation
// unit gets its own copy of the module).
#[allow(dead_code)]
pub fn spawn_camel_job_with_args(dir: &Path, doc: &Path, extra_args: &[&str]) -> KillOnDrop {
    let mut command = Command::new(env!("CARGO_BIN_EXE_camel"));
    command.arg("job").arg(doc);
    for arg in extra_args {
        command.arg(arg);
    }
    let child = command
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .expect("failed to spawn `camel` binary"); // allow-unwrap
    KillOnDrop(child)
}

/// Bounded wait for the child to exit: `Some(code)` once it has self-exited
/// within `timeout` (a death by signal maps to the `-1` sentinel, since
/// `ExitStatus::code` is `None` for a killed process); `None` when it was
/// still alive at the deadline — it is force-killed and reaped first.
// Private core of `wait_exit_bounded`/`wait_exit_code_bounded`; in the test
// binaries that call neither wrapper it is dead code like them (each
// compilation unit gets its own copy of the module).
#[allow(dead_code)]
fn wait_exit_core(child: &mut Child, timeout: Duration) -> Option<i32> {
    let start = Instant::now();
    let step = Duration::from_millis(25);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.code().unwrap_or(-1)),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                thread::sleep(step);
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }
}

/// Wait for the child to exit, but at most `timeout`. If it is still alive
/// at the deadline, force-kill and reap, returning `false`.
#[allow(dead_code)]
pub fn wait_exit_bounded(child: &mut Child, timeout: Duration) -> bool {
    wait_exit_core(child, timeout).is_some()
}

/// Wait for the child to exit, at most `timeout`; force-kill and reap at
/// the deadline, returning the `-1` sentinel. A death by signal (what a
/// shell reports as 130/143) also surfaces as `-1`: `status.code()` is
/// `None` when the process was killed.
// Shared by the job/run signal tests; the test binaries that include
// `common` without calling it would otherwise warn dead_code (each
// compilation unit gets its own copy of the module).
#[allow(dead_code)]
pub fn wait_exit_code_bounded(child: &mut Child, timeout: Duration) -> i32 {
    wait_exit_core(child, timeout).unwrap_or(-1)
}

/// Run `program` with `args` inside `dir` to completion and return
/// `(exit_code, stdout, stderr)` with both pipes drained concurrently (a
/// pipe buffer can otherwise deadlock a chatty child). The environment is
/// inherited plus `envs`. The 90 s deadline follows the generous-deadline
/// note at the module head; a child still alive there is force-killed and
/// reported as exit `-1`.
// Shared by job_one_shot_test.rs and compiled_artifact_test.rs; the test
// binaries that include `common` without calling it would otherwise warn
// dead_code (each compilation unit gets its own copy of the module).
#[allow(dead_code)]
pub fn run_binary(
    dir: &Path,
    program: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> (i32, String, String) {
    let mut child = Command::new(program)
        .args(args)
        .envs(envs.iter().copied())
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn child process");
    let out_buf = SharedBuf::default();
    let err_buf = SharedBuf::default();
    let out_handle = thread::spawn({
        let buf = Arc::clone(&out_buf);
        let stdout = child.stdout.take().expect("stdout piped");
        move || drain_to_buffer(stdout, buf)
    });
    let err_handle = thread::spawn({
        let buf = Arc::clone(&err_buf);
        let stderr = child.stderr.take().expect("stderr piped");
        move || drain_to_buffer(stderr, buf)
    });
    let deadline = Instant::now() + Duration::from_secs(90);
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break -1;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    };
    let _ = out_handle.join();
    let _ = err_handle.join();
    (
        exit_code,
        out_buf.lock().expect("stdout lock").clone(),
        err_buf.lock().expect("stderr lock").clone(),
    )
}
