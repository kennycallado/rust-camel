//! Watchdog-backed process-group supervision for bridge children (mission 390).
//!
//! [`GroupGuard`] owns a *watchdog* shell process that leads its own process
//! group and is immune to `SIGTERM`. The bridge process joins that group at
//! spawn time. A liveness pipe connects the guard to the watchdog: when the
//! guard is dropped (or the whole test process exits, closing the write end),
//! the watchdog sees EOF and `SIGKILL`s its own process group, so the bridge
//! can never outlive the process that created it.
//!
//! All supervision and cleanup is runtime-independent: the monitor is a plain
//! [`std::thread`], and every signal/reap path is synchronous. Dropping or
//! shutting down the caller's Tokio runtime cannot disarm supervision.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use crate::process::BridgeError;

/// Arming script body. `trap "" TERM` makes the watchdog ignore `SIGTERM`
/// (including a group-wide graceful stop); the `read` loop only ends on EOF of
/// the liveness pipe (guard dropped); `kill -KILL -0` then kills the whole
/// process group, watchdog included.
pub(crate) const WATCHDOG_SCRIPT: &str =
    "trap \"\" TERM; while read -r _; do :; done; kill -KILL -0";

/// Fixed descriptor number the watchdog writes its arming acknowledgement to.
///
/// Must be a single digit: multi-digit redirection targets are not POSIX,
/// and `dash` rejects them with "Bad fd number". 9 is the highest of
/// the reserved 3-9 range, so it is the least likely to collide with stdio
/// duplication plumbing.
const ACK_FD: RawFd = 9;

const ARM_TIMEOUT: Duration = Duration::from_secs(2);
const MONITOR_POLL: Duration = Duration::from_millis(100);
const WAIT_POLL: Duration = Duration::from_millis(10);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

/// Serialises pipe creation (and the watchdog spawn that must observe the
/// same fd table) across every [`GroupGuard::establish_with_program`] call.
///
/// On Linux `pipe2(O_CLOEXEC)` is atomic, so the lock only matters on targets
/// without `pipe2`, where `pipe()` + `fcntl(CLOEXEC)` leaves a window in which
/// a concurrent spawn could inherit the transient descriptors.
static PIPE_SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Wraps [`WATCHDOG_SCRIPT`] with the post-`trap` acknowledgement. The parent
/// accepts arming only after the ack byte arrives, which can only be written
/// after `trap "" TERM` executed, closing the fork-to-trap window in which a
/// `SIGTERM` could otherwise kill the watchdog before it armed itself.
fn armed_script(ack_fd: RawFd) -> String {
    format!("trap \"\" TERM; printf '\\001' >&{ack_fd}; {WATCHDOG_SCRIPT}")
}

fn make_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1 as libc::c_int; 2];

    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: fds points at two valid c_int slots.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        // SAFETY: fds points at two valid c_int slots.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        for i in 0..2 {
            // SAFETY: both fds were just created by pipe().
            if unsafe { libc::fcntl(fds[i], libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                let err = io::Error::last_os_error();
                // SAFETY: both fds are open and owned here; closing them on a
                // setup failure avoids the leak.
                unsafe {
                    libc::close(fds[0]);
                    libc::close(fds[1]);
                }
                return Err(err);
            }
        }
    }

    // SAFETY: pipe/pipe2 returned two fresh descriptors that we own.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Non-reaping "has this child exited?" probe (`WNOWAIT` keeps it waitable).
/// Returns `true` on error as well, so callers fail closed.
fn child_exited(pid: u32) -> bool {
    // SAFETY: siginfo_t is a plain C struct; all-zero is a valid initial value
    // before waitid fills it.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a valid, writable siginfo_t; P_PID + our own child.
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc != 0 {
        return true;
    }
    // SAFETY: waitid succeeded and zeroed/si_pid access is defined.
    unsafe { info.si_pid() == pid as libc::pid_t }
}

/// Bounded read of the single arming acknowledgement byte.
fn read_ack(fd: RawFd, timeout: Duration) -> io::Result<u8> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "watchdog arming acknowledgement timed out",
            ));
        }
        let millis = remaining.as_millis().min(i32::MAX as u128) as i32;
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pollfd points at one valid descriptor description.
        let rc = unsafe { libc::poll(&mut pollfd, 1, millis.max(1)) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if rc == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "watchdog arming acknowledgement timed out",
            ));
        }
        let mut byte = [0u8; 1];
        // SAFETY: byte is a valid 1-byte destination.
        let n = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), 1) };
        if n == 1 {
            return Ok(byte[0]);
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "watchdog closed the arming pipe before acknowledging",
            ));
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err);
    }
}

/// Best-effort abort for a watchdog that failed arming: group `SIGKILL`
/// (never a bare-PID signal) then a bounded reap.
fn abort_watchdog(child: &mut tokio::process::Child, pid: u32) {
    // SAFETY: negative pid addresses the watchdog's own process group.
    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    let deadline = Instant::now() + ARM_TIMEOUT;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => {}
        }
        std::thread::sleep(WAIT_POLL);
    }
}

