# Tasks: jobteardown

## camel-cli job execution

### Task 1.1: Add a single bounded teardown border for post-boot failures

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)

**Steps:**
1. Re-locate the current `execute_job` boot-handle acquisition and enumerate every early `return 2` after it, preserving each diagnostic and exit outcome.
2. Introduce an async inner execution boundary or equivalent single post-boot result boundary so route loading, URI validation, target selection, route registration, and `ctx.start()` failures return through one owner.
3. At that boundary, calculate the existing mode/deadline-aware shutdown budget and call the existing `shutdown(&mut ctx, &boot_handle, budget)` helper exactly once for each path that did not already shut down.
4. Keep the existing signal force-exit task, transport shutdown path, successful shutdown path, and batch/report behavior unchanged; do not add a second shutdown call to paths already covered.
5. Add an opt-in `CAMEL_JOB_SHUTDOWN_MARKER` observation in the shared shutdown helper, emitting exactly `camel job: shutdown complete` once after the bounded shutdown attempt when the environment variable is present, without changing production output by default.

**Tests:**
- `job_early_failure_shutdown_is_observed`: arrange a temporary job fixture with two consumer routes sharing the send-target base; spawn through `common::spawn_camel_job_with_args` with the shutdown marker environment enabled; act by waiting with `wait_for_marker`; assert stderr contains `camel job: shutdown complete` and `is ambiguous: 2 consumer routes share its base`, then assert `wait_exit_code_bounded` returns 2.
- `job_transport_failure_shutdown_is_observed_once`: arrange a deterministic `SendError::Transport` fixture by targeting a closed localhost HTTP port, allowing the connection-refused outer send failure to reach the existing transport branch after its retry window; enable the shutdown marker; assert stderr contains exactly one `camel job: shutdown complete`, a deterministic diagnostic substring naming the closed-port transport failure, and exit code 2.
- Existing signal tests in `crates/camel-cli/tests/job_signal_test.rs`: run unchanged and assert their existing markers, graceful exits, and force-exit behavior still pass.
- Existing job one-shot/batch tests: verify diagnostics, reports, exit codes, and unchanged behavior.

**Acceptance:**
- All post-boot failure exits use bounded teardown and existing diagnostics/exit code 2.
- The marker is opt-in and emitted exactly once per shutdown invocation; normal CLI output is unchanged when unset.
- `cargo fmt --check --all` and the targeted `camel-cli` tests pass.

- [x] 1.1

### Task 1.2: Add executable regression coverage and test helper support

**Files:**
- `crates/camel-cli/tests/job_early_failure_test.rs` (new)
- `crates/camel-cli/tests/common/mod.rs` (modified only if needed to pass the marker environment)

**Steps:**
1. Reuse the existing job integration harness and fixture-writing patterns rather than introducing a new process runner.
2. Add the duplicate-consumer fixture needed to reach the deterministic ambiguous-target branch after boot.
3. Add the named test `job_early_failure_shutdown_is_observed` with exact marker, diagnostic, and exit-code assertions.
4. Add the named transport regression test `job_transport_failure_shutdown_is_observed_once`. The originally considered closed-localhost HTTP fixture cannot reach the outer transport branch because job target gates reject that consumer scheme; use the deterministic equivalent `direct:go?block=true`, whose persistent endpoint-creation failure reaches outer `SendError::Transport` after the retry window. Assert a stable diagnostic substring rather than the runtime detail string.
5. Ensure both tests wait for the marker before collecting the final exit status so they distinguish teardown from an ordinary exit 2.

**Tests:**
- `job_early_failure_shutdown_is_observed`: pre-fix behavior must time out waiting for the marker; fixed behavior observes one marker and exits 2.
- `job_transport_failure_shutdown_is_observed_once`: fixed behavior observes exactly one marker and exits 2.
- `cargo test -p camel-cli --test job_early_failure_test -- job_early_failure_shutdown_is_observed --exact`
- `cargo test -p camel-cli --test job_early_failure_test -- job_transport_failure_shutdown_is_observed_once --exact`

**Acceptance:**
- The regression test is deterministic and exercises a real booted context.
- The test does not assert shutdown using exit code alone.
- The test compiles and passes in the worktree.

- [x] 1.2

## Verification

### Task 2.1: Run mission quality gates

**Files:**
- No source changes; verification only.

**Steps:**
1. Run targeted `camel-cli` job tests and the unchanged signal tests.
2. Run `cargo test --workspace --no-run` once for the required cross-crate compile check.
3. Run formatting, clippy/lint gates applicable to touched code, and the required rustdoc command for touched crates.
4. Inspect the diff and confirm only the job execution/test zone plus OpenSpec artifacts changed.

**Tests:**
- `cargo test --workspace --no-run`: workspace compiles without errors.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint --no-deps`: documentation builds without warnings.

**Acceptance:**
- All required gates pass, or any environmental blocker is recorded in the park report.
- No untracked generated files or unrelated changes remain.

- [x] 2.1
