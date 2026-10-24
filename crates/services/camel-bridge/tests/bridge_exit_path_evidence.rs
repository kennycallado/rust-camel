//! Bridge child-lifetime evidence suite (mission 390).
//!
//! `leak_parent_then_exit` reproduces the CI mechanism that hung run
//! 39578427824: a bridge process is spawned with INHERITED stderr, its owning
//! test process leaks the handle (no destructor) and calls `exit(0)`. The
//! orphan keeps the inherited stderr write end open, so a shell pipeline
//! consuming the cargo test output never sees EOF. Pre-fix the assertion fails
//! (orphans survive); post-fix the group watchdog kills the group.
//!
//! The remaining tests are the AFTER exit-path matrix: normal stop, panic
//! unwinding, SIGTERM/SIGINT/SIGKILL parent death, death inside the
//! TERM→escalation teardown window, early watchdog-leader death, caller-runtime
//! retirement, and concurrent spawns. Survivor assertions always go through
//! `running_members()`, which excludes zombies.
//!
//! Every re-executing test uses a role environment variable so the outer and
//! inner halves each run in a real test-binary process. Unfiltered `--ignored`
//! runs never recurse: the outer process is the only one without a role.

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Arc, Barrier};
#[cfg(unix)]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
const LEAK_ROLE: &str = "leak-parent";
#[cfg(unix)]
const LEAK_TEST: &str = "leak_parent_then_exit";
#[cfg(unix)]
const FILE_CAPTURE_ROLE: &str = "file-capture";
#[cfg(unix)]
const FILE_CAPTURE_TEST: &str = "stderr_file_capture_unchanged";

/// Role of the current test process, set by the self-reexec parent.
#[cfg(unix)]
fn role() -> Option<String> {
    std::env::var("BRIDGE_EVIDENCE_ROLE").ok()
}

/// Unique token embedded in the fake bridge script filename.
///
/// The evidence script exports `BRIDGE_EVIDENCE_TOKEN` for one invocation so
/// that its `pgrep` verdict and cleanup only ever see this run's fixtures.
/// Standalone runs (no env) still produce a unique `*-fake-bridge-*` token.
#[cfg(unix)]
fn unique_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let prefix = std::env::var("BRIDGE_EVIDENCE_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "standalone".to_string());
    format!("{prefix}-fake-bridge-{}-{nanos}", std::process::id())
}

/// POSIX single-quote a string for safe interpolation into a shell script.
#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Write an executable fake bridge `sh` script into `dir` and return its path.
///
/// Ordering is part of the contract: pid capture, optional stderr flood, TERM
/// policy, TERM-immune grandchild, bounded arm poll, and only then the
/// `ready` line. `BridgeProcess::start` returning therefore implies the traps
/// are installed and the grandchild is already alive and TERM-immune.
#[cfg(unix)]
fn fake_bridge_script(
    dir: &Path,
    term_ignoring: bool,
    flood_stderr: bool,
    emit_ready: bool,
) -> PathBuf {
    fake_bridge_script_with_post_ready_writes(dir, term_ignoring, flood_stderr, emit_ready, 0)
}

/// Like [`fake_bridge_script`], optionally writing `post_ready_writes` stderr
/// lines at 100 ms intervals after readiness. The write loop stops when the
/// test creates `<dir>/post-ready-stop`; each successful line also records its
/// index in `<dir>/post-ready-ack`. A failed stderr write exits the fixture, so
/// a closed drain reader is observable instead of being masked by an ignored
/// `SIGPIPE`.
#[cfg(unix)]
fn fake_bridge_script_with_post_ready_writes(
    dir: &Path,
    term_ignoring: bool,
    flood_stderr: bool,
    emit_ready: bool,
    post_ready_writes: u32,
) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let token = unique_token();
    let script_path = dir.join(format!("{token}.sh"));
    let pidfile = dir.join("bridge.pid");
    let term_marker = dir.join("term-marker");
    let die_marker = dir.join("die-marker");
    let gc_marker = dir.join("gc-marker");
    let gc_token = format!("{token}-gc");

    let stderr_block = if flood_stderr {
        // 200 KiB in 4 KiB chunks, exactly as specified.
        format!(
            "i=0\nwhile [ \"$i\" -lt 50 ]; do\n  printf '%s' '{}' >&2\n  i=$((i+1))\ndone\n",
            "x".repeat(4096)
        )
    } else {
        String::new()
    };

    let term_block = if term_ignoring {
        format!(
            "trap \"echo term > {}; echo die > {}\" TERM\n",
            shell_quote(&term_marker.to_string_lossy()),
            shell_quote(&die_marker.to_string_lossy()),
        )
    } else {
        "trap '' TERM\n".to_string()
    };

    let terminal_block = if term_ignoring {
        "while :; do :; done\n"
    } else {
        "wait\n"
    };

    let ready_block = if emit_ready {
        "echo '{\"status\":\"ready\",\"port\":4242}'\n".to_string()
    } else {
        String::new()
    };

    let post_ready_block = if post_ready_writes > 0 {
        let stop = shell_quote(&dir.join("post-ready-stop").to_string_lossy());
        let ack = shell_quote(&dir.join("post-ready-ack").to_string_lossy());
        format!(
            "i=0\n\
             while [ \"$i\" -lt {post_ready_writes} ] && [ ! -f {stop} ]; do\n\
               sleep 0.1\n\
               if ! printf '%s\\n' \"post-ready stderr $i\" >&2; then\n\
                 exit 1\n\
               fi\n\
               printf '%s' \"$i\" > {ack}\n\
               i=$((i+1))\n\
             done\n"
        )
    } else {
        String::new()
    };

    let script = format!(
        "#!/bin/sh\n\
         set -u\n\
         echo $$ > {pidfile}\n\
         {stderr_block}\
         {term_block}\
         sh -c \"trap \\\"\\\" TERM; echo armed > {gc_marker}; while :; do sleep 1; done\" {gc_token} &\n\
         gc_pid=$!\n\
         i=0\n\
         while [ \"$i\" -lt 60 ]; do\n\
           if kill -0 \"$gc_pid\" 2>/dev/null && [ -f {gc_marker} ]; then break; fi\n\
           i=$((i+1))\n\
           sleep 0.05\n\
         done\n\
         if ! kill -0 \"$gc_pid\" 2>/dev/null || [ ! -f {gc_marker} ]; then\n\
           echo \"fake bridge grandchild failed to arm\" >&2\n\
           exit 1\n\
         fi\n\
         {ready_block}\
         echo 'fake-bridge stderr alive' >&2\n\
         {post_ready_block}\
         {terminal_block}",
        pidfile = shell_quote(&pidfile.to_string_lossy()),
        stderr_block = stderr_block,
        term_block = term_block,
        gc_marker = shell_quote(&gc_marker.to_string_lossy()),
        gc_token = gc_token,
        ready_block = ready_block,
        post_ready_block = post_ready_block,
        terminal_block = terminal_block,
    );

    std::fs::write(&script_path, script).expect("write fake bridge script");
    let mut perms = std::fs::metadata(&script_path)
        .expect("stat fake bridge script")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).expect("chmod fake bridge script");

    script_path
}