fn supervision(message: impl Into<String>) -> BridgeError {
    BridgeError::Supervision(message.into())
}

/// Reap policy for the synchronous cleanup core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReapMode {
    /// Poll `try_wait` until both children are reaped (bounded per child).
    Blocking,
    /// Single `try_wait` per child; leaves any straggler to the OS.
    NonBlocking,
}

/// State shared between the guard, its monitor thread, and `spawn_blocking`
/// cleanup workers. All fields are synchronous primitives; `GroupShared` is
/// `Send + Sync`.
struct GroupShared {
    pgid: Mutex<Option<i32>>,
    write_end: Mutex<Option<OwnedFd>>,
    watchdog: Mutex<Option<tokio::process::Child>>,
    bridge: Mutex<Option<tokio::process::Child>>,
    /// Serialises spawn against signal/reap. Held for the whole synchronous
    /// cleanup sequence; never held across an `.await`.
    cleanup_exec: Mutex<()>,
    /// Set at the start of every cleanup so the monitor stands down.
    stop: AtomicBool,
    /// Per-instance test hook: when set, the monitor skips triggering cleanup.
    #[cfg(test)]
    monitor_paused: AtomicBool,
}

impl GroupShared {
    /// Synchronous cleanup core. The caller MUST hold `cleanup_exec`.
    fn cleanup_locked(&self, wait: Option<Duration>, reap: ReapMode) -> Result<(), BridgeError> {
        self.stop.store(true, Ordering::SeqCst);

        let pgid = *lock(&self.pgid);
        if let Some(pgid) = pgid {
            // SAFETY: negative pid = process group; designed to fail benignly
            // when the group is already gone.
            unsafe { libc::kill(-pgid, libc::SIGTERM) };

            let bridge_pid = lock(&self.bridge)
                .as_ref()
                .and_then(tokio::process::Child::id);
            if let Some(grace) = wait
                && let Some(bridge_pid) = bridge_pid
            {
                let deadline = Instant::now() + grace;
                while !child_exited(bridge_pid) && Instant::now() < deadline {
                    std::thread::sleep(WAIT_POLL);
                }
            }

            // TERM-immune members (the watchdog) only fall to KILL.
            // SAFETY: negative pid = process group.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };

            // Invalidate BEFORE any reap: no signal may be issued afterwards.
            *lock(&self.pgid) = None;
        }

        reap_slot(&self.bridge, "bridge", reap)?;
        reap_slot(&self.watchdog, "watchdog", reap)?;
        Ok(())
    }

    /// Lock-taking wrapper around [`Self::cleanup_locked`].
    fn cleanup_sync(&self, wait: Option<Duration>, reap: ReapMode) -> Result<(), BridgeError> {
        let _exec = lock(&self.cleanup_exec);
        self.cleanup_locked(wait, reap)
    }

    /// Async stop façade: the blocking cleanup runs on a `spawn_blocking`
    /// worker so no mutex guard crosses an `.await` on the caller's task.
    pub(crate) async fn stop_group(self: &Arc<Self>, grace: Duration) -> Result<(), BridgeError> {
        let shared = Arc::clone(self);
        tokio::task::spawn_blocking(move || shared.cleanup_sync(Some(grace), ReapMode::Blocking))
            .await
            .map_err(|err| supervision(format!("cleanup worker join failed: {err}")))?
    }

    /// EOF-path classified shutdown: wait (non-reaping `WNOWAIT`) up to `wait`
    /// for the already-exited bridge, then run the full serialized cleanup
    /// ordering (group TERM → KILL → pgid invalidation) before reaping the
    /// bridge and capturing its exit status.
    ///
    /// This preserves the invariant "no reap before the final group signal and
    /// invalidation": the bridge is never reaped by a path that could still
    /// issue a group signal afterwards. Signaling happens before the reap in
    /// both this and the normal [`Self::cleanup_locked`] path.
    ///
    /// `Ok(None)` means the bridge had not exited within the bound; the guard's
    /// drop path remains responsible for killing and reaping it.
    fn shutdown_classified(
        &self,
        wait: Duration,
    ) -> Result<Option<std::process::ExitStatus>, BridgeError> {
        let bridge_pid = lock(&self.bridge)
            .as_ref()
            .and_then(tokio::process::Child::id);
        if let Some(pid) = bridge_pid {
            let deadline = Instant::now() + wait;
            while !child_exited(pid) && Instant::now() < deadline {
                std::thread::sleep(WAIT_POLL);
            }
        }

        let _exec = lock(&self.cleanup_exec);
        self.stop.store(true, Ordering::SeqCst);

        let pgid = *lock(&self.pgid);
        if let Some(pgid) = pgid {
            // SAFETY: negative pid = process group; designed to fail benignly
            // when the group is already gone.
            unsafe { libc::kill(-pgid, libc::SIGTERM) };

            // The bridge has already exited at EOF, so there is no grace wait;
            // TERM-immune members (the watchdog) only fall to KILL.
            // SAFETY: negative pid = process group.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };

            // Invalidate BEFORE any reap: no signal may be issued afterwards.
            *lock(&self.pgid) = None;
        }

        let status = match lock(&self.bridge).as_mut() {
            Some(child) => child
                .try_wait()
                .map_err(|err| supervision(format!("reap bridge failed: {err}")))?,
            None => None,
        };
        reap_slot(&self.watchdog, "watchdog", ReapMode::Blocking)?;
        Ok(status)
    }
}

