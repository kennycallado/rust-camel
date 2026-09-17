//! End-to-end coexistence test: a `camel job` runs next to a live
//! `camel run` that holds the shared runtime journal (the redb lock on
//! `journal.db`) and the shared Prometheus and health listener ports,
//! all from one ambient `Camel.toml`. The job's boot projection
//! (jobcoexist) neutralizes the runtime journal and the observability
//! stack, so the job boots and completes instead of dying with exit 2
//! on the journal lock or on a diagnostic-port bind conflict. Without
//! the projection this test fails with exit 2.
//!
//! ADR-0070 subprocess exception: the spawned `camel run` cannot
//! receive a staged socket, so this test probes free ports with
//! `TcpListener::bind("127.0.0.1:0")` and drops the listeners before
//! spawning. The failure mode is loud (the server exits on a bind
//! error) and the full flow is retried once on exactly that failure.
//!
//! This is the 26th camel-cli integration-test binary (the gate count
//! moves 25 → 26).

mod common;

use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use common::{run_binary, spawn_camel_job, spawn_camel_run, spawn_drained, wait_exit_code_bounded};

/// The Linux bind-failure text: the loud, retriable signature of the
/// ADR-0070 port-probe race (another process grabbed the probed port
/// between the listener drop and the server's bind).
const BIND_RACE_MARK: &str = "Address already in use";

/// Exit ceiling for one spawned job, 90 s per the `common/mod.rs`
/// generous-deadline convention and above the fixture's 60 s job
/// timeout, so a hung job is force-killed and reported as `-1` instead
/// of hanging the test. The happy path returns as soon as both jobs
/// exit.
const JOB_WAIT: Duration = Duration::from_secs(90);

/// Write the ambient config shared by the server and the job: the
/// route glob, logs off (the job's stdout must carry only the JSON
/// report), no watch, the shared runtime journal, and both diagnostic
/// listeners on the reserved ports.
fn write_shared_config(dir: &Path, prom_port: u16, health_port: u16) {
    std::fs::write(
        dir.join("Camel.toml"),
        format!(
            r#"[default]
routes = ["routes/*.yaml"]
log_level = "off"
watch = false

[default.runtime_journal]
path = "journal.db"
durability = "immediate"

[default.observability.prometheus]
enabled = true
host = "127.0.0.1"
port = {prom_port}

[default.observability.health]
enabled = true
host = "127.0.0.1"
port = {health_port}
handler_timeout_ms = 6000
"#,
        ),
    )
    .expect("write Camel.toml");
}

/// Reserve two free ports by binding ephemeral listeners and dropping
/// them (ADR-0070 subprocess exception — see the module header).
fn reserve_two_ports() -> (u16, u16) {
    let first = TcpListener::bind("127.0.0.1:0").expect("probe bind 1");
    let second = TcpListener::bind("127.0.0.1:0").expect("probe bind 2");
    let prom_port = first.local_addr().expect("probe addr 1").port();
    let health_port = second.local_addr().expect("probe addr 2").port();
    drop(first);
    drop(second);
    (prom_port, health_port)
}

