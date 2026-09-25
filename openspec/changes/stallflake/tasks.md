# Tasks: stallflake

## Phase 1: deterministic watchdog tests

### Task 1.1 — dev-dependency and invoke-stall test rewrite

- [x] 1.1

**Files**

- `crates/components/camel-component-wasm/Cargo.toml` (modified)
- `crates/components/camel-component-wasm/src/runtime.rs` (modified)

**Steps**

1. In `crates/components/camel-component-wasm/Cargo.toml`
   `[dev-dependencies]`, add
   `tokio = { workspace = true, features = ["test-util"] }`
   after the `camel-language-api.workspace = true` line.
2. In `crates/components/camel-component-wasm/src/runtime.rs`, change
   the attribute of `invoke_stall_progress_resets_timer` from
   `#[tokio::test]` to `#[tokio::test(start_paused = true)]`.
3. Keep the test body as is: 4 pings 25 ms apart, then
   `ds.notify_one()`, watchdog `Some(Duration::from_millis(40))`,
   drain timeout 60 s, assert `result.is_ok()`.
4. Update the comment above the pinger loop to state that paused
   time makes the 25/40 ms values ordering inputs, not wall-clock
   margins.

**Tests**

- name: `invoke_stall_progress_resets_timer`
- setup: virtual clock paused at start; watchdog timeout 40 ms.
- action: pinger emits 4 `notify_one` calls 25 virtual ms apart,
  then signals drain_started; the drive future returns `Ok(())`.
- assert: `result.is_ok()` holds; the clock never passes a timeout
  deadline while a permit is unconsumed, so the run is
  deterministic.
- command: `cargo test -p camel-component-wasm --lib invoke_stall`
- expected: passes deterministically after the rewrite; passes in
  well under one second of real time.

**Acceptance**

- `cargo test -p camel-component-wasm --lib invoke_stall` exits 0.
- The rewritten test finishes in under one second of real time
  (paused clock, no wall-clock waits).

### Task 1.2 — drain-chunks test rewrite

- [x] 1.2

**Files**

- `crates/components/camel-component-wasm/src/runtime.rs` (modified)

**Steps**

1. Change the attribute of `drain_watchdog_passes_when_chunks_flow`
   from `#[tokio::test]` to `#[tokio::test(start_paused = true)]`.
2. Keep the test body as is: `ds.notify_one()` first, then 5 pings
   10 ms apart, drain timeout 50 ms, `invoke_stall_timeout` `None`,
   assert `result.is_ok()`.
3. Update the test comment to name the paused-clock determinism.

**Tests**

- name: `drain_watchdog_passes_when_chunks_flow`
- setup: virtual clock paused at start; drain timeout 50 ms.
- action: drain_started first, then 5 progress pings 10 virtual ms
  apart; the drive future returns `Ok(())`.
- assert: `result.is_ok()` holds deterministically.
- command:
  `cargo test -p camel-component-wasm --lib drain_watchdog_passes_when_chunks_flow`
- expected: passes deterministically after the rewrite.

**Acceptance**

- `cargo test -p camel-component-wasm --lib drain_watchdog` exits 0
  (all four drain_watchdog tests, including the untouched
  trips-on-stall tests).

### Task 1.3 — mutation evidence and gates

- [x] 1.3

**Files**

- `openspec/changes/stallflake/mutation-evidence.md` (new)

**Steps**

1. Apply the mutant from design.md "Mutation check" in
   `crates/components/camel-component-wasm/src/runtime.rs`: arm the
   Phase 1 sleep once before the loop
   (`let mut stall_sleep = Box::pin(tokio::time::sleep(t));` and
   `_ = &mut stall_sleep` as the select arm), leaving the progress
   `continue` branch untouched.
2. Run
   `cargo test -p camel-component-wasm --lib invoke_stall_progress_resets_timer`.
   Capture the failure output.
3. Revert the mutant without touching Task 1.1/1.2 work: before step 1,
   copy the file aside
   (`cp crates/components/camel-component-wasm/src/runtime.rs /tmp/runtime-pre-mutant.rs`);
   after the run, restore it
   (`cp /tmp/runtime-pre-mutant.rs crates/components/camel-component-wasm/src/runtime.rs`)
   and confirm with
   `git -C <worktree> diff --stat` that only the expected rewrites
   remain.
4. Write `openspec/changes/stallflake/mutation-evidence.md` with the
   captured failure output and one sentence that ties it to the
   design.md mutant.
5. Run the gates from the mission order: `cargo fmt --check --all`;
   the four clippy legs from AGENTS.md scoped to this change
   (`cargo clippy --workspace --all-features --exclude camel-cli
   --exclude camel-component-kafka --exclude security-keycloak
   --exclude security-wasm-policy -- -D warnings`, then the kafka,
   cli default, and cli no-default legs);
   `cargo test -p camel-component-wasm --lib` (178+ tests);
   `cargo xtask lint-test-sleep`;
   `cargo xtask lint-unbounded-wait`;
   `cargo xtask lint-ignore`;
   `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core
   -p camel-builder -p camel-dsl -p camel-endpoint --no-deps`.

**Tests**

- name: mutation check (manual, evidence-only)
- setup: Task 1.1 and 1.2 rewrites present in the working tree.
- action: apply the timer-reset mutant, run the invoke_stall test.
- assert: the test fails with `invoke stalled` (the broken reset is
  detected); after revert, the suite passes.
- command:
  `cargo test -p camel-component-wasm --lib invoke_stall_progress_resets_timer`
- expected: fail under the mutant, pass after revert.

**Acceptance**

- `mutation-evidence.md` exists and contains the captured failing
  run.
- All gate commands above exit 0 after the revert.
- `cargo test -p camel-component-wasm --lib` reports 178+ passed,
  0 failed.