fn reap_slot(
    slot: &Mutex<Option<tokio::process::Child>>,
    label: &str,
    mode: ReapMode,
) -> Result<(), BridgeError> {
    let mut guard = lock(slot);
    let Some(child) = guard.as_mut() else {
        return Ok(());
    };

    match mode {
        ReapMode::NonBlocking => {
            child
                .try_wait()
                .map_err(|err| supervision(format!("reap {label} failed: {err}")))?;
            Ok(())
        }
        ReapMode::Blocking => {
            let deadline = Instant::now() + REAP_TIMEOUT;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return Ok(()),
                    Ok(None) => {}
                    Err(err) => return Err(supervision(format!("reap {label} failed: {err}"))),
                }
                if Instant::now() >= deadline {
                    // log-policy: system-broken
                    tracing::error!(
                        child = label,
                        "bridge supervision: child reap timed out post-KILL"
                    );
                    return Err(supervision("reap timed out post-KILL"));
                }
                std::thread::sleep(WAIT_POLL);
            }
        }
    }
}

/// Dedicated monitor thread: waitid (non-reaping) on the watchdog, then run
/// the serialized cleanup directly on this thread if it dies early.
fn monitor_loop(weak: Weak<GroupShared>, watchdog_pid: u32) {
    loop {
        std::thread::sleep(MONITOR_POLL);

        let Some(shared) = weak.upgrade() else {
            return;
        };
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        #[cfg(test)]
        if shared.monitor_paused.load(Ordering::SeqCst) {
            continue;
        }
        if child_exited(watchdog_pid) {
            let _ = shared.cleanup_sync(None, ReapMode::Blocking);
            // log-policy: system-broken
            tracing::error!("bridge watchdog exited early; group cleaned");
            return;
        }
    }
}

/// Owns the watchdog, the bridge child, and the monitor thread.
pub(crate) struct GroupGuard {
    shared: Arc<GroupShared>,
    /// The monitor detaches on drop; the stop flag plus the dropped `Weak` make
    /// it exit within one poll interval. It is never joined: joining could
    /// block behind an in-flight cleanup on the same thread.
    _monitor: Option<std::thread::JoinHandle<()>>,
}

/// How the watchdog child is launched.
enum WatchdogSpawn {
    /// POSIX-shell fallback (`sh -c <armed script>`).
    Shell(String),
    /// Host binary re-executed in watchdog role.
    Hook,
}

impl WatchdogSpawn {
    /// Spawn failure, classified per launch kind.
    ///
    /// The shell branch names BOTH remedies when no hook is installed: the
    /// caller has no shell watchdog and no self-exec fallback.
    fn spawn_error(&self, err: &io::Error) -> BridgeError {
        match self {
            Self::Hook => supervision(format!("watchdog spawn (self-exec) failed: {err}")),
            Self::Shell(program) => {
                if crate::watchdog::hook_executable().is_some() {
                    supervision(format!("watchdog spawn ({program}) failed: {err}"))
                } else {
                    supervision(format!(
                        "watchdog spawn ({program}) failed: no POSIX sh available to run the \
                         bridge watchdog and the host binary has no watchdog hook: call \
                         camel_bridge::watchdog::maybe_run() as the first statement of main, or \
                         provide a POSIX sh (error: {err})"
                    ))
                }
            }
        }
    }
}

impl GroupGuard {
    /// Establish a watchdog-guarded group.
    ///
    /// With the embedder hook installed ([`crate::watchdog::maybe_run`]) the
    /// host binary itself is re-executed as the watchdog. Without the hook the
    /// POSIX-shell fallback keeps the legacy protocol unchanged.
    pub(crate) fn establish() -> Result<Self, BridgeError> {
        match crate::watchdog::hook_executable() {
            Some(exe) => {
                #[cfg(target_os = "linux")]
                let _ = exe;
                #[cfg(target_os = "linux")]
                let program = std::path::PathBuf::from("/proc/self/exe");
                #[cfg(not(target_os = "linux"))]
                let program = exe.to_path_buf();
                Self::establish_with_hook(&program)
            }
            None => Self::establish_with_program("sh"),
        }
    }

    /// Establish with an injectable watchdog shell (tests inject a failing one
    /// to prove fail-closed arming).
    pub(crate) fn establish_with_program(program: &str) -> Result<Self, BridgeError> {
        let mut cmd = tokio::process::Command::new(program);
        cmd.arg("-c").arg(armed_script(ACK_FD));
        Self::establish_spawn(cmd, WatchdogSpawn::Shell(program.to_string()))
    }

