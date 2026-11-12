# Tasks: clitesthook

Single-phase change (design.md omits `## Phases`): seam → deterministic test
→ stability evidence. Task order is load-bearing: 1.2 executes against the
1.1 seam.

## camel-core + camel-cli

### Task 1.1: env-gated pipeline-entry marker after the route drain claim

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)

**Steps:**
1. In the sequential route controller loop, locate the per-exchange drain
   claim: `let _drain_guard = DrainGuard::new(Arc::clone(&drain_in_flight));`
   (followed by the `claimfamily` sibling split onto `exchange.in_flight_claim`
   and the `let mut result = CANCEL_TOKEN.scope(cancel, async move {
   pipeline.call(exchange).await }).await;` call).
2. Immediately BEFORE the `let mut result = CANCEL_TOKEN.scope(...)`
   statement (thus after `_drain_guard` is bound and `in_flight_claim` is
   split), insert exactly:
   ```rust
   // Test seam (default off, mirrors `CAMEL_JOB_SIGNAL_MARKER` /
   // `CAMEL_JOB_SHUTDOWN_MARKER`): proves route pipeline entry with the
   // drain claim held, so job-signal tests can synchronize INT on an
   // observed teardown-holding exchange instead of a timing guess.
   if std::env::var_os("CAMEL_JOB_PIPELINE_MARKER").is_some() {
       eprintln!("camel job: pipeline entered");
   }
   ```
   No other change to the loop. No once-guard, no caching: per-dispatch
   gated emission is the blessed design (design.md §Approach 1).
3. Verify the insertion point: the emission must be lexically AFTER the
   `_drain_guard` binding and BEFORE the `CANCEL_TOKEN.scope` pipeline call
   in the same loop iteration.
4. Run `cargo fmt --check -p camel-core` and
   `cargo clippy -p camel-core --all-targets -- -D warnings`; fix any issue.
5. Commit the seam.

**Tests:**
- `camel-core regression sweep`: setup — the worktree at HEAD of task 1.1;
  action — run `cargo test -p camel-core --lib`; assert — exit 0 with no
  new failures vs the pre-change baseline (baseline: all green on `main`).
  command: `cargo test -p camel-core --lib`; expected: pass (the seam is
  inert with the env var unset — no test sets `CAMEL_JOB_PIPELINE_MARKER`
  in camel-core).
- `default-off proof (reviewer-checkable)`: setup — `git diff main...HEAD`
  restricted to `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs`;
  action — inspect the hunks; assert — the only added executable line is the
  `if std::env::var_os("CAMEL_JOB_PIPELINE_MARKER").is_some() { eprintln!(...) }`
  block (plus its doc comment), lexically between the `DrainGuard::new`
  binding and the `CANCEL_TOKEN.scope` pipeline call; no other behavior
  change in the loop.

