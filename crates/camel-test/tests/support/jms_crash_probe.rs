#![allow(dead_code)] // not every test binary including `support` uses this module

//! Lifecycle and configuration for the Phase-5 JMS crash probe.
//!
//! The probe is spawned as a fresh process-group leader so the test can reap it
//! together with the Java bridge it spawns. Every input travels through the
//! child environment; the parent test never mutates the global environment.

use std::path::Path;
use std::time::Duration;

/// Owns the crash-probe child and the process group it leads.
///
/// The process-group id (the spawned leader's pid, from `process_group(0)`) is
/// captured at spawn and stored — it is deliberately NOT re-derived from
/// `Child::id()`. `Child::id()` returns `None` once the child has been reaped,
/// which would skip group cleanup and leak a surviving descendant such as the
/// Java bridge. Cleanup disarms the stored pgid with `take()`, so an explicit
/// cleanup followed by `Drop` is idempotent and never signals a recycled group.
pub struct ProbeGuard {
    child: tokio::process::Child,
    #[cfg_attr(not(unix), allow(dead_code))]
    pgid: Option<i32>,
}

impl ProbeGuard {
    pub fn new(child: tokio::process::Child) -> Self {
        // Capture the owned group id before any wait can reap the leader.
        let pgid = child.id().map(|pid| pid as i32);
        Self { child, pgid }
    }

    /// The owned process-group id captured at spawn, if any. `Child::id()`
    /// returns `None` after a wait, so callers that need the group id for
    /// orphan assertions must read it through this accessor, never re-derive it.
    pub fn pgid(&self) -> Option<i32> {
        self.pgid
    }