    /// Establish with the host binary re-executed in watchdog role.
    fn establish_with_hook(program: &std::path::Path) -> Result<Self, BridgeError> {
        let mut cmd = tokio::process::Command::new(program);
        cmd.arg(crate::watchdog::ROLE_MARKER)
            .arg(ACK_FD.to_string());
        Self::establish_spawn(cmd, WatchdogSpawn::Hook)
    }

    /// Shared arming core: pipes, process-group leadership, ack verification,
    /// and the monitor thread. `cmd` already carries the watchdog arguments.
    fn establish_spawn(
        mut cmd: tokio::process::Command,
        kind: WatchdogSpawn,
    ) -> Result<Self, BridgeError> {
        let spawn_lock = lock(&PIPE_SPAWN_LOCK);
        let (live_read, live_write) =
            make_pipe().map_err(|err| supervision(format!("liveness pipe failed: {err}")))?;
        let (ack_read, ack_write) =
            make_pipe().map_err(|err| supervision(format!("arming pipe failed: {err}")))?;

        let ack_raw = ack_write.as_raw_fd();

        cmd.stdin(Stdio::from(File::from(live_read)))
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        // SAFETY: only async-signal-safe calls (setpgid/dup2/fcntl/close) run
        // between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if ack_raw == ACK_FD {
                    // Same fd already: just make sure it survives exec.
                    if libc::fcntl(ACK_FD, libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                } else {
                    // dup2 clears FD_CLOEXEC on the new descriptor, so the
                    // copy at ACK_FD survives exec while the original pipe fd
                    // keeps its close-on-exec flag. The copy dies with the
                    // watchdog (only it and the parent hold the write end).
                    if libc::dup2(ack_raw, ACK_FD) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    libc::close(ack_raw);
                }
                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(|err| kind.spawn_error(&err))?;

        // The child owns the dup'd ack fd; closing our copy here means both an
        // EOF and a successful ack are observable without waiting out the
        // timeout.
        drop(ack_write);
        drop(spawn_lock);

        let watchdog_pid = match child.id() {
            Some(pid) => pid,
            None => {
                return Err(supervision("watchdog exited before it could be tracked"));
            }
        };

        let ack_file = File::from(ack_read);
        match read_ack(ack_file.as_raw_fd(), ARM_TIMEOUT) {
            Ok(1) => {}
            Ok(byte) => {
                abort_watchdog(&mut child, watchdog_pid);
                return Err(supervision(format!(
                    "unexpected arming acknowledgement byte {byte}"
                )));
            }
            Err(err) => {
                abort_watchdog(&mut child, watchdog_pid);
                return Err(supervision(format!("arming acknowledgement failed: {err}")));
            }
        }
        drop(ack_file);

        let watchdog_pid_i32 = watchdog_pid as i32;
        // SAFETY: getpgid/kill with a known live child pid.
        let actual_pgid = unsafe { libc::getpgid(watchdog_pid_i32) };
        if actual_pgid != watchdog_pid_i32 {
            abort_watchdog(&mut child, watchdog_pid);
            return Err(supervision(
                "watchdog is not its own process group leader after arming",
            ));
        }
        // SAFETY: signal 0 is an existence probe only.
        if unsafe { libc::kill(watchdog_pid_i32, 0) } != 0 {
            abort_watchdog(&mut child, watchdog_pid);
            return Err(supervision("watchdog is not alive after arming"));
        }

        let shared = Arc::new(GroupShared {
            pgid: Mutex::new(Some(watchdog_pid_i32)),
            write_end: Mutex::new(Some(live_write)),
            watchdog: Mutex::new(Some(child)),
            bridge: Mutex::new(None),
            cleanup_exec: Mutex::new(()),
            stop: AtomicBool::new(false),
            #[cfg(test)]
            monitor_paused: AtomicBool::new(false),
        });

        match spawn_monitor(&shared, watchdog_pid) {
            Ok(handle) => Ok(Self {
                shared,
                _monitor: Some(handle),
            }),
            Err(err) => {
                let _ = shared.cleanup_sync(None, ReapMode::Blocking);
                Err(supervision(format!(
                    "watchdog monitor thread spawn failed: {err}"
                )))
            }
        }
    }

