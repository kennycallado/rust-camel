//! Self-executing watchdog role for bridge process-group supervision
//! (mission 417).
//!
//! An embedder installs the hook by calling [`maybe_run`](crate::watchdog::maybe_run) as the *first
//! statement of a synchronous `main`*, before anything consumes argv. The
//! bridge crate then re-executes the host binary itself as the watchdog
//! instead of requiring a POSIX `sh` on the host.
//!
//! The role speaks exactly the shell script's protocol: it refuses to arm
//! unless it already leads its own process group, ignores `SIGTERM`, writes a
//! single `0x01` acknowledgement byte to the descriptor passed as `argv[2]`,
//! waits for EOF on the liveness pipe (stdin), then `SIGKILL`s its process
//! group.
//!
//! The module compiles on every platform so embedders can call
//! [`maybe_run`](crate::watchdog::maybe_run) unconditionally. On non-Unix
//! targets it is a documented no-op: there are no process groups, so the role
//! protocol and group interaction do not exist there.

#[cfg(unix)]
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::os::fd::RawFd;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::OnceLock;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

/// Role marker passed as `argv[1]` when the host binary is re-executed as the
/// watchdog. Group supervision derives its spawn arguments from this constant.
pub const ROLE_MARKER: &str = "__camel-bridge-watchdog";

/// Arming acknowledgement byte. Matches the shell watchdog's `printf '\001'`.
#[cfg(unix)]
const ACK_BYTE: u8 = 0x01;

/// Exit code when the role is not its own process-group leader: refuse to arm
/// before any acknowledgement can be written.
///
/// Public so end-to-end harnesses can assert the exact refusal status instead
/// of treating "any unsuccessful exit" as proof of refusal-before-ack.
pub const WATCHDOG_REFUSAL_EXIT: i32 = 74;
/// Exit code for a malformed role invocation (missing or unusable ack fd).
#[cfg(unix)]
const BAD_ARGV_EXIT: i32 = 64;
/// Exit code when the acknowledgement write fails.
#[cfg(unix)]
const ACK_WRITE_EXIT: i32 = 65;
/// Defensive exit code after the group `SIGKILL` (the process normally dies
/// from that signal first).
#[cfg(unix)]
const AFTER_KILL_EXIT: i32 = 137;

/// Set by [`maybe_run`] once the hook is installed. Release/acquire ordering
/// publishes the executable recorded in [`EXE`].
#[cfg(unix)]
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// Executable path captured by [`maybe_run`] at startup. Linux supervision
/// ignores it in favour of the literal `/proc/self/exe`; other Unix targets
/// re-exec this captured path.
#[cfg(unix)]
static EXE: OnceLock<PathBuf> = OnceLock::new();

/// Install the watchdog hook.
///
/// MUST be the first statement of a synchronous `main`, before any argument
/// parsing or artifact self-detection. When the process was re-executed by
/// group supervision with [`ROLE_MARKER`] as `argv[1]`, this function becomes
/// the watchdog and never returns; otherwise it records the hook and returns.
///
/// # Panics
///
/// Never panics. A malformed role invocation exits the process non-zero
/// without writing the acknowledgement.
///
/// On non-Unix targets this is a documented no-op: the hook cannot run there
/// because there are no process groups to supervise.
pub fn maybe_run() {
    #[cfg(unix)]
    {
        let args: Vec<OsString> = std::env::args_os().skip(1).collect();

        // Record the hook in BOTH branches, before any role action. The path is
        // captured once; later re-exec alters `/proc/self/exe`, so this is the
        // stable "at startup" value used on non-Linux Unix.
        if let Ok(exe) = std::env::current_exe() {
            let _ = EXE.set(exe);
        }
        HOOK_INSTALLED.store(true, Ordering::Release);

        match classify_args(&args) {
            RoleInvocation::NotRole => {}
            RoleInvocation::Role(ack_fd) => run_role(ack_fd),
            RoleInvocation::MissingAckFd | RoleInvocation::InvalidAckFd => {
                std::process::exit(BAD_ARGV_EXIT);
            }
        }
    }
}

/// Recorded executable path, or `None` when [`maybe_run`] was never called.
#[cfg(unix)]
pub(crate) fn hook_executable() -> Option<&'static Path> {
    if !HOOK_INSTALLED.load(Ordering::Acquire) {
        return None;
    }
    EXE.get().map(PathBuf::as_path)
}