/// Read the process state character (field 3) of `/proc/<pid>/stat`.
///
/// `Ok(None)` means the process no longer exists: `pgrep` saw it, but it exited
/// and was reaped before this read, so `/proc/<pid>/stat` is absent. That is a
/// legitimate race in the bounded cleanup poll, not an inspection failure.
/// Every other failure (permission denied, malformed stat, no state field) is
/// returned as an error.
#[cfg(unix)]
fn process_state(pid: u32) -> Result<Option<char>, String> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("cannot read /proc/{pid}/stat: {err}")),
    };
    // `comm` (field 2) is parenthesised and may contain spaces or `)`, so the
    // state is the first token after the LAST `)`.
    let idx = stat
        .rfind(')')
        .ok_or_else(|| format!("malformed /proc/{pid}/stat: missing comm terminator"))?;
    stat[idx + 1..]
        .split_whitespace()
        .next()
        .and_then(|field| field.chars().next())
        .map(Some)
        .ok_or_else(|| format!("malformed /proc/{pid}/stat: missing state field"))
}

/// Live (non-zombie) processes whose command line matches `token`.
///
/// `pgrep` exit 1 means "no matches" and yields an empty list; every other
/// `pgrep` failure is returned as an error so the assertion can never pass
/// vacuously. A pid that `pgrep` saw but that exited before its stat read is
/// skipped as absent; read or parse failures on a live pid remain errors.
#[cfg(unix)]
fn running_members(token: &str) -> Result<Vec<u32>, String> {
    let output = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(token)
        .output()
        .map_err(|err| format!("failed to spawn pgrep for {token}: {err}"))?;
    match output.status.code() {
        Some(0) => {}
        Some(1) => return Ok(Vec::new()),
        other => {
            return Err(format!(
                "pgrep for {token} failed with status {other:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }

    let mut members = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let pid = line
            .trim()
            .parse::<u32>()
            .map_err(|err| format!("pgrep for {token} produced unparsable pid {line:?}: {err}"))?;
        let Some(state) = process_state(pid)
            .map_err(|err| format!("process inspection failed for {token}: {err}"))?
        else {
            // Exited between the pgrep snapshot and the /proc read: absent, not
            // a survivor.
            continue;
        };
        if state != 'Z' {
            members.push(pid);
        }
    }
    Ok(members)
}

/// `running_members` with inspection failures promoted to test failures.
#[cfg(unix)]
fn inspect_running(token: &str) -> Vec<u32> {
    match running_members(token) {
        Ok(members) => members,
        Err(err) => panic!("process inspection unavailable for {token}: {err}"),
    }
}

/// This evidence suite inspects Linux `/proc/<pid>/stat`; fail loudly elsewhere
/// instead of letting the survivor assertion degenerate.
#[cfg(unix)]
fn require_linux_proc() {
    assert!(
        Path::new("/proc/self/stat").exists(),
        "bridge_exit_path_evidence requires Linux /proc state inspection; \
         this evidence suite is only supported on Linux"
    );
}

/// Config for the fake bridge, reusing the production JMS spec descriptor.
#[cfg(unix)]
fn fake_config(binary: &Path) -> camel_bridge::process::BridgeProcessConfig {
    camel_bridge::process::BridgeProcessConfig {
        spec: &camel_bridge::spec::JMS_BRIDGE,
        binary_path: binary.to_path_buf(),
        broker_url: "tcp://localhost:61616".into(),
        broker_type: camel_bridge::process::BrokerType::Generic,
        username: None,
        password: None,
        start_timeout_ms: 5000,
        env_vars: Vec::new(),
        env_remove_keys: Vec::new(),
        expect_soap_ready: false,
    }
}

#[cfg(unix)]
#[test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
fn leak_parent_then_exit() {
    require_linux_proc();
    match role().as_deref() {
        None => outer_leak_parent_then_exit(),
        Some(LEAK_ROLE) => inner_leak_parent_then_exit(),
        Some(other) => panic!("unknown BRIDGE_EVIDENCE_ROLE: {other}"),
    }
}

/// Parent half: run the inner test in a fresh process with stderr INHERITED
/// (the reproduction IS inheritance), then assert the orphan group is gone.
#[cfg(unix)]
fn outer_leak_parent_then_exit() {
    let exe = std::env::current_exe().expect("resolve current test binary");
    let mut inner = std::process::Command::new(exe)
        .args(["--exact", LEAK_TEST, "--ignored", "--nocapture"])
        .env("BRIDGE_EVIDENCE_ROLE", LEAK_ROLE)
        // Force stderr inheritance inside BridgeProcess::start (the bug path).
        .env_remove("CAMEL_BRIDGE_LOG_STDERR")
        .stdout(std::process::Stdio::piped())
        // Deliberately NOT piped: the inner bridge inherits the cargo test
        // binary's stderr, which is the shell pipeline's write end.
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("spawn inner evidence process");

    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match inner.try_wait().expect("poll inner evidence process") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = inner.kill();
                panic!("inner evidence process did not exit within 60s");
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    };

    let mut stdout = String::new();
    if let Some(mut pipe) = inner.stdout.take() {
        pipe.read_to_string(&mut stdout)
            .expect("read inner evidence token");
    }
    let token = stdout
        .lines()
        .map(str::trim)
        .find(|line| line.contains("-fake-bridge-"))
        .expect("inner evidence process printed its fake bridge token")
        .to_string();

    // The inner process exit does not synchronously complete the group cleanup,
    // so await emptiness within the bounded cleanup contract.
    let empty_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let members = inspect_running(&token);
        if members.is_empty() {
            break;
        }
        if Instant::now() >= empty_deadline {
            panic!(
                "orphan survivors for {token} after inner exit ({status}): {members:?} \
                 — bridge children outlived the test process"
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // ... and it stays empty for the bounded re-check window.
    let recheck_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let members = inspect_running(&token);
        if !members.is_empty() {
            panic!("orphan survivors reappeared for {token}: {members:?}");
        }
        if Instant::now() >= recheck_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Child half: start the fake bridge with inherited stderr, leak the handle the
/// way a harness `exit()` skips destructors, announce the token, then exit.
#[cfg(unix)]
fn inner_leak_parent_then_exit() {
    let dir = tempfile::tempdir().expect("create evidence temp dir");
    let script = fake_bridge_script(dir.path(), true, false, true);
    let config = fake_config(&script);
    let token = script
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("fake bridge token in script filename")
        .to_string();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let process = runtime
        .block_on(camel_bridge::process::BridgeProcess::start(&config))
        .expect("start fake bridge process");

    // Simulate a harness that calls exit() and therefore never runs Drop.
    std::mem::forget(process);

    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{token}").expect("write evidence token");
    stdout.flush().expect("flush evidence token");

    std::process::exit(0);
}

/// A chatty stderr pipe (200 KiB written BEFORE the ready line) must not delay
/// readiness: the owned `stderr` drain keeps the pipe empty while startup is
/// still parsing stdout. Before this task stderr was inherited, so the scenario
/// was vacuous; this pins the new owned-stderr behavior end to end.
#[cfg(unix)]
#[tokio::test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
async fn chatty_stderr_does_not_delay_readiness() {
    let dir = tempfile::tempdir().expect("create evidence temp dir");
    let script = fake_bridge_script(dir.path(), false, true, true);
    let config = fake_config(&script);

    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_millis(config.start_timeout_ms),
        camel_bridge::process::BridgeProcess::start(&config),
    )
    .await;
    let elapsed = started.elapsed();

    let process = result
        .expect("readiness must not be delayed by a flooded stderr pipe")
        .expect("start fake bridge with flooded stderr");
    assert!(
        elapsed < Duration::from_millis(config.start_timeout_ms),
        "readiness took {elapsed:?}, which reached the {}ms startup bound — \
         the stderr pipe was not drained",
        config.start_timeout_ms
    );

    process.stop().await.expect("stop fake bridge");
}

/// `CAMEL_BRIDGE_LOG_STDERR` still redirects stderr to the per-bridge log file.
/// Run through a re-exec inner role because the variable is process-global.
#[cfg(unix)]
#[test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
fn stderr_file_capture_unchanged() {
    match role().as_deref() {
        None => outer_stderr_file_capture_unchanged(),
        Some(FILE_CAPTURE_ROLE) => inner_stderr_file_capture_unchanged(),
        Some(other) => panic!("unknown BRIDGE_EVIDENCE_ROLE: {other}"),
    }
}

#[cfg(unix)]
fn outer_stderr_file_capture_unchanged() {
    let log_dir = tempfile::tempdir().expect("create log temp dir");
    let exe = std::env::current_exe().expect("resolve current test binary");
    let mut inner = std::process::Command::new(exe)
        .args(["--exact", FILE_CAPTURE_TEST, "--ignored", "--nocapture"])
        .env("BRIDGE_EVIDENCE_ROLE", FILE_CAPTURE_ROLE)
        .env("BRIDGE_EVIDENCE_LOGDIR", log_dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn inner file-capture process");
    let inner_pid = inner.id();

    let status = inner.wait().expect("wait for inner file-capture process");
    assert!(
        status.success(),
        "inner file-capture process failed: {status}"
    );

    let log_name = camel_bridge::spec::JMS_BRIDGE
        .log_file_template
        .replace("{pid}", &inner_pid.to_string());
    let log_path = log_dir.path().join(log_name);

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let contents = std::fs::read_to_string(&log_path).unwrap_or_default();
        if contents.contains("fake-bridge stderr alive") {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "stderr log file {} does not contain the fake bridge stderr line: {:?}",
                log_path.display(),
                contents
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn inner_stderr_file_capture_unchanged() {
    let log_dir = std::env::var("BRIDGE_EVIDENCE_LOGDIR")
        .expect("outer process must pass BRIDGE_EVIDENCE_LOGDIR");
    // SAFETY: process-global env mutation; this re-executed inner process is the
    // only consumer, so no parallel test observes the change.
    unsafe { std::env::set_var("CAMEL_BRIDGE_LOG_STDERR", &log_dir) };

    let dir = tempfile::tempdir().expect("create evidence temp dir");
    let script = fake_bridge_script(dir.path(), false, false, true);
    let config = fake_config(&script);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let process = runtime
        .block_on(camel_bridge::process::BridgeProcess::start(&config))
        .expect("start fake bridge with file-captured stderr");

    // The fixture writes `fake-bridge stderr alive` immediately after the ready
    // line, with the bridge still alive. Poll for it before teardown so `stop()`
    // cannot kill the shell before that write lands in the redirected log.
    let log_name = config
        .spec
        .log_file_template
        .replace("{pid}", &std::process::id().to_string());
    let log_path = std::path::Path::new(&log_dir).join(log_name);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let contents = std::fs::read_to_string(&log_path).unwrap_or_default();
        if contents.contains("fake-bridge stderr alive") {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "live bridge did not write the stderr line to {} before teardown: {:?}",
                log_path.display(),
                contents
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    runtime.block_on(process.stop()).expect("stop fake bridge");
}

/// A bridge that never announces readiness must leave no group member behind
/// when `start()` returns `Timeout`: dropping the guard kills the child AND its
/// TERM-immune grandchild.
#[cfg(unix)]
#[test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
fn startup_timeout_leaves_no_member() {
    require_linux_proc();

    let dir = tempfile::tempdir().expect("create evidence temp dir");
    let script = fake_bridge_script(dir.path(), false, false, false);
    let token = script
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("fake bridge token in script filename")
        .to_string();
    let config = fake_config(&script);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    match runtime.block_on(camel_bridge::process::BridgeProcess::start(&config)) {
        Err(camel_bridge::process::BridgeError::Timeout(_)) => {}
        Err(other) => panic!("expected startup Timeout, got {other}"),
        Ok(_) => panic!("expected startup Timeout, got a running bridge"),
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let members = inspect_running(&token);
        if members.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "startup-timeout group members survived for {token}: {members:?} — \
                 the guard was not dropped on the timeout path"
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

// ---------------------------------------------------------------------------
// AFTER exit-path suite (Task 1.4)
// ---------------------------------------------------------------------------

#[cfg(unix)]
const PANIC_ROLE: &str = "panic-parent";
#[cfg(unix)]
const SIGTERM_ROLE: &str = "sigterm-parent";
#[cfg(unix)]
const SIGKILL_ROLE: &str = "sigkill-parent";
#[cfg(unix)]
const SIGINT_ROLE: &str = "sigint-parent";
#[cfg(unix)]
const TEARDOWN_ROLE: &str = "teardown-window";

#[cfg(unix)]
const PANIC_TEST: &str = "panic_parent_cleans_group";
#[cfg(unix)]
const SIGTERM_TEST: &str = "sigterm_parent_cleans_group";
#[cfg(unix)]
const SIGKILL_TEST: &str = "sigkill_parent_cleans_group";
#[cfg(unix)]
const SIGINT_TEST: &str = "sigint_parent_cleans_group";
#[cfg(unix)]
const TEARDOWN_TEST: &str = "term_during_teardown_window_parent_death";

/// Token embedded by [`fake_bridge_script`] in the script filename.
#[cfg(unix)]
fn script_token(script: &Path) -> String {
    script
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("fake bridge token in script filename")
        .to_string()
}

/// Token embedded in the TERM-immune grandchild's command line.
#[cfg(unix)]
fn gc_token(token: &str) -> String {
    format!("{token}-gc")
}

/// Fixture directory shared by a script, its pidfile, and its markers.
#[cfg(unix)]
fn fixture_dir(script: &Path) -> &Path {
    script
        .parent()
        .expect("fake bridge script has a parent dir")
}

#[cfg(unix)]
fn fixture_marker(script: &Path, name: &str) -> PathBuf {
    fixture_dir(script).join(name)
}

#[cfg(unix)]
fn fixture_pidfile(script: &Path) -> PathBuf {
    fixture_dir(script).join("bridge.pid")
}

/// Read a pid the fixture wrote, if the file is present and parsable.
#[cfg(unix)]
fn read_pidfile(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// Process-group id (field 5) of `/proc/<pid>/stat`.
///
/// For a bridge child this is the watchdog pid: the watchdog is the group
/// leader and the bridge joins its group in `pre_exec`.
#[cfg(unix)]
fn process_pgrp(pid: u32) -> Result<Option<u32>, String> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("cannot read /proc/{pid}/stat: {err}")),
    };
    let idx = stat
        .rfind(')')
        .ok_or_else(|| format!("malformed /proc/{pid}/stat: missing comm terminator"))?;
    let mut fields = stat[idx + 1..].split_whitespace();
    let _state = fields
        .next()
        .ok_or_else(|| format!("malformed /proc/{pid}/stat: missing state field"))?;
    let _ppid = fields
        .next()
        .ok_or_else(|| format!("malformed /proc/{pid}/stat: missing ppid field"))?;
    let pgrp = fields
        .next()
        .ok_or_else(|| format!("malformed /proc/{pid}/stat: missing pgrp field"))?;
    pgrp.parse::<u32>()
        .map(Some)
        .map_err(|err| format!("malformed /proc/{pid}/stat pgrp {pgrp:?}: {err}"))
}

/// Bounded 25 ms poll used by every assertion path (never a bare fixed sleep).
#[cfg(unix)]
fn poll_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Fail unless no live (non-zombie) member matching `token` remains in `bound`.
#[cfg(unix)]
fn wait_absent(token: &str, bound: Duration, context: &str) {
    if !poll_until(bound, || inspect_running(token).is_empty()) {
        panic!(
            "{context}: survivors for {token} after {bound:?}: {:?}",
            inspect_running(token)
        );
    }
}

/// Bounded wait for a child process, killing it on timeout so a hung inner
/// process cannot wedge the suite.
#[cfg(unix)]
fn wait_bounded(child: &mut std::process::Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().expect("poll inner evidence process") {
            Some(status) => return status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("inner evidence process did not exit within {timeout:?}");
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

/// Kill-and-reap guard for an inner process whose failure path must contain it.
///
/// Dropping the inner `Child` neither kills nor reaps it. If an assertion
/// panics after the inner was `SIGSTOP`ped, that leaves a permanently frozen
/// process holding the watchdog pipe. `Drop` sends `SIGKILL` (deliverable to a
/// stopped process) and reaps, unless disarmed at the successful end.
#[cfg(unix)]
struct InnerChildGuard {
    child: std::process::Child,
    armed: bool,
}

#[cfg(unix)]
impl InnerChildGuard {
    fn new(child: std::process::Child) -> Self {
        Self { child, armed: true }
    }

    /// The child already exited (and was reaped): nothing left to contain.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(unix)]
impl std::ops::Deref for InnerChildGuard {
    type Target = std::process::Child;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

#[cfg(unix)]
impl std::ops::DerefMut for InnerChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

#[cfg(unix)]
impl Drop for InnerChildGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Never signal a reaped pid: it may already belong to another process.
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        // SAFETY: direct SIGKILL to the child process this test spawned.
        let _ = unsafe { libc::kill(self.child.id() as i32, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

/// Re-exec the current test binary in one of this suite's inner roles.
///
/// The fixture script is created by the OUTER process and passed explicitly, so
/// both halves share the same token, pidfile, and markers without a stdout
/// control channel that could block while the inner is still alive.
#[cfg(unix)]
fn spawn_inner(test_name: &str, role_name: &str, script: &Path) -> std::process::Child {
    let exe = std::env::current_exe().expect("resolve current test binary");
    std::process::Command::new(exe)
        .args(["--exact", test_name, "--ignored", "--nocapture"])
        .env("BRIDGE_EVIDENCE_ROLE", role_name)
        .env("BRIDGE_EVIDENCE_SCRIPT", script)
        // Force the owned-stderr path; file capture is a different test.
        .env_remove("CAMEL_BRIDGE_LOG_STDERR")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn inner evidence process")
}

#[cfg(unix)]
fn inner_script() -> PathBuf {
    PathBuf::from(
        std::env::var("BRIDGE_EVIDENCE_SCRIPT")
            .expect("outer process must pass BRIDGE_EVIDENCE_SCRIPT"),
    )
}

#[cfg(unix)]
fn build_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

/// The bridge must still be RUNNING (not a zombie, not gone) when the teardown
/// precondition is checked. A zombie means the group KILL already fired.
#[cfg(unix)]
fn assert_bridge_running(pid: u32, context: &str) {
    match process_state(pid) {
        Ok(Some('R' | 'S')) => {}
        Ok(Some(state)) => panic!(
            "{context}: bridge {pid} state is {state:?}, not R/S — \
             escalation already fired or the bridge exited"
        ),
        Ok(None) => panic!("{context}: bridge {pid} has no /proc entry — it was already killed"),
        Err(err) => panic!("{context}: cannot inspect bridge {pid}: {err}"),
    }
}

#[cfg(unix)]
enum ParentDeath {
    Panic,
    Signal(i32),
}

#[cfg(unix)]
fn outer_parent_death(test_name: &str, role_name: &str, term_ignoring: bool, death: ParentDeath) {
    let dir = tempfile::tempdir().expect("create evidence temp dir");
    let script = fake_bridge_script(dir.path(), term_ignoring, false, true);
    let token = script_token(&script);
    let gc = gc_token(&token);

    let mut inner = spawn_inner(test_name, role_name, &script);

    match death {
        ParentDeath::Panic => {
            let status = wait_bounded(&mut inner, Duration::from_secs(60));
            assert_eq!(
                status.code(),
                Some(101),
                "panic must unwind and exit 101, got {status}"
            );
            assert_eq!(status.signal(), None, "panic must not die by signal");
        }
        ParentDeath::Signal(signal) => {
            let marker = fixture_marker(&script, "ready-marker");
            assert!(
                poll_until(Duration::from_secs(15), || marker.exists()),
                "inner role {role_name} never announced readiness ({})",
                marker.display()
            );
            // SAFETY: direct signal to a child process this test spawned.
            assert_eq!(
                unsafe { libc::kill(inner.id() as i32, signal) },
                0,
                "failed to send signal {signal} to inner process {}",
                inner.id()
            );
            let status = wait_bounded(&mut inner, Duration::from_secs(15));
            assert_eq!(
                status.signal(),
                Some(signal),
                "inner did not die from signal {signal}: {status}"
            );
        }
    }

    wait_absent(&token, Duration::from_secs(5), role_name);
    wait_absent(&gc, Duration::from_secs(5), role_name);
}

#[cfg(unix)]
#[test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
fn panic_parent_cleans_group() {
    require_linux_proc();
    match role().as_deref() {
        None => outer_parent_death(PANIC_TEST, PANIC_ROLE, true, ParentDeath::Panic),
        Some(PANIC_ROLE) => inner_panic_parent(),
        Some(other) => panic!("unknown BRIDGE_EVIDENCE_ROLE: {other}"),
    }
}

#[cfg(unix)]
fn inner_panic_parent() {
    let script = inner_script();
    let config = fake_config(&script);
    let runtime = build_runtime();
    let process = runtime
        .block_on(camel_bridge::process::BridgeProcess::start(&config))
        .expect("start fake bridge");
    // A failing harness skips Drop; only the watchdog EOF backstop remains.
    std::mem::forget(process);
    panic!("evidence");
}

#[cfg(unix)]
#[test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
fn sigterm_parent_cleans_group() {
    require_linux_proc();
    match role().as_deref() {
        None => outer_parent_death(
            SIGTERM_TEST,
            SIGTERM_ROLE,
            true,
            ParentDeath::Signal(libc::SIGTERM),
        ),
        Some(SIGTERM_ROLE) => inner_signal_parent(),
        Some(other) => panic!("unknown BRIDGE_EVIDENCE_ROLE: {other}"),
    }
}

#[cfg(unix)]
#[test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
fn sigkill_parent_cleans_group() {
    require_linux_proc();
    match role().as_deref() {
        None => outer_parent_death(
            SIGKILL_TEST,
            SIGKILL_ROLE,
            true,
            ParentDeath::Signal(libc::SIGKILL),
        ),
        Some(SIGKILL_ROLE) => inner_signal_parent(),
        Some(other) => panic!("unknown BRIDGE_EVIDENCE_ROLE: {other}"),
    }
}

#[cfg(unix)]
#[test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
fn sigint_parent_cleans_group() {
    require_linux_proc();
    match role().as_deref() {
        None => outer_parent_death(
            SIGINT_TEST,
            SIGINT_ROLE,
            true,
            ParentDeath::Signal(libc::SIGINT),
        ),
        Some(SIGINT_ROLE) => inner_signal_parent(),
        Some(other) => panic!("unknown BRIDGE_EVIDENCE_ROLE: {other}"),
    }
}

#[cfg(unix)]
fn inner_signal_parent() {
    let script = inner_script();
    let config = fake_config(&script);
    let runtime = build_runtime();
    let process = runtime
        .block_on(camel_bridge::process::BridgeProcess::start(&config))
        .expect("start fake bridge");
    std::mem::forget(process);
    std::fs::write(fixture_marker(&script, "ready-marker"), b"ready")
        .expect("write readiness marker");
    // Wait for the outer to deliver the death signal.
    std::thread::sleep(Duration::from_secs(300));
}

#[cfg(unix)]
#[test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
fn term_during_teardown_window_parent_death() {
    require_linux_proc();
    match role().as_deref() {
        None => outer_term_during_teardown_window(),
        Some(TEARDOWN_ROLE) => inner_teardown_window(),
        Some(other) => panic!("unknown BRIDGE_EVIDENCE_ROLE: {other}"),
    }
}

/// Inner: TERM is trapped (both markers written inside the handler) and the
/// bridge loops forever, so `stop()` is stuck in its bounded non-reaping grace
/// wait when this process writes `ready-to-die`.
#[cfg(unix)]
fn inner_teardown_window() {
    let script = inner_script();
    let config = fake_config(&script);
    let die_marker = fixture_marker(&script, "die-marker");
    let ready_marker = fixture_marker(&script, "ready-to-die");
    let runtime = build_runtime();
    runtime.block_on(async move {
        let process = camel_bridge::process::BridgeProcess::start(&config)
            .await
            .expect("start fake bridge");
        // Detached: the outer kills this process inside the grace window, so
        // this task must never be awaited as a completion signal.
        let _stop = tokio::spawn(async move { process.stop().await });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if die_marker.exists() {
                break;
            }
            if Instant::now() >= deadline {
                panic!("bridge did not trap TERM within 2s");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        std::fs::write(&ready_marker, b"die").expect("write ready-to-die marker");
        // The outer freezes and kills this process inside the 5s grace window.
        tokio::time::sleep(Duration::from_secs(300)).await;
    });
}

#[cfg(unix)]
fn outer_term_during_teardown_window() {
    let dir = tempfile::tempdir().expect("create evidence temp dir");
    let script = fake_bridge_script(dir.path(), true, false, true);
    let token = script_token(&script);
    let gc = gc_token(&token);
    let mut inner = InnerChildGuard::new(spawn_inner(TEARDOWN_TEST, TEARDOWN_ROLE, &script));

    let ready = fixture_marker(&script, "ready-to-die");
    assert!(
        poll_until(Duration::from_secs(15), || ready.exists()),
        "inner never entered the TERM→escalation teardown window"
    );

    let pidfile = fixture_pidfile(&script);
    let bridge_pid = read_pidfile(&pidfile)
        .unwrap_or_else(|| panic!("missing/unparsable fixture pidfile {}", pidfile.display()));

    // Precondition (self-verifying): escalation has NOT fired. If it had, the
    // bridge would be SIGKILLed (zombie or gone) instead of running.
    assert_bridge_running(bridge_pid, "teardown window (before freeze)");

    let inner_pid = inner.id();
    // SAFETY: direct signal to the child process this test spawned.
    assert_eq!(
        unsafe { libc::kill(inner_pid as i32, libc::SIGSTOP) },
        0,
        "failed to freeze inner process {inner_pid}"
    );
    assert!(
        poll_until(Duration::from_secs(2), || {
            matches!(process_state(inner_pid), Ok(Some('T')))
        }),
        "inner process {inner_pid} did not reach stopped state"
    );

    // Freezing the whole inner process also freezes the spawn_blocking worker
    // that owns the escalation, so this re-check cannot race.
    assert_bridge_running(bridge_pid, "teardown window (while frozen)");

    // SAFETY: direct SIGKILL to the frozen child; the watchdog EOF backstop
    // must clean the group.
    assert_eq!(
        unsafe { libc::kill(inner_pid as i32, libc::SIGKILL) },
        0,
        "failed to kill frozen inner process {inner_pid}"
    );
    let status = wait_bounded(&mut inner, Duration::from_secs(15));
    assert_eq!(status.signal(), Some(libc::SIGKILL));

    wait_absent(&token, Duration::from_secs(5), TEARDOWN_ROLE);
    wait_absent(&gc, Duration::from_secs(5), TEARDOWN_ROLE);
    // Every containment assertion passed: the inner is reaped, stand the guard down.
    inner.disarm();
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
async fn watchdog_leader_exited_descendants_still_cleaned() {
    require_linux_proc();
    let dir = tempfile::tempdir().expect("create evidence temp dir");
    let script = fake_bridge_script(dir.path(), true, false, true);
    let token = script_token(&script);
    let gc = gc_token(&token);
    let config = fake_config(&script);

    let process = camel_bridge::process::BridgeProcess::start(&config)
        .await
        .expect("start fake bridge");
    let bridge_pid = process.child_pid().expect("bridge pid");
    let leader = process_pgrp(bridge_pid)
        .expect("inspect bridge process group")
        .expect("bridge /proc entry present");
    assert_ne!(
        leader, bridge_pid,
        "bridge must not be its own group leader"
    );

    // Early watchdog death: the monitor thread must clean the group without
    // depending on the leader still being alive.
    // SAFETY: direct SIGKILL to the watchdog group leader.
    assert_eq!(
        unsafe { libc::kill(leader as i32, libc::SIGKILL) },
        0,
        "failed to kill watchdog leader {leader}"
    );

    wait_absent(&token, Duration::from_secs(5), "early watchdog death");
    wait_absent(&gc, Duration::from_secs(5), "early watchdog death");

    // The group is already invalidated; stop() must still return Ok.
    process
        .stop()
        .await
        .expect("stop after leader death must succeed");
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
async fn launcher_grandchild_dies_with_group() {
    require_linux_proc();
    let dir = tempfile::tempdir().expect("create evidence temp dir");
    let script = fake_bridge_script(dir.path(), true, false, true);
    let token = script_token(&script);
    let gc = gc_token(&token);
    let config = fake_config(&script);

    let process = camel_bridge::process::BridgeProcess::start(&config)
        .await
        .expect("start fake bridge");
    process.stop().await.expect("graceful stop");

    wait_absent(&token, Duration::from_secs(5), "graceful stop");
    wait_absent(&gc, Duration::from_secs(5), "graceful stop");
}

/// Start a bridge on a dedicated std thread whose single-thread runtime is
/// dropped BEFORE the process handle is handed to the caller runtime.
#[cfg(unix)]
async fn start_on_retired_thread(script: &Path) -> camel_bridge::process::BridgeProcess {
    let config = fake_config(script);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = build_runtime();
        let result = runtime.block_on(camel_bridge::process::BridgeProcess::start(&config));
        // Retirement happens before the owner observes the process.
        drop(runtime);
        let _ = tx.send(result.map_err(|err| err.to_string()));
    });
    // The receiver itself is bounded: an abandoned blocking task would keep the
    // runtime alive at shutdown, reproducing the stall this suite exercises.
    tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(30)))
        .await
        .expect("join retired-thread receiver")
        .unwrap_or_else(|err| match err {
            std::sync::mpsc::RecvTimeoutError::Timeout => {
                panic!("retired thread did not hand over the process within 30s")
            }
            std::sync::mpsc::RecvTimeoutError::Disconnected => {
                panic!("retired thread dropped its sender before sending")
            }
        })
        .unwrap_or_else(|err| panic!("start on retired thread failed: {err}"))
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
async fn caller_thread_retirement_keeps_bridge() {
    require_linux_proc();

    // (a) The caller runtime retires, the bridge stays alive, explicit stop works.
    let dir_a = tempfile::tempdir().expect("create evidence temp dir A");
    let script_a = fake_bridge_script_with_post_ready_writes(dir_a.path(), true, false, true, 50);
    let token_a = script_token(&script_a);
    let gc_a = gc_token(&token_a);
    let process_a = start_on_retired_thread(&script_a).await;
    let pid_a = process_a.child_pid().expect("bridge pid A");
    // SAFETY: signal 0 is an existence probe only.
    assert_eq!(
        unsafe { libc::kill(pid_a as i32, 0) },
        0,
        "bridge A died together with its caller runtime"
    );

    // Post-retirement stderr activity: the fixture keeps writing stderr after
    // the originating runtime is gone. Those writes must still be drained —
    // the reader closing would make the fixture's next write fail and exit.
    let ack = fixture_marker(&script_a, "post-ready-ack");
    let ack_value = |path: &Path| -> u32 {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0)
    };
    let base_writes = ack_value(&ack);
    assert!(
        poll_until(Duration::from_secs(5), || {
            ack_value(&ack) >= base_writes.saturating_add(2)
        }),
        "fixture did not land post-retirement stderr writes (ack stayed at {base_writes}); \
         the drain reader did not survive caller runtime retirement"
    );
    assert_bridge_running(pid_a, "retired runtime (active stderr)");

    // Releasing the write loop keeps the fixture in its terminal wait; stop()
    // still owns the teardown.
    std::fs::write(fixture_marker(&script_a, "post-ready-stop"), b"stop")
        .expect("write post-ready stop marker");
    process_a.stop().await.expect("stop bridge A");
    wait_absent(&token_a, Duration::from_secs(5), "retired runtime (stop)");
    wait_absent(&gc_a, Duration::from_secs(5), "retired runtime (stop)");

    // (b) No stop ever runs on this instance: only the dedicated monitor thread
    // can clean up after the watchdog leader dies.
    let dir_b = tempfile::tempdir().expect("create evidence temp dir B");
    let script_b = fake_bridge_script(dir_b.path(), true, false, true);
    let token_b = script_token(&script_b);
    let gc_b = gc_token(&token_b);
    let process_b = start_on_retired_thread(&script_b).await;
    let pid_b = process_b.child_pid().expect("bridge pid B");
    let leader = process_pgrp(pid_b)
        .expect("inspect bridge process group")
        .expect("bridge /proc entry present");
    // SAFETY: direct SIGKILL to the watchdog group leader.
    assert_eq!(
        unsafe { libc::kill(leader as i32, libc::SIGKILL) },
        0,
        "failed to kill watchdog leader {leader}"
    );
    wait_absent(
        &token_b,
        Duration::from_secs(5),
        "retired runtime (watchdog)",
    );
    wait_absent(&gc_b, Duration::from_secs(5), "retired runtime (watchdog)");

    // No stop() was called on this instance; dropping it must not respawn or
    // leak anything (the group is already invalidated).
    drop(process_b);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
async fn concurrent_spawns_keep_eof_discipline() {
    require_linux_proc();
    let dir_a = tempfile::tempdir().expect("create evidence temp dir A");
    let dir_b = tempfile::tempdir().expect("create evidence temp dir B");
    let script_a = fake_bridge_script(dir_a.path(), true, false, true);
    let script_b = fake_bridge_script(dir_b.path(), true, false, true);
    let token_a = script_token(&script_a);
    let token_b = script_token(&script_b);
    let gc_a = gc_token(&token_a);
    let gc_b = gc_token(&token_b);
    let config_a = fake_config(&script_a);
    let config_b = fake_config(&script_b);

    // Genuinely concurrent starts on two OS threads, each with its own runtime.
    // `tokio::join!` would establish both futures in the same task, so spawn and
    // pipe creation would run strictly one after the other; the barrier releases
    // both threads into `start()` at the same instant instead.
    let barrier = Arc::new(Barrier::new(2));
    let spawn_start = |config: camel_bridge::process::BridgeProcessConfig| {
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            let runtime = build_runtime();
            barrier.wait();
            runtime.block_on(camel_bridge::process::BridgeProcess::start(&config))
        })
    };
    let thread_a = spawn_start(config_a);
    let thread_b = spawn_start(config_b);
    let process_a = thread_a
        .join()
        .expect("join start thread A")
        .expect("start bridge A");
    let process_b = thread_b
        .join()
        .expect("join start thread B")
        .expect("start bridge B");
    let pid_b = process_b.child_pid().expect("bridge pid B");

    // Dropping A tears down only A's group.
    drop(process_a);
    assert!(
        poll_until(Duration::from_secs(5), || {
            inspect_running(&token_a).is_empty()
                && inspect_running(&gc_a).is_empty()
                && unsafe { libc::kill(pid_b as i32, 0) } == 0
        }),
        "dropping bridge A must clean A's group and leave B alive"
    );

    process_b.stop().await.expect("stop bridge B");
    wait_absent(&token_b, Duration::from_secs(5), "concurrent spawn B");
    wait_absent(&gc_b, Duration::from_secs(5), "concurrent spawn B");
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "slow test: spawns real child processes (run with --ignored)"]
async fn graceful_stop_reaps_no_running_members() {
    require_linux_proc();
    let dir = tempfile::tempdir().expect("create evidence temp dir");
    let script = fake_bridge_script(dir.path(), true, false, true);
    let token = script_token(&script);
    let gc = gc_token(&token);
    let config = fake_config(&script);

    let process = camel_bridge::process::BridgeProcess::start(&config)
        .await
        .expect("start fake bridge");
    process.stop().await.expect("graceful stop");

    wait_absent(&token, Duration::from_secs(5), "graceful stop");
    wait_absent(&gc, Duration::from_secs(5), "graceful stop");
}