    /// Spawn the bridge into the watchdog's process group. The bridge joins the
    /// group in `pre_exec`, so a failed join aborts the spawn instead of
    /// leaking an unguarded process.
    pub(crate) fn spawn_bridge(&self, mut cmd: tokio::process::Command) -> Result<(), BridgeError> {
        let _exec = lock(&self.shared.cleanup_exec);

        let pgid = match *lock(&self.shared.pgid) {
            Some(pgid) => pgid,
            None => return Err(supervision("group already invalidated; refusing spawn")),
        };

        let watchdog_pid = lock(&self.shared.watchdog)
            .as_ref()
            .and_then(tokio::process::Child::id)
            .ok_or_else(|| supervision("watchdog handle unavailable; refusing spawn"))?;

        // waitid sees the exit while leaving the child waitable, so a zombie
        // watchdog (which still answers kill(pid, 0)) is rejected here.
        if child_exited(watchdog_pid) {
            return Err(supervision("watchdog exited before bridge spawn"));
        }

        // SAFETY: setpgid only, async-signal-safe.
        unsafe {
            cmd.pre_exec(move || {
                if libc::setpgid(0, pgid) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let child = cmd
            .spawn()
            .map_err(|err| supervision(format!("bridge spawn failed: {err}")))?;
        *lock(&self.shared.bridge) = Some(child);
        Ok(())
    }

    /// Watchdog pid (unit tests only; integration tests assert by token).
    #[cfg(test)]
    pub(crate) fn watchdog_pid(&self) -> Option<u32> {
        lock(&self.shared.watchdog)
            .as_ref()
            .and_then(tokio::process::Child::id)
    }

    /// Bridge pid (read-only lifecycle accessor).
    pub(crate) fn bridge_pid(&self) -> Option<u32> {
        lock(&self.shared.bridge)
            .as_ref()
            .and_then(tokio::process::Child::id)
    }

    /// Take the bridge's stdout pipe (the guard owns the child).
    pub(crate) fn take_bridge_stdout(&self) -> Option<tokio::process::ChildStdout> {
        lock(&self.shared.bridge)
            .as_mut()
            .and_then(|child| child.stdout.take())
    }

    /// Take the bridge's stderr pipe (the guard owns the child).
    pub(crate) fn take_bridge_stderr(&self) -> Option<tokio::process::ChildStderr> {
        lock(&self.shared.bridge)
            .as_mut()
            .and_then(|child| child.stderr.take())
    }

    /// Async graceful stop façade for the owning `BridgeProcess`.
    pub(crate) async fn stop_group(&self, grace: Duration) -> Result<(), BridgeError> {
        self.shared.stop_group(grace).await
    }

    /// Bounded classified shutdown for the EOF-exit path: wait non-reaping for
    /// the already-exited child, then run the serialized group signal +
    /// invalidation ordering and reap to obtain its exit status. Never issues a
    /// bare-PID signal.
    pub(crate) async fn shutdown_classified(
        &self,
        wait: Duration,
    ) -> Result<Option<std::process::ExitStatus>, BridgeError> {
        let shared = Arc::clone(&self.shared);
        tokio::task::spawn_blocking(move || shared.shutdown_classified(wait))
            .await
            .map_err(|err| supervision(format!("bridge shutdown worker join failed: {err}")))?
    }

    /// Current process-group id (unit tests only).
    #[cfg(test)]
    fn group_pgid(&self) -> Option<i32> {
        *lock(&self.shared.pgid)
    }
}

fn spawn_monitor(
    shared: &Arc<GroupShared>,
    watchdog_pid: u32,
) -> io::Result<std::thread::JoinHandle<()>> {
    let weak = Arc::downgrade(shared);
    std::thread::Builder::new()
        .name("camel-bridge-watchdog-monitor".into())
        .spawn(move || monitor_loop(weak, watchdog_pid))
}

impl Drop for GroupGuard {
    /// Synchronous teardown: never awaits, works with or without a runtime.
    fn drop(&mut self) {
        let shared = Arc::clone(&self.shared);
        shared.stop.store(true, Ordering::SeqCst);

        match shared.cleanup_exec.try_lock() {
            Ok(_exec) => {
                // Hold the guard directly: re-acquiring via cleanup_sync would
                // deadlock on the non-reentrant std mutex.
                if let Err(err) = shared.cleanup_locked(None, ReapMode::NonBlocking) {
                    // log-policy: system-broken
                    tracing::error!(error = %err, "bridge supervision: drop cleanup failed");
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                // An in-flight cleanup (monitor thread or spawn_blocking
                // worker) owns the sequence; do not race it.
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                let _exec = poisoned.into_inner();
                let _ = shared.cleanup_locked(None, ReapMode::NonBlocking);
            }
        }

        // Closing the liveness write end makes a group that somehow outlived
        // this guard still see EOF, and the watchdog kills itself.
        if let Ok(mut write_end) = shared.write_end.lock() {
            let _ = write_end.take();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Stdio;
    use tokio::process::Command;

    struct PauseGuard<'a>(&'a AtomicBool);

    impl<'a> PauseGuard<'a> {
        fn new(flag: &'a AtomicBool) -> Self {
            flag.store(true, Ordering::SeqCst);
            Self(flag)
        }
    }

    impl Drop for PauseGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    fn sleep_cmd(secs: u64) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("sleep {secs}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if condition() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(WAIT_POLL);
        }
    }

    #[cfg(target_os = "linux")]
    fn process_state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, rest) = stat.rsplit_once(')')?;
        rest.trim_start().chars().next()
    }

    /// Running = not a zombie and not gone. Zombies are pending reaps, not
    /// survivors (same rule as the evidence harness).
    #[cfg(target_os = "linux")]
    fn pid_running(pid: u32) -> bool {
        !matches!(process_state(pid), None | Some('Z') | Some('X'))
    }

    #[cfg(target_os = "linux")]
    fn group_running(pgid: i32) -> bool {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return false;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            let Some((_, rest)) = stat.rsplit_once(')') else {
                continue;
            };
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if fields.len() < 3 {
                continue;
            }
            let state = fields[0].chars().next();
            let Ok(pgrp) = fields[2].parse::<i32>() else {
                continue;
            };
            if pgrp == pgid && !matches!(state, Some('Z') | Some('X')) {
                return true;
            }
        }
        false
    }

    /// Bare `kill(pid, 0)` also answers 0 for zombies. A zombie is a pending
    /// reap, not a survivor. The Linux counterpart excludes Z/X states via
    /// /proc. BSD-family kernels detach an exited process from its process
    /// group at exit, so after group invalidation group-gone does not imply
    /// reaped: tests must wait (bounded) for the reap itself; see
    /// [`early_watchdog_death_surfaces`].
    #[cfg(not(target_os = "linux"))]
    fn pid_running(pid: u32) -> bool {
        // SAFETY: signal 0 is an existence probe.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    /// Bare `kill(-pgid, 0)` probe. The group may read empty while
    /// exited-but-unreaped members exist; do not treat group-gone as reaped.
    #[cfg(not(target_os = "linux"))]
    fn group_running(pgid: i32) -> bool {
        // SAFETY: signal 0 is an existence probe for the group.
        unsafe { libc::kill(-pgid, 0) == 0 }
    }

    #[test]
    fn watchdog_script_const_traps_term() {
        assert!(WATCHDOG_SCRIPT.contains("trap \"\" TERM"));
        assert!(WATCHDOG_SCRIPT.contains("kill -KILL -0"));

        let armed = armed_script(ACK_FD);
        let trap_at = armed
            .find("trap \"\" TERM")
            .expect("armed script must trap TERM first");
        let ack_at = armed
            .find("printf")
            .expect("armed script must acknowledge after trapping");
        assert!(trap_at < ack_at, "trap must precede the ack: {armed}");
    }

    /// True when the script redirects to a multi-digit descriptor, which is
    /// not POSIX and is rejected by dash.
    fn has_multi_digit_redirect(script: &str) -> bool {
        script.match_indices(">&").any(|(idx, _)| {
            script[idx + 2..]
                .bytes()
                .take(2)
                .filter(u8::is_ascii_digit)
                .count()
                >= 2
        })
    }

    /// Locate an alternative POSIX shell to prove that the armed script does
    /// not depend on GNU/bash-only redirection syntax.
    fn alternative_posix_shell() -> Option<&'static str> {
        ["dash", "mksh", "yash"].into_iter().find(|program| {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("command -v {program}"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    }

    #[test]
    fn ack_script_is_single_digit_fd() {
        let armed = armed_script(ACK_FD);
        assert!(
            armed.contains(">&9"),
            "armed script must acknowledge on a single-digit fd: {armed}"
        );
        assert!(
            !has_multi_digit_redirect(&armed),
            "armed script must not use a multi-digit redirection target: {armed}"
        );
    }

    #[tokio::test]
    async fn dash_shell_establishes_and_drops() {
        let Some(program) = alternative_posix_shell() else {
            println!("skipped: no alternative POSIX shell (dash/mksh/yash) on this system");
            return;
        };

        let guard = GroupGuard::establish_with_program(program).unwrap_or_else(|err| {
            panic!("{program} could not arm the watchdog (non-POSIX ack redirect?): {err}")
        });
        let pgid = guard.group_pgid().expect("pgid present after establish");
        guard
            .spawn_bridge(sleep_cmd(300))
            .expect("spawn bridge under alternative-shell watchdog");
        let bridge_pid = guard.bridge_pid().expect("bridge pid");

        drop(guard);

        assert!(
            wait_until(Duration::from_secs(5), || {
                !group_running(pgid) && !pid_running(bridge_pid)
            }),
            "{program} watchdog left a survivor after drop (pgid {pgid}, bridge {bridge_pid})"
        );
    }

    #[tokio::test]
    async fn establish_and_drop_kills_group() {
        let guard = GroupGuard::establish().expect("establish guard");
        let pgid = guard.group_pgid().expect("pgid present after establish");
        guard.spawn_bridge(sleep_cmd(300)).expect("spawn bridge");
        let bridge_pid = guard.bridge_pid().expect("bridge pid");

        drop(guard);

        assert!(
            wait_until(Duration::from_secs(5), || {
                !group_running(pgid) && !pid_running(bridge_pid)
            }),
            "group {pgid} or bridge {bridge_pid} still running after drop"
        );
    }

    #[tokio::test]
    async fn group_sigterm_leaves_watchdog_alive() {
        let guard = GroupGuard::establish().expect("establish guard");
        let pgid = guard.group_pgid().expect("pgid");
        let watchdog_pid = guard.watchdog_pid().expect("watchdog pid");

        // SAFETY: negative pid = group-wide SIGTERM.
        assert_eq!(
            unsafe { libc::kill(-pgid, libc::SIGTERM) },
            0,
            "send SIGTERM"
        );

        let died = wait_until(Duration::from_millis(300), || !pid_running(watchdog_pid));
        assert!(!died, "watchdog must be TERM-immune after arming");
        // SAFETY: signal 0 is an existence probe.
        assert_eq!(
            unsafe { libc::kill(watchdog_pid as i32, 0) },
            0,
            "watchdog must still answer after group SIGTERM"
        );
    }

    #[tokio::test]
    async fn no_signal_after_invalidation() {
        let guard = GroupGuard::establish().expect("establish guard");
        let pgid = guard.group_pgid().expect("pgid");

        guard
            .shared
            .stop_group(Duration::from_millis(100))
            .await
            .expect("first stop");
        assert_eq!(guard.group_pgid(), None, "pgid must be invalidated");

        // Second call sees pgid == None: no signal is issued, and it succeeds.
        guard
            .shared
            .stop_group(Duration::from_millis(100))
            .await
            .expect("second stop is a no-op success");
        assert!(!group_running(pgid));
    }

    #[tokio::test]
    async fn early_watchdog_death_surfaces() {
        let guard = GroupGuard::establish().expect("establish guard");
        let pgid = guard.group_pgid().expect("pgid");
        guard.spawn_bridge(sleep_cmd(300)).expect("spawn bridge");
        let bridge_pid = guard.bridge_pid().expect("bridge pid");
        let watchdog_pid = guard.watchdog_pid().expect("watchdog pid");

        // SAFETY: direct watchdog kill simulates early watchdog death.
        assert_eq!(unsafe { libc::kill(watchdog_pid as i32, libc::SIGKILL) }, 0);

        assert!(
            wait_until(Duration::from_secs(5), || {
                guard.group_pgid().is_none() && !group_running(pgid) && !pid_running(bridge_pid)
            }),
            "early watchdog death left group {pgid} or bridge {bridge_pid} uncleaned within 5s"
        );
    }

    #[tokio::test]
    async fn establish_failure_is_fail_closed() {
        if let Err(BridgeError::Supervision(_)) =
            GroupGuard::establish_with_program("/nonexistent/sh")
        {
            return;
        }
        panic!("expected fail-closed Supervision error");
    }

    /// No hook installed in this test process, so a shell that cannot spawn
    /// must fail closed and name BOTH remedies in the error.
    #[test]
    fn no_hook_no_shell_fails_closed_names_both_remedies() {
        let err = match GroupGuard::establish_with_program("/nonexistent/sh") {
            Ok(_) => panic!("expected fail-closed Supervision error"),
            Err(err) => err,
        };
        let message = err.to_string();
        assert!(
            message.contains("watchdog spawn"),
            "error must keep the watchdog-spawn context: {message}"
        );
        assert!(
            message.contains("maybe_run"),
            "error must name the hook remedy: {message}"
        );
        assert!(
            message.contains("POSIX sh"),
            "error must name the shell remedy: {message}"
        );
    }

    #[tokio::test]
    async fn graceful_stop_reaps_both_children() {
        let guard = GroupGuard::establish().expect("establish guard");
        let pgid = guard.group_pgid().expect("pgid");
        guard.spawn_bridge(sleep_cmd(30)).expect("spawn bridge");

        guard
            .shared
            .stop_group(Duration::from_secs(2))
            .await
            .expect("graceful stop");

        assert_eq!(guard.group_pgid(), None);
        let bridge_reaped = lock(&guard.shared.bridge)
            .as_mut()
            .expect("bridge handle retained")
            .try_wait()
            .expect("bridge try_wait")
            .is_some();
        let watchdog_reaped = lock(&guard.shared.watchdog)
            .as_mut()
            .expect("watchdog handle retained")
            .try_wait()
            .expect("watchdog try_wait")
            .is_some();
        assert!(bridge_reaped, "bridge must be reaped, not left a zombie");
        assert!(
            watchdog_reaped,
            "watchdog must be reaped, not left a zombie"
        );
        assert!(!group_running(pgid), "no group member may remain running");
    }

    #[tokio::test]
    async fn drop_reaps_without_signals() {
        let guard = GroupGuard::establish().expect("establish guard");
        let pgid = guard.group_pgid().expect("pgid");
        guard.spawn_bridge(sleep_cmd(300)).expect("spawn bridge");
        let bridge_pid = guard.bridge_pid().expect("bridge pid");
        let watchdog_pid = guard.watchdog_pid().expect("watchdog pid");

        // Drop path issues only group signals (group KILL before invalidation,
        // watchdog self-KILL on EOF); no bare-PID signal exists. Behavioural
        // outcome: no survivors.
        drop(guard);

        assert!(
            wait_until(Duration::from_secs(5), || {
                !group_running(pgid) && !pid_running(bridge_pid) && !pid_running(watchdog_pid)
            }),
            "survivors after drop: bridge {bridge_pid}, watchdog {watchdog_pid}, pgid {pgid}"
        );
    }

    #[tokio::test]
    async fn spawn_after_watchdog_death_fails_closed() {
        let guard = GroupGuard::establish().expect("establish guard");
        let pgid = guard.group_pgid().expect("pgid");
        let watchdog_pid = guard.watchdog_pid().expect("watchdog pid");

        let pause = PauseGuard::new(&guard.shared.monitor_paused);
        // SAFETY: direct watchdog kill; monitor is paused so the zombie stays.
        assert_eq!(unsafe { libc::kill(watchdog_pid as i32, libc::SIGKILL) }, 0);
        assert!(
            wait_until(Duration::from_secs(2), || child_exited(watchdog_pid)),
            "watchdog exit was not observed (zombie state not reached)"
        );
        assert_eq!(
            guard.group_pgid(),
            Some(pgid),
            "pgid must still be Some while the monitor is paused"
        );

        let err = guard
            .spawn_bridge(sleep_cmd(300))
            .expect_err("zombie watchdog must reject a bridge spawn");
        assert!(matches!(err, BridgeError::Supervision(_)));

        drop(pause);
        assert!(
            wait_until(Duration::from_secs(2), || {
                guard.group_pgid().is_none() && !pid_running(watchdog_pid)
            }),
            "monitor did not clean the group after unpause"
        );
    }

    /// Pins the per-platform "is this pid running?" predicate against an
    /// unreaped zombie, then proves that reaping clears it on every platform.
    ///
    /// Linux answers via `/proc/<pid>/stat` (a zombie is a pending reap, not a
    /// survivor). BSD-family kernels answer via bare `kill(pid, 0)`, which
    /// returns 0 for a zombie: that is the trap this test pins.
    #[test]
    fn zombie_probe_contract() {
        // NOTE: if an assertion below fires, the fixture child stays an
        // unreaped zombie until this test binary exits (`std::process::Child`
        // does not reap on drop). That is acceptable for a test fixture.
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn zombie fixture");
        let pid = child.id();

        // STRICT observation: `waitid(WNOWAIT)` only succeeds once the child
        // has exited, and it does not reap. `child_exited` is deliberately not
        // used here because it returns `true` on any waitid error.
        let mut probe_error: Option<io::Error> = None;
        let observed = wait_until(Duration::from_secs(5), || {
            if probe_error.is_some() {
                return false;
            }
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is a valid, writable siginfo_t for our own child;
            // WNOWAIT leaves the exit status waitable for the explicit reap
            // below.
            let rc = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    return false;
                }
                probe_error = Some(err);
                return false;
            }
            // SAFETY: waitid succeeded, so the siginfo_t was initialized.
            unsafe { info.si_pid() == pid as libc::pid_t }
        });
        assert!(
            observed,
            "strict waitid did not observe the unreaped zombie within 5s: {probe_error:?}"
        );

        #[cfg(target_os = "linux")]
        assert!(
            !pid_running(pid),
            "linux helper must treat a zombie as a pending reap, not a survivor"
        );

        #[cfg(target_os = "macos")]
        assert!(
            pid_running(pid),
            "bare probe answers 0 for a zombie; tests must wait for the reap"
        );

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            // No verified zombie predicate exists on this target; asserting
            // either answer would pin an accident. The reap invariant below
            // still holds everywhere.
        }

        let _status = child.wait().expect("reap zombie fixture");
        assert!(
            !pid_running(pid),
            "reaped child must be gone on every platform"
        );
    }

    /// Proves that the monitor thread, not the test, reaps BOTH children after
    /// an early watchdog death: the "no permanent zombie" invariant.
    #[tokio::test]
    async fn monitor_reaps_after_early_watchdog_death() {
        let guard = GroupGuard::establish().expect("establish guard");
        let pgid = guard.group_pgid().expect("pgid");
        guard.spawn_bridge(sleep_cmd(300)).expect("spawn bridge");
        let bridge_pid = guard.bridge_pid().expect("bridge pid");
        let watchdog_pid = guard.watchdog_pid().expect("watchdog pid");

        // SAFETY: direct watchdog kill simulates early watchdog death.
        assert_eq!(unsafe { libc::kill(watchdog_pid as i32, libc::SIGKILL) }, 0);

        // `tokio::process::Child::id()` returns `None` only after an internal
        // `try_wait` through the retained handle observed the exit
        // (`FusedChild::Done`). This test never calls `try_wait` before the
        // check, so only the monitor's cleanup can clear the handle state.
        // The check itself must not do the reaping, hence `id()` and not
        // `try_wait()`.
        assert!(
            wait_until(Duration::from_secs(5), || {
                guard.group_pgid().is_none()
                    && lock(&guard.shared.bridge)
                        .as_ref()
                        .and_then(tokio::process::Child::id)
                        .is_none()
                    && lock(&guard.shared.watchdog)
                        .as_ref()
                        .and_then(tokio::process::Child::id)
                        .is_none()
            }),
            "monitor left an unreaped child after early watchdog death (permanent zombie): \
             bridge {bridge_pid}, watchdog {watchdog_pid}, pgid {pgid}"
        );
    }
}
