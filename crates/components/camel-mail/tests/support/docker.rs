//! Docker gate and bounded docker invocations for the `MAIL_ITEST` tier.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DOCKER_CMD_TIMEOUT: Duration = Duration::from_secs(30);

pub enum Gate {
    Skip(String),
    Run,
}

pub fn gate(itest: Option<&str>, docker_ok: impl Fn() -> bool) -> Gate {
    match itest {
        None => Gate::Skip(
            "mail integration tests skipped: MAIL_ITEST is not set; run with MAIL_ITEST=1"
                .to_string(),
        ),
        Some(_) => {
            if docker_ok() {
                Gate::Run
            } else {
                panic!("infra-unavailable: mail docker tier requires docker (MAIL_ITEST=1)");
            }
        }
    }
}

pub fn docker_available() -> bool {
    docker_exec(&["info"]).is_ok()
}

/// `true` when the docker tier is active; prints the notice and returns
/// `false` when `MAIL_ITEST` is unset.
pub fn gate_active() -> bool {
    let itest = std::env::var("MAIL_ITEST").ok();
    match gate(itest.as_deref(), docker_available) {
        Gate::Skip(notice) => {
            eprintln!("{notice}");
            false
        }
        Gate::Run => true,
    }
}

/// Run one docker command with a 30 s bound; `Ok(stdout)` on exit 0.
pub fn docker_exec(args: &[&str]) -> Result<String, String> {
    let mut child = Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot spawn docker: {e}"))?;
    let deadline = Instant::now() + DOCKER_CMD_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("docker {args:?} timed out"));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(format!("docker wait failed: {e}")),
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut stdout);
    }
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    if status.success() {
        Ok(stdout)
    } else {
        Err(format!("docker {args:?} failed: {}", stderr.trim()))
    }
}

/// Strictly increasing within the process, so parallel tests never share a
/// container name or subject.
pub fn nanos() -> u128 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let prev = LAST
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |p| Some(now.max(p + 1)))
        .unwrap_or(0);
    u128::from(now.max(prev + 1))
}