**Acceptance:**
- `cargo fmt --check -p camel-core` exits 0.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-core --lib` exits 0.
- The diff shows the gated emission between the `DrainGuard::new` binding
  and the `CANCEL_TOKEN.scope` pipeline call (default-off proof).
- Worktree clean after commit.

- [x] 1.1

### Task 1.2: deterministic second-signal test (marker-synchronized)

**Files:**
- `crates/camel-cli/tests/job_signal_test.rs` (modified)
- `openspec/changes/clitesthook/evidence/negative-control.md` (new)

**Steps:**
1. Add three marker constants beside the existing `ARMED_MARKER` /
   `TRUST_MARKER` consts:
   ```rust
   /// Route-pipeline-entry seam marker (`CAMEL_JOB_PIPELINE_MARKER`):
   /// proves the teardown-holding exchange was claimed and dispatched
   /// with the drain counter held — the observation the blind pair
   /// lacked (bd rc-af1y3).
   const PIPELINE_ENTRY_MARKER: &str = "camel job: pipeline entered";
   /// Boot-complete INFO line (camel-core context lifecycle).
   const CONTEXT_STARTED_MARKER: &str = "CamelContext started";
   /// Shutdown-start INFO line (camel-core context lifecycle): the
   /// force-exit guard is spawned before shutdown begins, so observing
   /// this line proves the guard owns the signal streams.
   const STOPPING_CONTEXT_MARKER: &str = "Stopping CamelContext";
   ```
2. Rewrite `job_signal_second_signal_forces_exit` IN PLACE (same fn name,
   same three assertions), replacing the blind `sh -c "kill -INT;
   kill -TERM"` pair body with the observed sequence:
   - setup: `tempdir`, `write_config` (log_level INFO),
     `write_sequential_held_one_shot_fixture` (the held one-shot fixture with
     route-level `sequential: true` — seda:work, `delay: 30000`, Always-wait,
     `timeout: 60s`; see design.md §Approach 2 amendment for why the flag is
     required);
   - spawn: `spawn_camel_job_with_env(dir.path(), Path::new("job.job.yaml"),
     &[], &[("CAMEL_JOB_PIPELINE_MARKER", "1")])`, then `spawn_drained`;
   - act 1: `wait_for_marker(..., CONTEXT_STARTED_MARKER, WAIT)` then
     `wait_for_marker(..., PIPELINE_ENTRY_MARKER, WAIT)`; assert both reached
     (failure message must include `drained.captured()` like the sibling tests);
   - act 2: anchor `let int_sent = Instant::now();`, then
     `send_signal(&child, "-INT")`;
   - act 3: `wait_for_marker(..., STOPPING_CONTEXT_MARKER, WAIT)`; assert reached;
   - act 4: `send_signal(&child, "-TERM")`;
   - assert: `let code = wait_exit_code_bounded(&mut child, WAIT);`
     `let elapsed = int_sent.elapsed();` `let output = drained.finish();`
     then the three assertions byte-identical to the current test:
     `assert_eq!(code, 1, "the second stop signal must force-exit 1; ...")`,
     `assert!(elapsed < FORCE_EXIT_BOUND, "the force exit must stay bounded; ...")`,
     `assert!(output.contains("forcing exit"), "expected the ... WARN; ...")`
     (keep the existing assertion messages verbatim).
3. Negative control (proves the test depends on the observed marker, per the
   blessed spec): temporarily comment out the three seam lines from task 1.1,
   run `cargo test -p camel-cli --test job_signal_test
   job_signal_second_signal_forces_exit --exact`, and confirm the test FAILS
   on the `PIPELINE_ENTRY_MARKER` wait (marker-never-reached assertion);
   restore the seam lines exactly, re-run, confirm green, and prove the
   restore byte-identical: `git diff --exit-code
   crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs`
   must exit 0 (record that exit code in the evidence file too). Record
   both outcomes (commands + exit codes) in
   `openspec/changes/clitesthook/evidence/negative-control.md` (new) and
   commit it with the test.
4. Remove `use std::process::Command;` ONLY if the compiler/clippy reports
   it unused after the rewrite (`cargo check -p camel-cli --tests`); keep
   `Instant` (elapsed anchor). Update the test's doc
   comment: INT after the observed pipeline-entry marker (worker claim
   observed), TERM after `Stopping CamelContext`, drain hold from the claimed
   30 s-delay exchange keeps the guard window seconds wide; the pair comment
   about `sh -c` exec gaps is deleted with the pair.
5. Confirm no OTHER test in the file uses `spawn_camel_job`'s no-env path in
   a way this rewrite breaks (the rewrite switches THIS test to
   `spawn_camel_job_with_env`; sibling tests are untouched).
6. Run `cargo fmt --check -p camel-cli` and
   `cargo clippy -p camel-cli --all-targets -- -D warnings`; fix any issue.
7. Run the full signal-test file twice back-to-back (determinism smoke);
   both runs must be fully green.
8. Commit the test.

**Tests:**
- `job_signal_second_signal_forces_exit` (rewritten): setup — tempdir with
  INFO config + sequential held one-shot fixture
  (`write_sequential_held_one_shot_fixture`: 30 s delay, 60 s timeout,
  route-level `sequential: true` — see design.md §Approach 2 amendment), child
  spawned with `CAMEL_JOB_PIPELINE_MARKER=1`; action — wait
  `CamelContext started` + `camel job: pipeline entered`, send INT, wait
  `Stopping CamelContext`, send TERM; assert — exit code == 1 AND elapsed
  (from just before INT) < 15 s AND captured output contains `forcing exit`.
  command: `cargo test -p camel-cli --test job_signal_test
  job_signal_second_signal_forces_exit`; expected: pass with the 1.1 seam
  in place (fails — wrong exit or marker timeout — without it).
