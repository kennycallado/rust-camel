# Tasks: jms-crash-probe-topology

Shared worker conventions (apply to every task below):

- Worktree: the current checkout. Never build in the main checkout
  (`/home/kenny/dev/rust-camel`); `TMPDIR=/home/shared/tmp`.
- Gates per task: `cargo fmt --check --all`, `cargo clippy -p <touched
  crates> --all-targets -- -D warnings` (camel-jms also falls under the
  workspace clippy gate), and run the touched test target. If a target needs
  docker brokers, compile it (`--no-run`) and note
  `integration-verification-deferred-to-CI` — the conductor runs the docker
  legs after Task 3.
- `lint-unbounded-wait` ceiling is 0: no unbounded waits, no new sleeps in
  test bodies; use the existing bounded `wait_until` helper.
- Commit per task; never leave the tree dirty between tasks.

## Phase 1: Authoritative pid handoff

### Task 1: JmsBridgePool::bridge_pid read-only accessor

Files:

- `crates/components/camel-jms/src/component.rs` (pool impl block, near
  `resolve_broker_name` / `restart_slot`)
- existing tests module in the same file (stub-bridge pattern:
  `pool_knob_stub`, `write_stub`, `env_guard`, `BRIDGE_ENV_LOCK`)

Steps:

1. Add to the `JmsBridgePool` impl block:

   ```rust
   /// Read-only lifecycle accessor: the pid of the live bridge child serving
   /// `broker_name`, or `None` when the slot has no installed bridge process.
   /// Testapparatus use (crash-probe handoff): the pid is the authoritative
   /// `BridgeProcess::child_pid` — never re-derived from process groups.
   pub async fn bridge_pid(&self, broker_name: &str) -> Option<u32>
   ```

   Implementation: `self.resolve_broker_name(Some(broker_name)).ok()?` →
   `self.slots.get(&name)?.clone()` → lock `slot.process` →
   `guard.as_ref().and_then(BridgeProcess::child_pid)`. Doc comment required
   (doc-build gate runs with `-D warnings`).

2. Unit test (mirror `pool_knobs_reach_initial_and_replacement_spawn_env`
   setup: rustls provider install, `BRIDGE_ENV_LOCK`, `env_guard` for
   `CAMEL_JMS_BRIDGE_BINARY_PATH`, stub script that writes `$$` to a pidfile
   then announces ready):

   - name: `bridge_pid_accessor_returns_live_child_pid`
   - arrange: pool from `JmsPoolConfig::single_broker("tcp://localhost:61616",
     BrokerType::ActiveMq)` with `bridge_start_timeout_ms: 30_000`;
     env-var stub bridge; `pool.get_or_create_slot("default", false).await`
     returns the ready slot
   - act: `pool.bridge_pid("default").await`
   - assert: `Some(pid)` where `pid` equals the pid the stub wrote to its
     pidfile (read the pidfile, parse, compare)
   - then act 2: `pool.restart_slot("default")` and re-acquire, or drop the
     slot process via shutdown; assert the accessor returns `None` once the
     slot's process is gone (use the existing teardown path the neighboring
     tests use; bounded wait for the child to exit)

Acceptance criteria:

- [x] `cargo clippy -p camel-component-jms --all-targets -- -D warnings`
  exits 0 (plus workspace clippy membership unchanged)
- [x] `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-jms --no-deps`
  exits 0
- [x] the new unit test passes (`cargo test -p camel-component-jms
  bridge_pid_accessor -- --nocapture`)
- [ ] `- [ ] 1`

> Review note (r_glm, APPROVE-WITH-FINDINGS, resolved): the test arranges by
> installing the stub child via `install_ready_process` instead of the
> pool-spawned `get_or_create_slot` path. Accepted deviation: production
> acquisition requires an mTLS connect a plaintext-ready stub cannot satisfy,
> so the pool-spawn happy path is untestable at unit tier; the accessor chain
> under test is identical and the deviation is documented in the test's doc
> comment.

