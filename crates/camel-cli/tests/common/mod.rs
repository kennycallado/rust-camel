//! Shared subprocess plumbing for the `camel-cli` integration tests that
//! spawn the real `camel` binary: pipe drainers, a kill-on-drop child guard,
//! a bounded marker wait, and a single-SIGTERM sender. Observation and exit
//! deadlines here are deliberately generous (30 s): under a whole-workspace
//! `cargo test` run the OS is saturated by hundreds of peer processes and
//! subprocess startup slows roughly 100x, while the happy path still returns
//! the moment the marker is seen or the child exits, so the headroom never
//! slows the fast path.

use std::io::Read;
use std::ops::{Deref, DerefMut};
use std::process::{Child, Command};
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
/// full ~466 MB `camel` binary, which initializes ~15 always-on component
/// bundles. Under a whole-workspace `cargo test` run the OS is saturated by
/// hundreds of peer processes and subprocess startup slows ~100x. The poll
/// short-circuits the moment the marker is seen, so the generous ceiling
/// only buys headroom under load; it never slows the fast path.
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
