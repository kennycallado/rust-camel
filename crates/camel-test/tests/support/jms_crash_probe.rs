#![allow(dead_code)] // not every test binary including `support` uses this module

//! Lifecycle and configuration for the Phase-5 JMS crash probe.
//!
//! # Topology
//!
//! ```text
//! jms_test (parent)
//! └── probe (process-group leader; its group holds ONLY the probe)
//!     └── owns JmsBridgePool → BridgeProcess → GroupGuard
//!         ├── watchdog `sh` (own group leader; holds the liveness-pipe read end)
//!         └── jms-bridge (execs into the watchdog group: setpgid(0, watchdog_pgid))
//! ```
//!
//! The probe is spawned as a fresh process-group leader so the test can signal
//! exactly that group. The probe process owns the bridge (`JmsBridgePool` →
//! `BridgeProcess` → `GroupGuard`), but the bridge does NOT live in the probe
//! group: at exec it joins the watchdog-led supervisor process group
//! (`setpgid(0, watchdog_pgid)`). The watchdog holds the read end of an
//! `O_CLOEXEC` liveness pipe whose write end only the probe process holds. When
//! the probe dies — even by `SIGKILL` — the write end closes, the watchdog's
//! `read` returns EOF, and the watchdog `SIGKILL`s its own group.
//!
//! What dies when the probe dies:
//!
//! - nothing in the probe group: the probe was its only member, and it is the
//!   process that died;
//! - everything in the supervisor group: the watchdog and the Java bridge.
//!
//! The test therefore learns the bridge pid and the supervisor pgid from the
//! handoff artifact the probe publishes at
//! `<XDG_STATE_HOME>/bridge-handoff-<probe_pid>.json`, then proves the bridge
//! died with the probe rather than surviving as an orphan.
//!
//! Every input travels through the child environment; the parent test never
//! mutates the global environment.
//!
//! # Handoff schema
//!
//! The probe writes one JSON object, published atomically before its marker:
//! `{"probe_pid":u32,"bridge_pid":u32,"supervisor_pgid":i32}`. Parsing it is
//! not enough: [`read_bridge_handoff`] also cross-checks the bridge pid's
//! liveness, executable, and process group, so a stale or recycled pid can
//! never be mistaken for the owned bridge.

use std::path::{Path, PathBuf};
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
    /// group assertions must read it through this accessor, never re-derive it.
    pub fn pgid(&self) -> Option<i32> {
        self.pgid
    }

    /// Kill only the process-group leader (the probe itself). Descendants that
    /// stayed in the probe group keep running, and the stored pgid is NOT
    /// disarmed, so `kill_group`/`Drop` still reaches them later.
    ///
    /// Phase 5.2 requires a leader-only kill: the bridge is not in this group
    /// (it joined the watchdog-led supervisor group at exec), so it dies only
    /// when the probe's death closes the liveness pipe.
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
/// `kill(-pgid)` reaps it and every descendant that stayed in that group. The
/// crash-probe bridge leaves the group at exec (see the module topology), so it
/// is killed by its watchdog instead.
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
/// `/proc/<pid>/stat`. Used to prove the watchdog-left supervisor group empties
/// after the probe dies and that the probe group holds no leftovers after
/// cleanup. Excludes the just-killed (zombie) leader.
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

/// Path of the bridge topology handoff the probe publishes:
/// `<xdg_state_home>/bridge-handoff-<probe_pid>.json`.
#[cfg(unix)]
pub fn bridge_handoff_path(xdg_state_home: &Path, probe_pid: u32) -> PathBuf {
    xdg_state_home.join(format!("bridge-handoff-{probe_pid}.json"))
}

/// The bridge topology a probe published in its handoff artifact.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeHandoff {
    /// Pid of the Java bridge owned by the probe.
    pub bridge_pid: i32,
    /// Process-group id of the watchdog-led group the bridge joined.
    pub supervisor_pgid: i32,
}