### Task 2: jms_crash_probe publishes the bridge handoff

Files:

- `crates/camel-test/src/bin/jms_crash_probe.rs`

Steps:

1. Keep a pool handle: `let pool = std::sync::Arc::new(pool);` then
   `JmsComponent::with_scheme("jms", pool.clone())`.
2. Add a pure helper `bridge_handoff_bytes(probe_pid: u32, bridge_pid: u32,
   supervisor_pgid: i32) -> String` producing one-line JSON
   `{"probe_pid":…,"bridge_pid":…,"supervisor_pgid":…}` (serde_json).
3. Add `publish_bridge_handoff(pool: &JmsBridgePool, xdg_state_home: &Path) ->
   Result<(), String>`:
   - bounded poll (deadline 5s, step 200ms, `tokio::time::sleep` — main
     scope, not a test body) for `pool.bridge_pid(BROKER_NAME)`; `None` at
     deadline → `Err` (setup-grade failure)
   - supervisor pgid: `#[cfg(unix)] libc::getpgid(pid as i32)`; on
     non-unix write `0`
   - destination
     `<xdg_state_home>/bridge-handoff-<std::process::id()>.json` via the
     existing `write_artifact_atomic`; map errors into `Err`
4. Call `publish_bridge_handoff(...)` as the FIRST statement of the route
   processor's async block, before the `match role` (so probe, survivor,
   threshold, and gated roles all publish). Map the error into
   `camel_api::CamelError::ProcessorError` — ordering guarantee: the marker
   or receipt becomes visible only after the handoff exists.
5. Extend the binary module docstring: the handoff artifact (path schema,
   JSON schema), and the topology paragraph (probe group holds only the
   probe; the bridge lives in the watchdog-led supervisor group; when the
   probe process dies the liveness-pipe EOF makes the watchdog SIGKILL that
   group).

Tests (compile + existing behavior):

- `cargo test -p camel-test --features integration-tests --test jms_test
  jms_crash_probe_missing_required_input -- --exact` still passes (no broker
  needed).
- `cargo test -p camel-test --bin jms_crash_probe` (artifact atomicity unit
  tests) still passes.

Acceptance criteria:

- [ ] fmt + clippy (`-p camel-test`) exit 0
- [ ] `- [ ] 2`

### Task 3: Test support validation + crash-proof restructure + regression guard

Files:

- `crates/camel-test/tests/support/jms_crash_probe.rs`
- `crates/camel-test/tests/support/jms_crash_probe_cases.rs`

Steps:

1. `jms_crash_probe.rs`:
   - Rewrite the module docstring: document the topology (probe group vs
     watchdog-led supervisor group, who owns the bridge, what dies when the
     probe dies) per the design doc.
   - Add `#[cfg(unix)] pub fn bridge_handoff_path(xdg_state_home: &Path,
     probe_pid: u32) -> PathBuf` →
     `<root>/bridge-handoff-<probe_pid>.json`.
   - Add `#[cfg(unix)] pub struct BridgeHandoff { pub bridge_pid: i32, pub
     supervisor_pgid: i32 }` and `#[cfg(unix)] pub fn
     read_bridge_handoff(path: &Path, expected_probe_pid: u32, expected_exe:
     &Path) -> Result<BridgeHandoff, String>` with validation order:
     (a) exists + parses as JSON with the three fields → else Err containing
     `probe group <expected_probe_pid>` and `supervisor group`;
     (b) `probe_pid` field == expected → else Err containing `stale`,
     `probe group <expected>`, and `supervisor group <from file>`;
     (c) bridge pid alive non-zombie (`/proc/<pid>/stat`);
     (d) canonicalized `/proc/<pid>/exe` == canonicalized `expected_exe`;
     (e) pgrp cross-check: `process_group_of(bridge_pid) ==
     Some(supervisor_pgid)` and `supervisor_pgid != expected_probe_pid as
     i32` (topology proof).
   - Remove `find_group_process_by_exe` (no remaining callers after step 2).
     Keep `live_group_members`, `process_exe`, `process_group_of`.
