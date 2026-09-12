# Tasks: jobsignals

## `camel-cli` signal and report control flow

### Task 1.1: Add signal-aware job wait and teardown flow

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/signal.rs` (new)

**Steps:**
1. Arm Unix SIGINT and SIGTERM streams at the first lines of `run_job`, before config loading; provide the non-Unix Ctrl+C equivalent, preserve both streams until the first signal is consumed, and emit a flushed `signal streams armed` tracing marker for subprocess synchronization.
2. Add private `JobWaitOutcome` and `async fn await_job_operation_or_signal<S, F>(signal: S, operation: F) -> JobWaitOutcome` using `tokio::select! { biased; signal first, operation second }`, with concrete bounds matching the existing send and drain futures.
3. Use the helper for send and batch drain, mapping a signal to an `Interrupted` report and preserving signal-first precedence over completion and timeout.
4. On interruption, cancel the active operation, run `shutdown` with `shutdown_budget` (one-shot floor, batch no-floor), then write the report; record `shutdown_error` only for non-zero budgets.
5. After the first signal, spawn a force-exit task that owns the registered signal streams, exits 1 on the next signal, and abort it after teardown completes. Keep early validation/boot failures stderr-only.
6. Extract `fn exit_code_for(outcome: &str) -> i32` from the inline mapping, add `Interrupted` coverage through it, and preserve Completed/Failed/Timeout behavior.

**Tests:**
- `job_report_interrupted_serializes`: arrange an Interrupted `JobReport` with a shutdown detail; act with `serde_json::to_value`; assert `outcome` is `Interrupted` and `shutdown_error` is present.
- `job_exit_code_interrupted`: arrange an Interrupted report; act through the extracted outcome-code helper; assert code 2.
- `job_signal_wins_ready_tie`: arrange ready signal and operation futures; act through `await_job_operation_or_signal`; assert signal outcome.
- `job_interrupted_shutdown_budget_by_mode`: arrange interrupted one-shot and batch branches with matching deadlines; act through the interruption teardown path; assert one-shot selects at least `MIN_SHUTDOWN_BUDGET` and batch selects the remaining deadline without a floor.
- `job_interrupted_shutdown_failure_preserves_verdict`: arrange an interrupted report and a non-zero-budget teardown error; act through report finalization; assert `outcome` remains `Interrupted`, `shutdown_error` is set, and exit code is 2.
- Command: `cargo test -p camel-cli --lib job::` for unit cases.
- Before implementation, each new test must fail for the missing contract; after implementation, all named tests pass.

**Acceptance:**
- First SIGINT/SIGTERM during boot, send, or batch drain yields `Interrupted` and exit 2 with bounded teardown.
- Second SIGINT/SIGTERM during teardown force-exits 1; first signal cannot be consumed by force-exit task.
- Existing success, failure, timeout, shutdown, and batch completion tests in `crates/camel-cli/tests/job_one_shot_test.rs` remain passing under `cargo test -p camel-cli --test job_one_shot_test`; the existing unit suite covers zero-budget stderr-only behavior.
- `cargo fmt --check --all` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.1

> Apply-time notes (review resolution): the `signal streams armed`
> marker is a flushed `eprintln!` on stderr, not a tracing event —
> the tracing subscriber installs only later in boot, after arming.
> The report/exit-code/shutdown-budget unit tests live in
> `crates/camel-cli/src/commands/job/tests.rs` (split from `mod.rs`
> for file-size hygiene; test names unchanged). Arming is gated on
> document presence: the listing path (`args.document is None`)
> never installs the streams — armed-then-dropped handlers would
> swallow SIGINT/SIGTERM during listing — while document-path entry
> arming before config load is unchanged.

### Task 1.2: Document camel job signal and outcome contract

**Files:**
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. Add a `camel job` signal handling section that states entry registration, first-signal cancellation, one-shot/batch teardown budgets, Interrupted report/exit 2, and second-signal force exit 1.
2. Add `Signal interruption` to the `camel job failure modes` table and update the outcome list to include `Interrupted`.
3. Update line anchors for affected `run_job` log-policy entries after code changes and keep prose in English.

**Tests:**
- `lint_context_citations_job_signal_contract`: arrange the updated context document and source anchors; act with `cargo xtask lint-context-citations`; assert exit 0 and no stale `run_job` anchors.
- Command: `cargo xtask lint-context-citations`.
- Before implementation, the new signal row and contract are absent; after implementation, the lint passes.

**Acceptance:**
- Documentation states exact signal, outcome, and exit-code behavior, including budget asymmetry and Tokio signal coalescing.
- `cargo xtask lint-context-citations` exits 0.

- [x] 1.2

## Test harness integration

### Task 2.1: Add subprocess signal regression coverage

**Files:**
- `crates/camel-cli/tests/job_signal_test.rs` (new)

**Steps:**
1. Reuse the existing subprocess helpers (`spawn_camel_job`, signal delivery, and flushed trust-model warning) rather than adding a second process protocol.
2. Create minimal temporary `Camel.toml`, `jobs/*.job.yaml`, and route fixtures with a blocking `seda:` route for one-shot interruption and a non-empty queue for batch drain interruption.
3. Assert JSON reports through `--report`, process exit codes, absence of shell signal exits 130/143, and bounded force-exit timing.
4. Include the INT-then-TERM pair to cover both signal streams and retain a separate single-SIGTERM boot-buffer test.

**Tests:**
- `job_signal_interrupts_one_shot`: setup temp project and held `seda:` job; action send SIGTERM after marker; assert report `Interrupted`, exit 2, and report `error` mentions signal.
- `job_signal_interrupts_batch_drain`: setup batch document with active drain; action send SIGINT; assert report `Interrupted` and exit 2.
- `job_signal_second_signal_forces_exit`: setup held job; action send SIGINT then SIGTERM; assert exit 1 and no completed report is required.
- `job_signal_during_boot_is_buffered`: setup a slow boot fixture with a flushed `signal streams armed` marker emitted immediately after registration; action send SIGTERM after that marker and before the trust-model warning; assert exit 2 and report `Interrupted`, not 130/143.
- Command: `cargo test -p camel-cli --test job_signal_test`.
- Before implementation, tests fail because `run_job` has default signal disposition; after implementation, all pass.

**Acceptance:**
- Subprocess tests deterministically cover first SIGINT, first SIGTERM, boot buffering, batch drain, and second-signal force exit.
- `cargo test -p camel-cli --test job_signal_test` exits 0 without leaving child processes.

- [x] 2.1