/// Parsed shape of the arguments after `argv[0]`.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoleInvocation {
    /// Normal embedder argv: install the hook and return.
    NotRole,
    /// Watchdog role with a usable acknowledgement descriptor.
    Role(RawFd),
    /// Role marker present but no `argv[2]`.
    MissingAckFd,
    /// Role marker present, `argv[2]` unparsable or negative.
    InvalidAckFd,
}

/// Pure argv classifier, kept separate from the process-global side effects so
/// the role contract is unit-testable without forking.
#[cfg(unix)]
fn classify_args(args: &[OsString]) -> RoleInvocation {
    if args.first().map(OsString::as_os_str) != Some(OsStr::new(ROLE_MARKER)) {
        return RoleInvocation::NotRole;
    }
    match args.get(1).and_then(|arg| arg.to_str()) {
        Some(text) => match text.parse::<RawFd>() {
            Ok(fd) if fd >= 0 => RoleInvocation::Role(fd),
            _ => RoleInvocation::InvalidAckFd,
        },
        None => RoleInvocation::MissingAckFd,
    }
}

/// True when this process does not lead its own process group, i.e. the role
/// must refuse to arm (a later `kill(0, SIGKILL)` would hit foreign processes).
#[cfg(unix)]
fn refuses_when_not_group_leader() -> bool {
    // SAFETY: getpgrp and getpid have no preconditions.
    let pgid = unsafe { libc::getpgrp() };
    // SAFETY: getpid has no preconditions.
    let pid = unsafe { libc::getpid() };
    pgid != pid
}

/// Execute the watchdog role. Never returns.
#[cfg(unix)]
fn run_role(ack_fd: RawFd) -> ! {
    if refuses_when_not_group_leader() {
        // Refuse BEFORE the ack: the parent then fails closed on arming.
        std::process::exit(WATCHDOG_REFUSAL_EXIT);
    }

    // SAFETY: changing a signal disposition to SIG_IGN has no memory safety
    // preconditions and is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }

    let byte = ACK_BYTE;
    // SAFETY: ack_fd is the descriptor supervision duplicated to fd 9 in
    // pre_exec; writing one byte to a valid descriptor is safe.
    let written = unsafe { libc::write(ack_fd, (&byte as *const u8).cast(), 1) };
    if written != 1 {
        std::process::exit(ACK_WRITE_EXIT);
    }

    // EOF on the liveness pipe means the owning process is gone. Reading via
    // std keeps the unsafe surface to the signal/ack calls above.
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);

    // SAFETY: pid 0 addresses every process in the caller's process group;
    // the leader check above guarantees no foreign group is signalled.
    unsafe {
        libc::kill(0, libc::SIGKILL);
    }

    std::process::exit(AFTER_KILL_EXIT);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn non_role_argv_is_not_a_role_invocation() {
        // The unit-test harness argv shape: no role marker in any position.
        assert_eq!(
            classify_args(&args(&["--test-threads", "1", "--nocapture"])),
            RoleInvocation::NotRole
        );
        assert_eq!(classify_args(&[]), RoleInvocation::NotRole);
        // The marker only counts as argv[1].
        assert_eq!(
            classify_args(&args(&["--nocapture", ROLE_MARKER, "9"])),
            RoleInvocation::NotRole
        );
    }

    #[test]
    fn role_argv_parses_the_ack_fd() {
        assert_eq!(
            classify_args(&args(&[ROLE_MARKER, "9"])),
            RoleInvocation::Role(9)
        );
        assert_eq!(
            classify_args(&args(&[ROLE_MARKER, "0"])),
            RoleInvocation::Role(0)
        );
    }

    #[test]
    fn role_marker_parsing_rejects_missing_ack_fd() {
        assert_eq!(
            classify_args(&args(&[ROLE_MARKER])),
            RoleInvocation::MissingAckFd
        );
    }

    #[test]
    fn role_marker_parsing_rejects_unusable_ack_fd() {
        assert_eq!(
            classify_args(&args(&[ROLE_MARKER, "not-a-fd"])),
            RoleInvocation::InvalidAckFd
        );
        assert_eq!(
            classify_args(&args(&[ROLE_MARKER, "-1"])),
            RoleInvocation::InvalidAckFd
        );
    }

    /// The test harness (and therefore every ordinary cargo test process) is
    /// not a process-group leader, which is exactly the state the role must
    /// refuse to arm from.
    #[test]
    fn harness_is_not_a_group_leader_so_the_role_refuses() {
        assert!(
            refuses_when_not_group_leader(),
            "test harness unexpectedly leads its own process group; \
             the refuse-to-arm precondition is no longer exercised"
        );
    }
}