- `full-file regression`: action — `cargo test -p camel-cli --test
  job_signal_test`; assert — all tests in the binary pass (the rewritten
  test plus the untouched siblings: interrupts one-shot, interrupts batch
  drain, boot-buffered, and the four listing tests); expected: pass, run
  twice consecutively.

**Acceptance:**
- The blind pair (`sh -c` + two kills) no longer appears anywhere in the
  file; exactly one test asserts the second-signal force-exit contract.
- `cargo fmt --check -p camel-cli` exits 0.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-cli --test job_signal_test` fully green twice in a row.
- Worktree clean after commit.

- [x] 1.2

### Task 1.3: stability evidence (≥50 runs) and touched-crate gate sweep

**Files:**
- `openspec/changes/clitesthook/evidence/stability-runs.jsonl` (new)
- `openspec/changes/clitesthook/evidence/summary.md` (new)

**Steps:**
1. Build once: `cargo test -p camel-cli --test job_signal_test --no-run`;
   capture the test-binary path from the output.
2. Unloaded sweep: run the rewritten test 50 times by invoking the captured
   test binary directly 50 times
   (`<test-binary> job_signal_second_signal_forces_exit --exact --test-threads=1`),
   one JSON line per run in `evidence/stability-runs.jsonl`:
   `{"run": N, "mode": "unloaded", "exit": <code>, "elapsed_s": <f64>}` —
   `elapsed_s` is the WALL time of the whole binary invocation (spawn to
   exit), and the outlier threshold below applies to that wall time (the
   test's internal INT→exit bound stays enforced by `FORCE_EXIT_BOUND`
   inside the test itself).
3. Compute in `evidence/summary.md`: total runs, pass count (exit 0),
   min/median/max wall elapsed, outliers (any run with wall elapsed ≥ 10 s
   or any non-zero exit — any non-zero exit, INCLUDING `timeout`'s 124, is
   a FAIL in the pass/fail tally — a FAILURE of this task's acceptance that
   must be investigated, never folded into the wall-time outlier count).
   Also state the negative-control outcome from
   `evidence/negative-control.md` (task 1.2).
4. Load sweep (best effort, exactly one batch): start one CPU spinner per
   core (`sh -c 'while :; do :; done' &` × nproc, tracked PIDs) with the
   spinner kills registered in a `trap ... EXIT` handler, run the test 10
   additional times (same `--exact --test-threads=1` invocation, each run
   wrapped in `timeout 60`) recording `mode: "loaded"` lines, then let the
   trap kill all spinner PIDs on exit. If the host forbids the load or the
   batch would exceed 20 minutes, record `mode: "loaded-skipped"` in
   summary.md with the reason — skipping is acceptable, unbounded retries
   are not.
5. Touched-crate gate sweep (all from the worktree):
   `cargo fmt --check --all`;
   `cargo clippy -p camel-core --all-targets -- -D warnings`;
   `cargo clippy -p camel-cli --all-targets -- -D warnings`;
   `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-core -p camel-cli --no-deps`;
   `bash scripts/gates-local.sh`;
   record every exit code in summary.md's gate table. The gate table must
   be exhaustive: gates that `gates-local.sh` skips by default (network and
   docs-toolchain gates: `lint-commits`, `doc-guide-test`, `cargo-audit`)
   are listed as `skipped (CI)` — never silently omitted; `lint-commits`
   additionally skipped by conductor policy (remote fetch op).
6. Commit the evidence.

**Tests:**
- `stability distribution`: setup — task 1.2 green; action — 50 unloaded
  invocations of the test binary; assert — 50/50 exit 0 and every elapsed
  recorded; command: the step-2 loop; expected: 50/50 pass, median well
  under 15 s, outliers documented if any.
- `gate sweep`: action — the step-5 commands; assert — every command exits
  0 (gates-local.sh exit 0 required; a pre-existing failure unrelated to
  this change must be verified on `main` and reported, never silently
  skipped); expected: all green.

**Acceptance:**
- `evidence/stability-runs.jsonl` holds ≥50 `unloaded` lines, all exit 0.
- `evidence/summary.md` states pass count, elapsed distribution, outlier
  handling, and the gate table with per-gate exit codes.
- Load sweep either recorded (10 `loaded` lines) or explicitly skipped with
  reason.
- Worktree clean after commit.

- [x] 1.3