/// Is the TCP port accepting connections?
fn port_open(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// One full coexistence flow: fixture with reserved ports, live
/// `camel run`, readiness poll, `camel job ping`, report assertion.
/// `Err` carries the failure text; the bind-race signature inside it
/// makes the caller retry the whole flow once.
fn coexistence_flow() -> Result<(), String> {
    let dir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let (prom_port, health_port) = reserve_two_ports();
    write_shared_config(dir.path(), prom_port, health_port);
    std::fs::create_dir(dir.path().join("routes")).map_err(|e| format!("mkdir routes: {e}"))?;
    std::fs::create_dir(dir.path().join("jobs")).map_err(|e| format!("mkdir jobs: {e}"))?;
    std::fs::write(
        dir.path().join("routes/ping.yaml"),
        r#"routes:
  - id: "ping"
    from: "direct:ping"
    steps:
      - to: "log:ping"
"#,
    )
    .map_err(|e| format!("write route: {e}"))?;
    std::fs::write(
        dir.path().join("jobs/ping.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:ping
    body: "ping"
routeFilesFromRoot:
  - routes/ping.yaml
"#,
    )
    .map_err(|e| format!("write job doc: {e}"))?;

    let mut server = spawn_camel_run(dir.path());
    let server_out = spawn_drained(&mut server);

    // Readiness: both diagnostic ports accept TCP connections (30 s
    // deadline, 100 ms interval). Bail out loud if the server dies on
    // the way — a bind error is the retriable port-probe race.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(Some(status)) = server.try_wait() {
            let captured = server_out.finish();
            return Err(format!("server exited early ({status:?}):\n{captured}"));
        }
        if port_open(prom_port) && port_open(health_port) {
            break;
        }
        if Instant::now() >= deadline {
            // Kill FIRST: `finish` joins the pipe-drain threads, which
            // would block forever on a still-running server's open
            // pipes (a hang, not a failure).
            let _ = server.kill();
            let _ = server.wait();
            let captured = server_out.finish();
            return Err(format!(
                "server not ready within 30s (prom :{prom_port}, health :{health_port}):\n{captured}"
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }

    // The server stayed alive through boot: it now holds the redb lock
    // on journal.db and both listener ports.
    if let Ok(Some(status)) = server.try_wait() {
        return Err(format!("server exited right after readiness ({status:?})"));
    }

    // The job runs in the same directory against the same ambient
    // config. With the boot projection it completes; without it, its
    // journal open or port bind would fail boot with exit 2.
    let exe = Path::new(env!("CARGO_BIN_EXE_camel"));
    let (code, stdout, stderr) = run_binary(dir.path(), exe, &["job", "ping"], &[]);
    if code != 0 {
        return Err(format!(
            "camel job ping exited {code};\nstdout:\n{stdout}\nstderr:\n{stderr}"
        ));
    }
    let report: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        format!("job stdout is not the JSON report ({e}):\n{stdout}\nstderr:\n{stderr}")
    })?;
    if report["outcome"] != "Completed" {
        return Err(format!("job outcome is not Completed: {report}"));
    }

    // The server stayed alive while the job ran, closing the
    // "throughout" segment between readiness and job completion.
    if let Ok(Some(status)) = server.try_wait() {
        return Err(format!("server died while the job ran ({status:?})"));
    }

    // Kill the server; the kill-on-drop guard is the backstop.
    let _ = server.kill();
    let _ = server.wait();
    Ok(())
}

/// The lock-based proof: with a live server holding the journal redb
/// lock and both reserved ports, the job still exits 0 with a
/// `Completed` JSON report — any journal open or port bind attempt by
/// the job would have failed its boot with exit 2.
#[test]
fn job_coexists_with_live_server_holding_journal_and_ports() {
    match coexistence_flow() {
        Ok(()) => {}
        // ADR-0070 port-probe race: retry the full flow exactly once.
        Err(e) if e.contains(BIND_RACE_MARK) => {
            if let Err(retry) = coexistence_flow() {
                panic!("coexistence flow failed on retry after bind race: {retry}");
            }
        }
        Err(e) => panic!("coexistence flow failed: {e}"),
    }
}

// ── Concurrent jobs + malformed ambient config (jobcoexist task 4) ────
//
// The two tests below reuse the task-3 fixture helpers above: the
// concurrency test shares `write_shared_config` + `reserve_two_ports`
// (the projection is proven by two overlapping job boots on one
// ambient config), and the malformed test pins the fail-loud config
// ordering with the same `camel job` entrypoint.

/// Fixture whose worker pipeline holds each exchange ~2 s: a job
/// document `jobs/slow.job.yaml` sending to `seda:work` (the runner
/// forces `waitForTaskToComplete=Always` on seda targets, so the send
/// blocks for the whole delay) plus the `seda:work` consumer route in
/// `routes/slow.yaml`. The 2 s delay keeps two back-to-back job
/// processes overlapping through their whole contended window.
fn write_delay_job(dir: &Path) {
    std::fs::create_dir(dir.join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.join("routes/slow.yaml"),
        r#"routes:
  - id: "slow"
    from: "seda:work"
    steps:
      - delay: 2000
      - to: "log:slow"
"#,
    )
    .expect("write slow route");
    std::fs::create_dir(dir.join("jobs")).expect("mkdir jobs");
    std::fs::write(
        dir.join("jobs/slow.job.yaml"),
        r#"execute:
  mode: one-shot
  timeout: 60s
  send:
    to: seda:work
    body: "slow"
routeFilesFromRoot:
  - routes/slow.yaml
"#,
    )
    .expect("write slow job doc");
}

/// Two `camel job slow` processes launched back-to-back against one
/// ambient config (journal + both listeners enabled) both complete.
/// The boot projection neutralizes the journal and the observability
/// stack in EACH process, so neither opens the redb lock nor binds a
/// listener. The 2 s worker delay keeps the two executions overlapping
/// for the whole contended window: without the projection the second
/// process would exit 2 on the journal lock or the port bind.
#[test]
fn two_concurrent_jobs_share_ambient_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (prom_port, health_port) = reserve_two_ports();
    write_shared_config(dir.path(), prom_port, health_port);
    write_delay_job(dir.path());

    // Back-to-back spawn: the gap is process-spawn latency, far under
    // the 2 s route delay both jobs then block on.
    let mut first = spawn_camel_job(dir.path(), Path::new("slow"));
    let mut second = spawn_camel_job(dir.path(), Path::new("slow"));
    let first_drained = spawn_drained(&mut first);
    let second_drained = spawn_drained(&mut second);
    let first_bufs = first_drained.markers();
    let second_bufs = second_drained.markers();

    // Bounded waits: a hung job is force-killed and reaped, then
    // surfaces as the `-1` sentinel in the assertion below.
    let first_code = wait_exit_code_bounded(&mut first, JOB_WAIT);
    let second_code = wait_exit_code_bounded(&mut second, JOB_WAIT);
    let _ = first_drained.finish();
    let _ = second_drained.finish();

    let first_stdout = first_bufs[0].lock().expect("first stdout lock").clone();
    let second_stdout = second_bufs[0].lock().expect("second stdout lock").clone();
    let first_stderr = first_bufs[1].lock().expect("first stderr lock").clone();
    let second_stderr = second_bufs[1].lock().expect("second stderr lock").clone();

    assert_eq!(
        first_code, 0,
        "first job must complete; stdout:\n{first_stdout}\nstderr:\n{first_stderr}"
    );
    assert_eq!(
        second_code, 0,
        "second job must complete; stdout:\n{second_stdout}\nstderr:\n{second_stderr}"
    );
    for (label, stdout, stderr) in [
        ("first", &first_stdout, &first_stderr),
        ("second", &second_stdout, &second_stderr),
    ] {
        let report: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!("{label} job stdout is not the JSON report ({e}):\n{stdout}\nstderr:\n{stderr}")
        });
        assert_eq!(report["outcome"], "Completed", "{label} report: {report}");
    }
}

/// A malformed ambient config fails loud: `[runtime_journal]` with an
/// unknown `durability` variant aborts config load with the existing
/// `camel-cli job failed:` diagnostic and exit 2 — BEFORE the boot
/// projection runs, so there is no boot and no report. Pins the
/// fail-loud ordering (parse/validate, then project).
#[test]
fn malformed_ambient_config_fails_loud() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Camel.toml"),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "off"
watch = false

[default.runtime_journal]
path = "journal.db"
durability = "bogus"
"#,
    )
    .expect("write malformed Camel.toml");

    let exe = Path::new(env!("CARGO_BIN_EXE_camel"));
    let (code, stdout, stderr) = run_binary(dir.path(), exe, &["job", "ping"], &[]);
    assert_eq!(
        code, 2,
        "malformed config must exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("camel-cli job failed")
            && stderr.contains("failed to load")
            && stderr.contains("bogus"),
        "stderr must carry the configuration diagnostic; got:\n{stderr}"
    );
    assert!(
        !stdout.contains("outcome"),
        "no boot, no report on a load failure; got stdout:\n{stdout}"
    );
}
