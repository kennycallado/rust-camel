//! audit.rs — one structured audit event per execution (security-relevant).

use std::time::Duration;

pub struct ExecAuditEvent<'a> {
    pub route_id: &'a str,
    pub profile: &'a str,
    pub canonical_executable: &'a str,
    pub args: &'a [String],
    pub env_keys: Vec<&'a str>, // keys only, never values
    pub cwd: &'a str,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub duration: Duration,
}

pub(crate) fn emit(ev: &ExecAuditEvent<'_>) {
    // I-3: redacted summary at info! (never leak args — they are a primary injection
    // vector in the agentic threat model); full command line at debug! only.
    tracing::info!(
        route_id = ev.route_id,
        profile = ev.profile,
        exe = ev.canonical_executable,
        arg_count = ev.args.len(),
        env_keys = ?ev.env_keys,
        cwd = ev.cwd,
        exit_code = ?ev.exit_code,
        timed_out = ev.timed_out,
        stdout_truncated = ev.stdout_truncated,
        stderr_truncated = ev.stderr_truncated,
        dur_ms = ev.duration.as_millis() as u64,
        "exec audit"
    );
    tracing::debug!(
        route_id = ev.route_id,
        exe = ev.canonical_executable,
        args = ?ev.args,
        "exec command line"
    );
}
