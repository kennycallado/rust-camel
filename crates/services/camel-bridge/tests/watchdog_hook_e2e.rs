//! Harness-free end-to-end proof for the self-executing watchdog (mission 417).
//!
//! The binary itself installs the hook with `watchdog::maybe_run()` as the
//! first statement of `main`, exactly like a real embedder. Group supervision
//! then re-executes this binary in watchdog role instead of requiring a POSIX
//! `sh`.
//!
//! The scenarios are Unix-only, but `fn main` must stay unconditional: the
//! target is registered with `harness = false`, so Cargo needs a `main` on
//! every platform. On non-Unix hosts `main` prints a skip note and exits 0.
//!
//! Scenarios run sequentially; each prints `PASS <name>` and the process exits
//! non-zero on the first failure.

fn main() {
    #[cfg(unix)]
    {
        // Embedder shape: the hook MUST run before anything else. A role
        // re-exec also reaches this line, where `maybe_run` hands control to
        // the watchdog branch when argv carries the marker.
        camel_bridge::watchdog::maybe_run();
        if let Some(role) = std::env::var_os(unix::ROLE_ENV) {
            unix::run_role(role);
        }
        unix::run();
    }
    #[cfg(not(unix))]
    {
        println!("SKIP watchdog_hook_e2e: unix-only scenarios (non-unix host)");
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::OsString;
    use std::os::fd::{AsRawFd, RawFd};
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    use camel_bridge::process::{BridgeProcess, BridgeProcessConfig, BrokerType};
    use camel_component_api::test_support::EnvGuard;

    /// Arming descriptor supervision duplicates for the watchdog role. Must
    /// match `group::ACK_FD` (single digit; POSIX redirection target).
    const ACK_FD: RawFd = 9;

    /// Role selected by the scenario that re-execs this binary.
    pub(super) const ROLE_ENV: &str = "CAMEL_WATCHDOG_E2E_ROLE";
    /// Fixture directory shared between the outer scenario and its role.
    const DIR_ENV: &str = "CAMEL_WATCHDOG_E2E_DIR";
    /// Role that starts a real bridge and then waits to be killed.
    const PARENT_DEATH_ROLE: &str = "parent_death";
    /// Written by the role AFTER `BridgeProcess::start` returns, so the outer
    /// only probes pids once the watchdog, bridge, and grandchild are armed.
    const ARMED_MARKER: &str = "armed";

    pub(super) fn run() {
        let scenarios: [(&str, fn()); 3] = [
            (
                "empty_path_no_shell_start_succeeds",
                empty_path_no_shell_start_succeeds,
            ),
            (
                "refuses_to_arm_when_not_group_leader",
                refuses_to_arm_when_not_group_leader,
            ),
            (
                "hook_parent_death_eof_kills_group_after_sigterm_immunity",
                hook_parent_death_eof_kills_group_after_sigterm_immunity,
            ),
        ];

        for (name, scenario) in scenarios {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(scenario));
            match outcome {
                Ok(()) => println!("PASS {name}"),
                Err(_) => {
                    eprintln!("FAIL {name}");
                    std::process::exit(1);
                }
            }
        }
    }

    /// Dispatch a re-executed role. The role branch never returns: it either
    /// plays the bridge-owning parent until the outer kills it, or exits on a
    /// malformed invocation.
    pub(super) fn run_role(role: OsString) -> ! {
        match role.to_str() {
            Some(PARENT_DEATH_ROLE) => parent_death_role(),
            Some(other) => {
                eprintln!("unknown {ROLE_ENV}: {other}");
                std::process::exit(2);
            }
            None => {
                eprintln!("{ROLE_ENV} is not valid UTF-8");
                std::process::exit(2);
            }
        }
    }

    /// Role half: start a real bridge (the watchdog is this same binary
    /// re-executed through the hook), announce readiness, then wait to be
    /// killed. The process-global hook install stays in this subprocess, so no
    /// sibling scenario observes a flipped hook state.
    fn parent_death_role() -> ! {
        let dir = PathBuf::from(
            std::env::var_os(DIR_ENV).expect("outer scenario must pass the fixture directory"),
        );
        let script = launcher_script(&dir);
        let config = fake_bridge_config(&script);
        let runtime = tokio::runtime::Runtime::new().expect("build tokio runtime");
        // Keep the process alive (and with it the liveness write end) until the
        // outer sends SIGKILL; the watchdog then sees EOF and kills the group.
        let _process = runtime
            .block_on(BridgeProcess::start(&config))
            .expect("start fake bridge in the role process");
        std::fs::write(dir.join(ARMED_MARKER), b"armed").expect("write armed marker");
        loop {
            std::thread::sleep(Duration::from_secs(60)); // allow-test-sleep: the outer kills this process; the sleep only parks the role
        }
    }

    /// Scenario (c): through the HOOK path, prove both halves of the
    /// supervisor contract.
    ///
    /// The outer process never establishes a group itself: the bridge-owning
    /// parent is a re-executed role subprocess, so the process-global hook
    /// install cannot leak into the other scenarios.
    ///
    /// (a) `SIGTERM` to the bridge GROUP leaves the watchdog, the launcher
    ///     bridge, and the TERM-ignoring grandchild alive.
    /// (b) `SIGKILL` the role parent closes the liveness pipe; the watchdog
    ///     sees EOF and kills the whole group, so the launcher and grandchild
    ///     disappear.
    fn hook_parent_death_eof_kills_group_after_sigterm_immunity() {
        let dir = tempfile::tempdir().expect("create fixture temp dir");
        let fixture_dir = dir.path().to_path_buf();
        let exe = std::env::current_exe().expect("resolve current test binary");

        let child = std::process::Command::new(exe)
            .env(ROLE_ENV, PARENT_DEATH_ROLE)
            .env(DIR_ENV, &fixture_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn parent-death role");
        let mut guard = TeardownGuard::new(child);

        wait_for_armed(&mut guard, &fixture_dir.join(ARMED_MARKER));

        let bridge_pid = read_pidfile(&fixture_dir.join("bridge.pid"))
            .expect("launcher bridge must publish its pid");
        let grandchild_pid =
            read_pidfile(&fixture_dir.join("gc.pid")).expect("grandchild must publish its pid");
        let pgid = process_pgrp(bridge_pid)
            .expect("inspect bridge process group")
            .unwrap_or_else(|| panic!("bridge {bridge_pid} is not inspectable"));
        assert_ne!(
            pgid, bridge_pid,
            "the launcher bridge {bridge_pid} must not lead the watchdog group"
        );
        guard.set_pgid(pgid);

        // (a) SIGTERM the whole group: the watchdog ignores it and both fixture
        // levels ignore it, so every member must survive the bounded window.
        // SAFETY: negative pid addresses the process group this scenario owns.
        assert_eq!(
            unsafe { libc::kill(-(pgid as libc::pid_t), libc::SIGTERM) },
            0,
            "failed to SIGTERM bridge group {pgid}"
        );
        let survive_until = Instant::now() + Duration::from_millis(750);
        while Instant::now() < survive_until {
            assert!(
                pid_running(bridge_pid),
                "launcher bridge {bridge_pid} died from a group SIGTERM"
            );
            assert!(
                pid_running(grandchild_pid),
                "grandchild {grandchild_pid} died from a group SIGTERM"
            );
            std::thread::sleep(Duration::from_millis(25)); // allow-test-sleep: bounded survival window, deadline checked above
        }

        // (b) Kill the parent: the liveness pipe reaches EOF and the
        // hook-installed watchdog must take the group down.
        let role_pid = guard.child_mut().id();
        // SAFETY: direct SIGKILL to the role child this scenario spawned.
        assert_eq!(
            unsafe { libc::kill(role_pid as libc::pid_t, libc::SIGKILL) },
            0,
            "failed to SIGKILL role parent {role_pid}"
        );
        let status = wait_bounded(guard.child_mut(), Duration::from_secs(10));
        assert!(
            !status.success(),
            "role parent {role_pid} unexpectedly exited cleanly: {status}"
        );

        assert!(
            wait_until(Duration::from_secs(5), || {
                !pid_running(bridge_pid) && !pid_running(grandchild_pid)
            }),
            "EOF path left survivors: launcher {bridge_pid} running={}, grandchild \
             {grandchild_pid} running={}",
            pid_running(bridge_pid),
            pid_running(grandchild_pid)
        );
        assert!(
            wait_until(Duration::from_secs(5), || !group_running(pgid)),
            "watchdog group {pgid} survived the EOF-triggered SIGKILL"
        );
        guard.disarm();
    }

    /// Poll for the role's armed marker, failing fast if the role exits first.
    fn wait_for_armed(guard: &mut TeardownGuard, marker: &Path) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if marker.exists() {
                return;
            }
            match guard
                .child_mut()
                .try_wait()
                .expect("poll parent-death role")
            {
                Some(status) => panic!("role exited before arming: {status}"),
                None if Instant::now() >= deadline => {
                    panic!("role never armed within 30s ({})", marker.display())
                }
                None => std::thread::sleep(Duration::from_millis(25)), // allow-test-sleep: bounded armed poll, deadline checked above
            }
        }
    }

    /// Kill-and-reap guard: a panic before the explicit teardown must not leave
    /// the role or its bridge group running for the rest of the suite.
    struct TeardownGuard {
        child: Option<Child>,
        pgid: Option<u32>,
        armed: bool,
    }

    impl TeardownGuard {
        fn new(child: Child) -> Self {
            Self {
                child: Some(child),
                pgid: None,
                armed: true,
            }
        }

        fn set_pgid(&mut self, pgid: u32) {
            self.pgid = Some(pgid);
        }

        fn child_mut(&mut self) -> &mut Child {
            self.child.as_mut().expect("role child is still owned")
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for TeardownGuard {
        fn drop(&mut self) {
            if !self.armed {
                return;
            }
            if let Some(mut child) = self.child.take() {
                if !matches!(child.try_wait(), Ok(Some(_))) {
                    let _ = child.kill();
                }
                let _ = child.wait();
            }
            if let Some(pgid) = self.pgid.take() {
                // SAFETY: negative pid addresses the watchdog group created by
                // this scenario; it is leader-checked and owned by the role.
                unsafe {
                    libc::kill(-(pgid as libc::pid_t), libc::SIGKILL);
                }
            }
        }
    }

    /// Scenario (a): with `PATH` pointing at an EMPTY directory, the bridge
    /// still starts because the watchdog is the host binary re-executed through
    /// its absolute path.
    ///
    /// A set-but-empty `PATH` is stronger than an unset one: `execvp` falls
    /// back to the platform default search path when `PATH` is absent, so an
    /// unset `PATH` cannot prove the shell fallback was skipped. An empty
    /// directory makes the `sh` search deterministically fruitless while
    /// absolute-path spawns (`/proc/self/exe`, the fixture's absolute path with
    /// a kernel-resolved shebang) keep working.
    ///
    /// Teardown must leave no live member of the watchdog's process group.
    fn empty_path_no_shell_start_succeeds() {
        let dir = tempfile::tempdir().expect("create fixture temp dir");
        let script = fake_bridge_script(dir.path());
        let config = fake_bridge_config(&script);
        let empty_path_dir = dir.path().join("empty-path-dir");
        std::fs::create_dir_all(&empty_path_dir).expect("create empty PATH directory");

        let runtime = tokio::runtime::Runtime::new().expect("build tokio runtime");
        let process = {
            let _path_empty = EnvGuard::set("PATH", empty_path_dir.clone());
            runtime
                .block_on(BridgeProcess::start(&config))
                .expect("start must succeed with the self-exec watchdog and an empty PATH")
        };

        let bridge_pid = process.child_pid().expect("bridge pid");
        let pgid = process_pgrp(bridge_pid)
            .expect("inspect bridge process group")
            .unwrap_or_else(|| panic!("bridge {bridge_pid} is not inspectable"));

        runtime
            .block_on(process.stop())
            .expect("stop fake bridge process");

        assert!(
            wait_until(Duration::from_secs(5), || {
                !group_running(pgid) && !pid_running(bridge_pid)
            }),
            "watchdog group {pgid} or bridge {bridge_pid} survived teardown"
        );
    }

    /// Scenario (b): a role process that is NOT its own process-group leader
    /// must REFUSE to arm — exiting with the refusal code BEFORE writing the
    /// arming acknowledgement.
    ///
    /// Plain `std::process::Command` spawn (no `setpgid`), so the child
    /// inherits the caller's group and can never lead it. A real pipe is
    /// connected to the role's ack descriptor (fd 9) via `pre_exec`, exactly
    /// like `group::establish_spawn`; the parent keeps the read end and
    /// asserts EOF with zero bytes.
    fn refuses_to_arm_when_not_group_leader() {
        let exe = std::env::current_exe().expect("resolve current test binary");
        let (ack_read, ack_write) = std::io::pipe().expect("create ack pipe");
        let ack_read_fd = ack_read.as_raw_fd();
        let ack_write_fd = ack_write.as_raw_fd();

        let mut cmd = std::process::Command::new(exe);
        cmd.arg(camel_bridge::watchdog::ROLE_MARKER)
            .arg(ACK_FD.to_string())
            // Held open below: an armed role would block on the liveness pipe
            // instead of SIGKILLing the caller's process group.
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        // SAFETY: only async-signal-safe dup2/fcntl calls run between fork and
        // exec, mirroring `group::establish_spawn`'s pre_exec pattern.
        unsafe {
            cmd.pre_exec(move || {
                if ack_write_fd == ACK_FD {
                    // Same fd already: just make sure it survives exec.
                    if libc::fcntl(ACK_FD, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                } else {
                    // dup2 clears FD_CLOEXEC on the copy at ACK_FD.
                    if libc::dup2(ack_write_fd, ACK_FD) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }

        let mut child = cmd.spawn().expect("spawn role child");
        // Parent must not hold a write end while observing EOF on the read end.
        drop(ack_write);
        let _liveness = child.stdin.take();

        let status = wait_bounded(&mut child, Duration::from_secs(10));
        assert_eq!(
            status.code(),
            Some(camel_bridge::watchdog::WATCHDOG_REFUSAL_EXIT),
            "role child must refuse before arming, got {status}"
        );

        let received =
            read_to_eof_bounded(ack_read_fd, Duration::from_secs(5)).expect("read ack pipe to EOF");
        assert!(
            received.is_empty(),
            "role child wrote {received:?} before refusing; the refusal must precede the ack"
        );
    }

    /// POSIX single-quote a string for safe interpolation into a shell script.
    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    /// Launcher fixture for the parent-death scenario: it publishes its pid,
    /// ignores `SIGTERM`, arms a TERM-ignoring sleeping grandchild, then prints
    /// the ready line and parks. Both levels ignore `TERM`, so only the
    /// watchdog's EOF-triggered group `SIGKILL` can take them down.
    fn launcher_script(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = dir.join("launcher.sh");
        let pidfile = dir.join("bridge.pid");
        let gc_pidfile = dir.join("gc.pid");
        let gc_marker = dir.join("gc-marker");

        let inner = format!(
            "trap '' TERM; echo armed > {}; echo $$ > {}; while :; do sleep 1; done",
            shell_quote(&gc_marker.to_string_lossy()),
            shell_quote(&gc_pidfile.to_string_lossy()),
        );
        let body = format!(
            "#!/bin/sh\n\
             set -u\n\
             echo $$ > {pidfile}\n\
             trap '' TERM\n\
             sh -c {inner} watchdog-e2e-gc &\n\
             gc_pid=$!\n\
             i=0\n\
             while [ \"$i\" -lt 60 ]; do\n\
               if kill -0 \"$gc_pid\" 2>/dev/null && [ -f {gc_marker} ]; then break; fi\n\
               i=$((i+1))\n\
               sleep 0.05\n\
             done\n\
             if ! kill -0 \"$gc_pid\" 2>/dev/null || [ ! -f {gc_marker} ]; then\n\
               echo \"launcher grandchild failed to arm\" >&2\n\
               exit 1\n\
             fi\n\
             printf '%s\\n' '{{\"status\":\"ready\",\"port\":4242}}'\n\
             while :; do sleep 1; done\n",
            pidfile = shell_quote(&pidfile.to_string_lossy()),
            inner = shell_quote(&inner),
            gc_marker = shell_quote(&gc_marker.to_string_lossy()),
        );

        std::fs::write(&script, body).expect("write launcher script");
        let mut perms = std::fs::metadata(&script)
            .expect("stat launcher script")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("chmod launcher script");
        script
    }

    /// Read a pid a fixture published, if present and parsable.
    fn read_pidfile(path: &Path) -> Option<u32> {
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    }

    /// Fake bridge: write its pid, announce readiness, then spin without any
    /// external binary (the fixture must work with an empty `PATH`).
    fn fake_bridge_script(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = dir.join("bridge.sh");
        let pidfile = dir.join("bridge.pid");
        let body = format!(
            "#!/bin/sh\n\
             set -u\n\
             echo $$ > '{}'\n\
             printf '%s\\n' '{{\"status\":\"ready\",\"port\":4242}}'\n\
             while :; do :; done\n",
            pidfile.display()
        );

        std::fs::write(&script, body).expect("write fake bridge script");
        let mut perms = std::fs::metadata(&script)
            .expect("stat fake bridge script")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("chmod fake bridge script");
        script
    }

    fn fake_bridge_config(script: &Path) -> BridgeProcessConfig {
        BridgeProcessConfig {
            spec: &camel_bridge::spec::JMS_BRIDGE,
            binary_path: script.to_path_buf(),
            broker_url: "tcp://localhost:61616".into(),
            broker_type: BrokerType::Generic,
            username: None,
            password: None,
            start_timeout_ms: 5000,
            env_vars: Vec::new(),
            env_remove_keys: Vec::new(),
            expect_soap_ready: false,
        }
    }

    /// Process-group id of `pid`.
    ///
    /// Linux reads `/proc/<pid>/stat`; other Unix targets use the portable
    /// `getpgid` call (macOS has no `/proc`).
    #[cfg(target_os = "linux")]
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

    #[cfg(not(target_os = "linux"))]
    fn process_pgrp(pid: u32) -> Result<Option<u32>, String> {
        // SAFETY: getpgid has no preconditions; a nonexistent pid yields ESRCH.
        let pgid = unsafe { libc::getpgid(pid as libc::pid_t) };
        if pgid >= 0 {
            return Ok(Some(pgid as u32));
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        Err(format!("cannot getpgid({pid}): {err}"))
    }

    /// Process state character (field 3) of `/proc/<pid>/stat`.
    #[cfg(target_os = "linux")]
    fn process_state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, rest) = stat.rsplit_once(')')?;
        rest.trim_start().chars().next()
    }

    /// Running = not a zombie and not gone.
    #[cfg(target_os = "linux")]
    fn pid_running(pid: u32) -> bool {
        !matches!(process_state(pid), None | Some('Z') | Some('X'))
    }

    #[cfg(not(target_os = "linux"))]
    fn pid_running(pid: u32) -> bool {
        signal_probe(pid as libc::pid_t)
    }

    /// Any live (non-zombie) process whose process-group id is `pgid`.
    #[cfg(target_os = "linux")]
    fn group_running(pgid: u32) -> bool {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return false;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
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
            let Ok(pgrp) = fields[2].parse::<u32>() else {
                continue;
            };
            if pgrp == pgid && !matches!(state, Some('Z') | Some('X')) {
                return true;
            }
        }
        false
    }

    #[cfg(not(target_os = "linux"))]
    fn group_running(pgid: u32) -> bool {
        // A negative pid addresses every member of the process group.
        signal_probe(-(pgid as libc::pid_t))
    }

    /// `kill(target, 0)` existence probe: `0` or `EPERM` means the target (or
    /// group) still exists, `ESRCH` means it is gone.
    #[cfg(not(target_os = "linux"))]
    fn signal_probe(target: libc::pid_t) -> bool {
        // SAFETY: signal 0 performs existence/permission checking only.
        let rc = unsafe { libc::kill(target, 0) };
        if rc == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    /// Bounded drain of `fd` until EOF. A pipe that never closes fails the
    /// scenario instead of parking it.
    fn read_to_eof_bounded(fd: RawFd, timeout: Duration) -> std::io::Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut out = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "ack pipe never reached EOF",
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
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            if rc == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "ack pipe never reached EOF",
                ));
            }
            let mut chunk = [0u8; 64];
            // SAFETY: chunk is a valid 64-byte destination.
            let n = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
            if n == 0 {
                return Ok(out);
            }
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            out.extend_from_slice(&chunk[..n as usize]);
        }
    }

    /// Bounded 25 ms poll; never a bare fixed sleep.
    fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if condition() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25)); // allow-test-sleep: bounded poll, deadline checked above
        }
    }

    /// Bounded wait for a child, killing it on timeout so a hung child cannot
    /// wedge the suite.
    fn wait_bounded(child: &mut Child, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait().expect("poll role child") {
                Some(status) => return status,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    panic!("role child did not exit within {timeout:?}");
                }
                None => std::thread::sleep(Duration::from_millis(25)), // allow-test-sleep: bounded poll, deadline checked above
            }
        }
    }
}
