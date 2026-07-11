//! process.rs — OS layer: spawn, capped IO drain, process-tree kill.

use bytes::Bytes;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Result of a successful (possibly timed-out) execution.
pub struct RawResult {
    pub stdout: Bytes,
    pub stderr: Bytes,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Drain a reader into Bytes, stopping storage (but continuing to drain) once
/// `cap` is reached. Sets truncated=true if the stream exceeded the cap.
pub async fn drain_with_cap<R: tokio::io::AsyncRead + Unpin>(r: R, cap: usize) -> (Bytes, bool) {
    let mut buf = Vec::with_capacity(cap.min(8192));
    let mut reader = r;
    let mut truncated = false;
    let mut tmp = [0u8; 8192];
    loop {
        match reader.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => {
                if !truncated {
                    let space = cap - buf.len();
                    if n <= space {
                        buf.extend_from_slice(&tmp[..n]);
                    } else {
                        // Store the prefix that fits, then flip truncated.
                        // Keep draining so the child never blocks on a full pipe.
                        buf.extend_from_slice(&tmp[..space]);
                        truncated = true;
                    }
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "drain read interrupted");
                break;
            }
        }
    }
    (Bytes::from(buf), truncated)
}

/// Spawn the child in its own process group (Unix) with given env/cwd/args.
/// `kill_on_drop(true)` is a belt-and-suspenders guard so an accidentally-dropped
/// Child never leaks a process (C-1 defense in depth).
pub fn spawn(
    exe: &std::path::Path,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    cwd: &std::path::Path,
) -> std::io::Result<tokio::process::Child> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter())
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        // process_group(0) creates a new process group for the child.
        // This is a standard POSIX operation. The resulting Child id is
        // always non-zero after a successful spawn, so kill_tree can safely
        // use -pgid later.
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// Kill the whole process group (Unix). Best-effort on Windows (post-v1: job object).
/// M-5: guard against pid 0/None (would signal the caller's own group if the child
/// already exited and `id()` returned None).
pub fn kill_tree(child: &tokio::process::Child) {
    let Some(pid) = child.id() else { return };
    #[cfg(unix)]
    {
        // SAFETY: sending SIGKILL to a negative pid signals the whole process group.
        // This is a kernel syscall with no in-process memory-safety implications.
        // pid is guaranteed non-zero (guarded above) so we cannot accidentally kill
        // our own process group. The return value is intentionally ignored (best-effort
        // kill — the child may already have exited).
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.start_kill();
    }
}