2. `jms_crash_probe_cases.rs` — rewrite `run_crash_before_completion_proof`:
   - imports: drop `find_group_process_by_exe`, add `bridge_handoff_path`,
     `read_bridge_handoff`.
   - after the marker/journal pre-kill checks: `let handoff =
     read_bridge_handoff(&bridge_handoff_path(xdg_root.path(), probe_pgid as
     u32), probe_pgid as u32, &binary).unwrap_or_else(|e| panic!("{e}"));`
     `let bridge_pid = handoff.bridge_pid; let supervisor_pgid =
     handoff.supervisor_pgid;`
   - keep `owner_before == Some(bridge_pid as u32)` exactly as-is.
   - after `probe.kill_leader_only()` and the probe-death wait, REPLACE the
     survival assertion with the death phase:
     `wait_until("watchdog killed the exact owner bridge", 30s, 100ms, …
     Ok(!process_alive(bridge_pid)))` — panic message on timeout MUST name
     the watchdog and both groups (bridge survived ⇒ watchdog broken);
     `wait_until("supervisor group empty", 30s, 100ms, …
     Ok(live_group_members(supervisor_pgid).is_empty()))`;
     bounded lock-release observation: `wait_until("destination lock
     released at bridge exit", 30s, 250ms, … Ok(lock_owner_pid(&lock) !=
     Ok(Some(bridge_pid as u32))))` with the final observed owner in the
     panic message. Remove the rc-icvzl branch and the two
     `find_group_process_by_exe` re-assertions (post-kill and success-path).
   - survivor phase unchanged (receipt id/body/header, `reservation_count ==
     2`, two RESERVE frames, no COMMIT). Read the survivor's handoff too
     (path from the survivor's pgid) and after
     `survivor.kill_and_reap()` + `probe.kill_and_reap()` wait bounded for
     the survivor's bridge pid to be gone. Drop the old
     `live_group_members(probe_pgid)` wait (the probe group no longer holds
     the bridge).
   - update the final `PROOF 5.2/5.3` eprintln: replace
     `exact_orphan_pid/orphan_alive_at_assert` with
     `bridge_pid`, `supervisor_pgid`, `bridge_died_after_probe=true`,
     `supervisor_group_empty=true`, released-lock owner.
   - keep `assert!(survivor.is_alive(), …)` and the journal-after checks
     exactly as they are.
3. Regression guard (no broker) in `jms_crash_probe_cases.rs`:
   - name: `jms_crash_probe_handoff_missing_or_stale_names_both_groups`
   - arrange: tempdir; expected probe pid 4242; expected exe
     `Path::new("/bin/sh")` canonicalized
   - act/assert 1 (absent): `read_bridge_handoff(&bridge_handoff_path(dir,
     4242), 4242, exe)` → Err; message contains `probe group 4242` and
     `supervisor group`
   - act/assert 2 (stale): write a handoff JSON naming `probe_pid: 7777`,
     `bridge_pid: 1`, `supervisor_pgid: 1` at the pid-7777 path; read with
     expected 4242 → Err; message contains `stale`, `probe group 4242`,
     `supervisor group 1`
   - plain `#[test]` (no broker, no runtime).

Tests:

- `cargo test -p camel-test --features integration-tests --test jms_test
  jms_crash_probe_handoff_missing_or_stale -- --exact --nocapture` passes.
- `cargo test -p camel-test --features integration-tests --test jms_test
  --no-run` compiles (docker legs run after this task).

Acceptance criteria:

- [ ] fmt + clippy (`-p camel-test`) exit 0; `cargo xtask
  lint-unbounded-wait` stays at ceiling 0
- [ ] `- [ ] 3`

## Phase 2: Verification (conductor-run)

- [x] 4. `cargo build --workspace` green; fmt/clippy/lint gates recorded;
  docker `cargo test -p camel-test --features integration-tests --test
  jms_test` green; both crash tests 10x loop zero failures.