    /// Kill only the process-group leader (the probe itself), leaving every
    /// descendant — notably the Java bridge — alive. The stored pgid is NOT
    /// disarmed, so `kill_group`/`Drop` still reap the orphan later.
    ///
    /// Phase 5.2 requires this: killing the whole group at the handoff barrier
    /// would kill the orphan bridge before the survivor has acquired the
    /// journal and observed the inherited count.
    pub fn kill_leader_only(&mut self) {
        let Some(pgid) = self.pgid else {
            return;
        };
        #[cfg(unix)]
        if pgid > 0 {
            // SAFETY: a positive pid signals exactly the leader; `pgid` is the
            // spawned leader pid captured at spawn, never 0.
            unsafe {
                libc::kill(pgid, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        let _ = pgid;
        let _ = self.child.start_kill();
    }

    /// Kill the probe's process group and reap the direct child.
    pub async fn kill_and_reap(&mut self) {
        self.kill_group();
        let _ = self.child.wait().await;
    }

    /// Wait up to `timeout` for the probe to exit on its own. Returns `None`
    /// when the child is still running at the deadline. Reaping the leader here
    /// does not disable the guard: the stored pgid still reaches descendants.
    pub async fn wait_bounded(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        tokio::time::timeout(timeout, self.child.wait())
            .await
            .ok()
            .and_then(Result::ok)
    }

    /// True while the probe is still running.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Signal the owned process group, then the direct child (best-effort).
    /// `take()` disarms the pgid, making repeated cleanup a no-op.
    fn kill_group(&mut self) {
        let Some(pgid) = self.pgid.take() else {
            return;
        };
        #[cfg(unix)]
        if pgid > 0 {
            // SAFETY: a negative pid signals the whole process group; `pgid` is
            // the spawned leader pid captured at spawn and is never 0, so the
            // test's own process group cannot be addressed by accident.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        let _ = pgid;
        let _ = self.child.start_kill();
    }
}

impl Drop for ProbeGuard {
    fn drop(&mut self) {
        self.kill_group();
    }
}

/// Spawn `command` as a fresh process-group leader wrapped in a [`ProbeGuard`].
///
/// `process_group(0)` makes the child its own group leader, so a single
/// `kill(-pgid)` reaps it and every descendant it spawned.
pub fn spawn_group_leader(mut command: tokio::process::Command) -> ProbeGuard {
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    ProbeGuard::new(command.spawn().expect("spawn process-group leader"))
}

/// Spawn the crash probe in its default `probe` role. All Classic inputs
/// (broker URL, bridge binary, broker type, fixture credentials, XDG state
/// root, queue) travel through the child environment; the marker path is the
/// single argv. The parent test never mutates the global environment
/// (`set_var` races under parallel test threads).
pub fn spawn_probe(
    binary: &Path,
    broker_url: &str,
    xdg_state_home: &Path,
    queue: &str,
    marker: &Path,
) -> ProbeGuard {
    spawn_probe_role(binary, broker_url, xdg_state_home, queue, marker, "probe")
}

/// Spawn the crash probe in an explicit role (`probe` or `survivor`).
///
/// Phase 5.2 needs a second, survivor-shaped child: it consumes the same queue
/// in the same journal domain, writes a receipt (message id, body, reservation
/// count, headers) from its first accepted exchange, and then blocks so no
/// `COMMIT` is ever observed. `CAMEL_JMS_CRASH_ROLE` selects the behavior; the
/// artifact path is the single argv.
pub fn spawn_probe_role(
    binary: &Path,
    broker_url: &str,
    xdg_state_home: &Path,
    queue: &str,
    artifact: &Path,
    role: &str,
) -> ProbeGuard {
    spawn_probe_configured(
        binary,
        &ProbeBroker {
            url: broker_url,
            broker_type_env: "activemq",
            username: super::activemq::ACTIVEMQ_USER,
            password: super::activemq::ACTIVEMQ_PASSWORD,
        },
        xdg_state_home,
        queue,
        artifact,
        role,
    )
}

/// Broker identity and explicit credentials for a configured probe spawn.
///
/// The Phase-5 crash proof must spawn the same role against Classic and
/// Artemis with identical journal-domain inputs, so the broker type and
/// credentials travel here rather than being hardcoded per spawn.
pub struct ProbeBroker<'a> {
    pub url: &'a str,
    /// `CAMEL_JMS_BROKER_TYPE` value: `activemq` or `artemis`.
    pub broker_type_env: &'a str,
    pub username: &'a str,
    pub password: &'a str,
}

/// Spawn the crash probe in an explicit role against an explicit broker type
/// and credentials. Mirrors [`spawn_probe_role`] but lets the caller pin
/// Artemis (or another broker) without touching the Classic defaults.
pub fn spawn_probe_configured(
    binary: &Path,
    broker: &ProbeBroker<'_>,
    xdg_state_home: &Path,
    queue: &str,
    artifact: &Path,
    role: &str,
) -> ProbeGuard {
    spawn_probe_configured_with_logs(binary, broker, xdg_state_home, queue, artifact, role, None)
}

/// Like [`spawn_probe_configured`] but also captures the spawned bridge's
/// native log into `log_dir`.
///
/// The bridge writes its console log to stdout by default (verified against
/// the 0.7.1 native binary), so `CAMEL_BRIDGE_LOG_STDERR` alone would capture
/// nothing. Setting `QUARKUS_LOG_CONSOLE_STDERR=true` moves the Quarkus
/// console handler to stderr; `camel-bridge` then redirects that stderr to
/// `<log_dir>/jms-bridge-<probe-pid>.log` (the probe process id, matching
/// `BridgeProcess::start`). Both variables travel on the probe's child
/// environment, so the parent test process never mutates its own environment.
pub fn spawn_probe_configured_with_logs(
    binary: &Path,
    broker: &ProbeBroker<'_>,
    xdg_state_home: &Path,
    queue: &str,
    artifact: &Path,
    role: &str,
    log_dir: Option<&Path>,
) -> ProbeGuard {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_jms_crash_probe"));
    command
        .arg(artifact)
        .env("CAMEL_JMS_BROKER_URL", broker.url)
        .env("CAMEL_JMS_BRIDGE_BINARY_PATH", binary)
        .env("CAMEL_JMS_BROKER_TYPE", broker.broker_type_env)
        .env("CAMEL_JMS_CRASH_ROLE", role)
        .env("CAMEL_JMS_USERNAME", broker.username)
        .env("CAMEL_JMS_PASSWORD", broker.password)
        .env("XDG_STATE_HOME", xdg_state_home)
        .env("CAMEL_JMS_CRASH_QUEUE", queue);
    if let Some(log_dir) = log_dir {
        command
            .env("QUARKUS_LOG_CONSOLE_STDERR", "true")
            .env("CAMEL_BRIDGE_LOG_STDERR", log_dir);
    }
    spawn_group_leader(command)
}

/// The live, non-zombie process ids in the process group `pgid`, read from
/// `/proc/<pid>/stat`. Used to prove an orphan bridge survives the probe kill
/// and is gone after cleanup. Excludes the just-killed (zombie) leader.
#[cfg(unix)]
pub fn live_group_members(pgid: i32) -> Vec<i32> {
    let mut members = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return members;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        // comm is parenthesized and may contain spaces; parse the fields after
        // the closing paren: state, ppid, pgrp, ...
        let Some((_, rest)) = stat.rsplit_once(')') else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let state = fields.next().unwrap_or("?");
        let _ppid = fields.next();
        let group = fields.next().and_then(|f| f.parse::<i32>().ok());
        if group == Some(pgid) && state != "Z" {
            members.push(pid);
        }
    }
    members
}

/// The executable behind `pid` (`/proc/<pid>/exe`), if resolvable.
#[cfg(unix)]
pub fn process_exe(pid: i32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// The process-group id of `pid`, read from `/proc/<pid>/stat`.
#[cfg(unix)]
pub fn process_group_of(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    let _ppid = fields.next()?;
    fields.next()?.parse::<i32>().ok()
}

/// The live process in process group `pgid` that runs `exe`, excluding
/// `exclude` (the leader). This identifies the EXACT bridge that owns the
/// destination journal — not merely any group member.
#[cfg(unix)]
pub fn find_group_process_by_exe(pgid: i32, exe: &std::path::Path, exclude: i32) -> Option<i32> {
    let want = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    for pid in live_group_members(pgid) {
        if pid == exclude || process_group_of(pid) != Some(pgid) {
            continue;
        }
        let Some(got) = process_exe(pid) else {
            continue;
        };
        let got = std::fs::canonicalize(&got).unwrap_or(got);
        if got == want {
            return Some(pid);
        }
    }
    None
}