/// Read and validate the handoff at `path` for the probe whose pid is
/// `expected_probe_pid` and whose bridge runs `expected_exe`.
///
/// Validation order:
///
/// 1. the file is readable and parses to an object with numeric `probe_pid`,
///    `bridge_pid`, and `supervisor_pgid`;
/// 2. the recorded `probe_pid` equals `expected_probe_pid` (a mismatch is a
///    stale artifact from another probe);
/// 3. the bridge pid is alive and not a zombie;
/// 4. `/proc/<bridge_pid>/exe` canonicalizes to `expected_exe`;
/// 5. the bridge's process group is the recorded supervisor group and that
///    group is NOT the probe group.
///
/// Step 5 is the topology proof: the bridge must be in the watchdog-led
/// supervisor group, never in the probe group the test is about to kill. Every
/// error is an actionable one-liner naming the artifact, the offending value,
/// and the expected one.
#[cfg(unix)]
pub fn read_bridge_handoff(
    path: &Path,
    expected_probe_pid: u32,
    expected_exe: &Path,
) -> Result<BridgeHandoff, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "bridge handoff {} is absent or unreadable ({e}) for probe group {expected_probe_pid}; the supervisor group is unknown — was the handoff published before the marker?",
            path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "bridge handoff {} is not a JSON object ({e}) for probe group {expected_probe_pid}; the supervisor group is unknown — expected {{\"probe_pid\",\"bridge_pid\",\"supervisor_pgid\"}}",
            path.display()
        )
    })?;
    let invalid = |field: &str, got: &dyn std::fmt::Display| {
        format!(
            "bridge handoff {} has no numeric {field} (got {got}) for probe group {expected_probe_pid}; the supervisor group is unknown — expected a JSON number",
            path.display()
        )
    };
    let parsed_probe_pid = value
        .get("probe_pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| invalid("probe_pid", &value.get("probe_pid").unwrap_or(&value)))?;
    let bridge_pid_u32 = value
        .get("bridge_pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| invalid("bridge_pid", &value.get("bridge_pid").unwrap_or(&value)))?;
    let supervisor_pgid = value
        .get("supervisor_pgid")
        .and_then(serde_json::Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| {
            invalid(
                "supervisor_pgid",
                &value.get("supervisor_pgid").unwrap_or(&value),
            )
        })?;
    let bridge_pid =
        i32::try_from(bridge_pid_u32).map_err(|_| invalid("bridge_pid", &bridge_pid_u32))?;

    if parsed_probe_pid != expected_probe_pid {
        return Err(format!(
            "bridge handoff {} is stale: it names probe group {parsed_probe_pid} but the expected probe group {expected_probe_pid} is the one being killed; its supervisor group {supervisor_pgid} belongs to another probe",
            path.display()
        ));
    }

    let stat = std::fs::read_to_string(format!("/proc/{bridge_pid}/stat")).map_err(|e| {
        format!(
            "bridge handoff {}: bridge pid {bridge_pid} (probe group {expected_probe_pid}) is not alive ({e}); the liveness-pipe watchdog likely already killed supervisor group {supervisor_pgid}",
            path.display()
        )
    })?;
    let state = stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .ok_or_else(|| {
            format!(
                "bridge handoff {}: cannot read the process state of bridge pid {bridge_pid} (probe group {expected_probe_pid}); expected a live member of supervisor group {supervisor_pgid}",
                path.display()
            )
        })?;
    if state == "Z" {
        return Err(format!(
            "bridge handoff {}: bridge pid {bridge_pid} (probe group {expected_probe_pid}) is a zombie; expected a live member of supervisor group {supervisor_pgid}",
            path.display()
        ));
    }

    let actual_exe = std::fs::read_link(format!("/proc/{bridge_pid}/exe")).map_err(|e| {
        format!(
            "bridge handoff {}: cannot resolve /proc/{bridge_pid}/exe ({e}) for probe group {expected_probe_pid}; expected {}",
            path.display(),
            expected_exe.display()
        )
    })?;
    let want = std::fs::canonicalize(expected_exe).unwrap_or_else(|_| expected_exe.to_path_buf());
    let got = std::fs::canonicalize(&actual_exe).unwrap_or(actual_exe);
    if got != want {
        return Err(format!(
            "bridge handoff {}: bridge pid {bridge_pid} (probe group {expected_probe_pid}) runs {} but the expected executable is {}",
            path.display(),
            got.display(),
            want.display()
        ));
    }

    let actual_pgid = process_group_of(bridge_pid).ok_or_else(|| {
        format!(
            "bridge handoff {}: cannot read the process group of bridge pid {bridge_pid}; expected supervisor group {supervisor_pgid} and probe group {expected_probe_pid}",
            path.display()
        )
    })?;
    if actual_pgid != supervisor_pgid {
        return Err(format!(
            "bridge handoff {}: bridge pid {bridge_pid} is in process group {actual_pgid}, not the recorded supervisor group {supervisor_pgid}; probe group {expected_probe_pid} must not hold the bridge",
            path.display()
        ));
    }
    if supervisor_pgid == expected_probe_pid as i32 {
        return Err(format!(
            "bridge handoff {}: supervisor group {supervisor_pgid} equals probe group {expected_probe_pid}; the bridge must live in the watchdog-led group, not the probe group",
            path.display()
        ));
    }

    Ok(BridgeHandoff {
        bridge_pid,
        supervisor_pgid,
    })
}
