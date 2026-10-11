# Tasks: 362-jsleak

Single coherent slice: no delivery phases. All commands run from the worktree
`/home/shared/rust-camel-worktrees/362-jsleak` with
`CARGO_TARGET_DIR=/home/shared/rust-camel-worktrees/362-jsleak/target CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp`;
never in the main checkout. Stop if the build disk is at or above 78% full.
Bd: rc-jo85f. Spec hash at plan start:
`sha256:3a7556845c3dbd431304e271c65831ae53baa3392489b2e0eec7af3f127d9316`.

## camel-language-js: integrity drift (primary)

### Task 1.1: Realm-leak regression tests first (red before the fix)

**Files:**
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)

**Steps:**
1. In the existing `#[cfg(test)] mod tests` in `worker.rs` (after
   `no_recycle_storm_on_steady_state`, around line 1344), add the six tests
   listed below. Reuse the existing `worker_state()` helper (line 810) and
   `eval_now(&mut state, source, JsExchange::default())` (line 781); read the
   private `state.recycle_count` field directly, the blessed test seam.
2. Do NOT modify `no_recycle_storm_on_steady_state`: it is the `camel.body`
   zero-recycle control and must stay green in every task.

**Tests:**
- `var_global_does_not_recycle_realm`: `let mut state = worker_state();` then
  100 × `eval_now(&mut state, "var y = 1; y", JsExchange::default())`; assert
  `state.recycle_count == 0`. Command
  `cargo test -p camel-language-js --lib var_global_does_not_recycle_realm`.
  Expected BEFORE 1.3: FAIL (counter equals 100). AFTER 1.3: pass.
- `function_global_does_not_recycle_realm`: as above with
  `"function f() {}; 0"`. Expected BEFORE: FAIL. AFTER: pass.
- `undeclared_global_does_not_recycle_realm`: as above with `"z = 1; z"`.
  Expected BEFORE: FAIL. AFTER: pass.
- `async_global_does_not_recycle_realm`: as above with
  `"async function af() {}; 0"`. Expected BEFORE: FAIL. AFTER: pass.
- `configurable_globals_are_scrubbed_between_evals`: `worker_state()`; eval
  `"var y = 1; z = 2; 0"`; then eval `"[typeof y, typeof z].join(',')"`; assert
  the second `return_value.as_str() == Some("undefined,undefined")` and
  `state.recycle_count == 0`. Expected BEFORE: typeof assertion passes but
  counter is 1 (FAILS on the counter). AFTER: pass.
- `nonconfigurable_addition_still_recycles`: `worker_state()`; eval
  `"Object.defineProperty(globalThis, 'pwn', { value: 1, configurable: false }); 0"`;
  then eval `"typeof pwn"`; assert the second result is `"undefined"` and
  `state.recycle_count == 1`. Expected BEFORE and AFTER: pass (negative
  control, must never flip).

**Acceptance:**
- `cargo test -p camel-language-js --lib global_does_not_recycle_realm` before
  1.3 shows the four new realm tests failing; the two negative controls and
  `no_recycle_storm_on_steady_state` pass.
- `cargo test -p camel-language-js --lib` compiles (only assertion failures).

- [ ] 1.1

### Task 1.2: Pure added-global descriptor classifier with unit tests

**Files:**
- `crates/languages/camel-language-js/src/engines/integrity.rs` (modified)

**Steps:**
1. Near `key_sets_equal` (line 208), add a private free function:
   `fn added_global_is_drift(desc: Option<&PropertyDescriptor>) -> bool` that
   returns `match desc { Some(d) => d.configurable() != Some(true), None => true }`.
   Doc comment: a configurable global added after the baseline is scrubbed by
   the pre-eval pass and is not drift; a non-configurable addition is drift;
   an unreadable descriptor fails closed as drift. `PropertyDescriptor` is
   already imported (line 25).
2. At the end of `integrity.rs`, add `#[cfg(test)] mod tests` that imports
   `super::added_global_is_drift`, `boa_engine::property::PropertyDescriptor`,
   and `boa_engine::JsValue`.

**Tests:**
- `configurable_added_global_is_not_drift`: build
  `PropertyDescriptor::builder().value(JsValue::undefined()).writable(true).enumerable(true).configurable(true).build()`;
  assert `!added_global_is_drift(Some(&desc))`. Command
  `cargo test -p camel-language-js --lib engines::integrity::tests`. Expected
  BEFORE the fn exists: filter matches 0 tests (cargo exits 0 with "0 passed"
  — not a pass). AFTER: 1 match, passing.
- `nonconfigurable_added_global_is_drift`: same builder with
  `.configurable(false)`; assert `added_global_is_drift(Some(&desc))`.
- `absent_added_global_descriptor_is_drift`: assert
  `added_global_is_drift(None)` (fail closed).

**Acceptance:**
- `cargo test -p camel-language-js --lib engines::integrity::tests` reports
  exactly 3 passing tests.
- No new `pub` or `pub(crate)` API is introduced; the helper is private.

- [ ] 1.2

### Task 1.3: Wire addition-aware verification and drift/recycle diagnostics

**Files:**
- `crates/languages/camel-language-js/src/engines/integrity.rs` (modified)
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)

**Steps:**
1. Replace the first check in `IntegrityBaseline::verify` (integrity.rs
   lines 182-186) with: `let global = ctx.global_object();` then
   `let keys = global.own_property_keys(ctx).unwrap_or_default();` build a
   `HashSet<&PropertyKey>` from `keys`; if any `self.global_keys` entry is
   absent, emit
   `tracing::debug!(cause = "global-key-set", "JS realm integrity drift detected")`
   and `return false;`. Then for each `key` in `&keys` not in
   `self.global_keys`: `let desc = global.borrow().properties().get(key);` and
   if `added_global_is_drift(desc.as_ref())`, emit the same
   `tracing::debug!(cause = "global-key-set", "JS realm integrity drift detected")`
   and `return false;`. Never log the key name, its value, or the source.
2. Keep `key_sets_equal` unchanged: `RootSnapshot::matches` still calls it
   (integrity.rs line 90). Update only the module doc comment at integrity.rs
   lines 1-18 to state that configurable global additions are ignored because
   the scrub removes them.
3. In `WorkerState::recycle` (worker.rs, after line 614
   `self.recycle_count += 1;`) add
   `tracing::debug!(recycle_count = self.recycle_count, "JS realm recycled");`.
   `tracing` is already a crate dependency (worker.rs line 44 import block
   does not need `tracing` imported for the macro path `tracing::debug!`).

**Tests:**
- The four Task 1.1 realm tests now pass:
  `cargo test -p camel-language-js --lib global_does_not_recycle_realm`
  → 4 passed.
- `cargo test -p camel-language-js --lib` → all green, including the existing
  `no_recycle_storm_on_steady_state`, `frozen_prototype_recycles_realm`,
  `failed_eval_residue_is_cleaned`, `date_prototype_persists_until_recycle`,
  and the new `nonconfigurable_addition_still_recycles`,
  `configurable_globals_are_scrubbed_between_evals`.

**Acceptance:**
- `cargo test -p camel-language-js --lib` exits 0.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0
  (no unused `key_sets_equal`).
- `cargo xtask lint-log-levels` and `cargo xtask lint-log-redaction` exit 0.
- Diagnostics contain no user-controlled key, source, or value.

- [ ] 1.3

### Task 1.4: Diagnostic coverage with tracing-test

**Files:**
- `crates/languages/camel-language-js/Cargo.toml` (modified)
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)

**Steps:**
1. Add `tracing-test = { workspace = true, features = ["no-env-filter"] }` to
   `[dev-dependencies]` in
   `crates/languages/camel-language-js/Cargo.toml` (the workspace entry exists
   at the root `Cargo.toml` line 194; `camel-health` is the existing
   precedent).
2. In `worker.rs` `#[cfg(test)] mod tests`, add one traced test.

**Tests:**
- `drift_and_recycle_emit_debug_without_user_data`: annotate with
  `#[tracing_test::traced_test]` and `#[test]`; `let mut state = worker_state();`;
  eval `"Object.defineProperty(globalThis, 'pwn', { value: 1, configurable: false }); 0"`
  (forces drift and a recycle); assert
  `logs_contain("JS realm integrity drift detected")` and
  `logs_contain("JS realm recycled")`; assert `!logs_contain("pwn")` and
  `!logs_contain("Object.defineProperty")`. Command
  `cargo test -p camel-language-js --lib drift_and_recycle_emit_debug_without_user_data`.
  Expected BEFORE 1.3: FAIL (no debug events). AFTER: pass.

**Acceptance:**
- `cargo test -p camel-language-js --lib drift_and_recycle_emit_debug_without_user_data`
  passes and the test asserts absence of the user-controlled strings.
- `cargo xtask lint-log-redaction` exits 0.

- [ ] 1.4

## camel-language-js: promise-job retention (secondary)

### Task 1.5: In-crate discarding job executor installed without a retaining fallback

**Files:**
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)

**Steps:**
1. Add imports to the worker.rs import block (lines 44-55):
   `use std::rc::Rc;`,
   `use boa_engine::context::ContextBuilder;`,
   `use boa_engine::job::{Job, JobExecutor};`, and gate the counter import
   with `#[cfg(test)] use std::cell::Cell;`.
2. Add, near `WorkerState`, a
   `#[derive(Default)] struct DiscardingJobExecutor` with a test-gated field
   `#[cfg(test)] dropped: Cell<u64>`. Implement `impl JobExecutor for DiscardingJobExecutor`:
   `fn enqueue_job(self: Rc<Self>, _job: Job, _context: &mut Context)`
   increments `self.dropped` under `#[cfg(test)]` only and otherwise does
   nothing (job is dropped, never retained);
   `fn run_jobs(self: Rc<Self>, _context: &mut Context) -> boa_engine::JsResult<()>`
   returns `Ok(())`. Doc comment: `script:` is synchronous (ADR-0006) and its
   promise jobs must not be retained; callbacks do not run.
3. Add `#[cfg(test)] impl DiscardingJobExecutor { fn dropped_count(&self) -> u64 { self.dropped.get() } }`.
4. In `WorkerState::new` (worker.rs line 367), replace `let mut ctx = Context::default();`
   with:
   `let executor = Rc::new(DiscardingJobExecutor::default());`
   `let mut ctx = ContextBuilder::new().job_executor(executor).build().expect("Building the worker JS context should not fail"); // allow-unwrap`
   Add a comment: `Context::default()` itself panics when its builder fails,
   so this preserves that exact behavior; on failure the worker MUST panic,
   never fall back to `Context::default()` (the retaining `SimpleJobExecutor`).

**Tests:**
- `discarding_executor_installed`: `let state = worker_state();` assert
  `state.ctx.downcast_job_executor::<DiscardingJobExecutor>().is_some()`.
  Command `cargo test -p camel-language-js --lib discarding_executor_installed`.
  Expected BEFORE: FAIL (default executor). AFTER: pass.

**Acceptance:**
- `cargo test -p camel-language-js --lib discarding_executor_installed` passes.
- `rg -n "Context::default" crates/languages/camel-language-js/src/engines/worker.rs`
  returns no production use (test-only references are allowed).
- The `expect` line carries `// allow-unwrap`; `cargo xtask lint-unwrap`
  exits 0.

- [ ] 1.5

### Task 1.6: Job-drop regression — delta equals N with zero recycles, plus post-recycle delta and error path

**Files:**
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)

**Steps:**
1. Read the executor via the helper below in each test:
   `let exec = state.ctx.downcast_job_executor::<DiscardingJobExecutor>().expect("discarding executor installed");`
2. Add the three tests. Source under test:
   `"Promise.resolve(1).then(() => {}); 0"` (one PromiseJob per eval, per the
   blessed probe: 20k evals → 20k dropped jobs).

**Tests:**
- `then_jobs_dropped_exactly_n_with_zero_recycles`: `worker_state()`; assert
  the fresh baseline `exec.dropped_count() == 0`; run `N = 50` evals of the
  source; assert `exec.dropped_count() - baseline == 50` and
  `state.recycle_count == 0`. Command
  `cargo test -p camel-language-js --lib then_jobs_dropped_exactly_n_with_zero_recycles`.
  Expected BEFORE 1.5: FAIL (default executor retains; no counter). AFTER:
  pass.
- `then_drops_after_forced_recycle`: `worker_state()`; eval
  `"Object.defineProperty(globalThis, 'pwn', { value: 1, configurable: false }); 0"`
  to force one recycle; assert `state.recycle_count == 1`; snapshot
  `before = exec.dropped_count()`; run 50 evals of the source; assert
  `exec.dropped_count() - before == 50` and `state.recycle_count == 1` (no
  additional recycle, executor neither dropped nor replaced by the retaining
  default).
- `error_path_jobs_are_dropped`: `worker_state()`; snapshot
  `before = exec.dropped_count()`; eval
  `"Promise.resolve(1).then(() => {}); throw new Error('boom')"` and assert it
  returns `Err`; assert `exec.dropped_count() - before == 1` and
  `state.recycle_count == 0` (cleanup on the error path retains nothing).

**Acceptance:**
- `cargo test -p camel-language-js --lib then_` and the documented filters
  pass.
- No `run_jobs()` call is added to production code: `rg -n "run_jobs" crates/languages/camel-language-js/src`
  finds only the executor's `Ok(())` impl.
- Memory flatness is NOT asserted by these unit tests; the blessed probe
  (20k evals plateau 17–31MB RSS; 20k jobs dropped) is the spec-mandated
  evidence recorded in the blessing.

- [ ] 1.6

### Task 1.7: Body write-back is unaffected by undrained jobs (359 guard)

**Files:**
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)

**Steps:**
1. Add one test proving that promise callbacks never mutate the result because
   jobs are discarded, so the Task 1.5 executor cannot change 359's
   write-back contract.

**Tests:**
- `then_callbacks_do_not_mutate_body`: build a mutating `eval_now` result with
  source
  `"camel.body = 'assigned'; Promise.resolve().then(() => { camel.body = 'from-then'; }); 'done'"`;
  assert the returned `JsEvalResult.body` equals `Value::String("assigned")`
  (the `.then` callback never runs) and `state.recycle_count == 0`. Command
  `cargo test -p camel-language-js --lib then_callbacks_do_not_mutate_body`.
  Expected after 1.5/1.6: pass; it fails if anyone later introduces draining.
- Existing `tests/integration.rs::test_mutating_expression_error_propagated`
  must stay green (error path leaves the exchange unchanged).

**Acceptance:**
- `cargo test -p camel-language-js` exits 0 (unit + integration tests).
- `then_callbacks_do_not_mutate_body` pins the write-back contract.

- [ ] 1.7

## camel-language-js: docs and gates

### Task 1.8: Update crate docs and run the local gates

**Files:**
- `crates/languages/camel-language-js/CONTEXT.md` (modified)
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)
- `crates/languages/camel-language-js/src/engines/integrity.rs` (modified)

**Steps:**
1. Update `CONTEXT.md` "Sandbox posture": state that configurable global
   additions (top-level `var`/`function`/`async function`, undeclared
   assignment) are removed between evaluations and do NOT trigger a recycle;
   only a non-configurable addition, a vanished baseline key, or an unreadable
   added-global descriptor recycles the realm. State that promise jobs are
   discarded at enqueue because `script:` is synchronous (ADR-0006) and their
   callbacks do not run. Add the declared non-guarantee: a script that now
   scrubs-and-reuses instead of recycling keeps its untracked state within the
   realm generation, exactly as existing `camel.body`-style scripts already
   do.
2. Align the `worker.rs` and `integrity.rs` module doc comments (lines 1-42
   and 1-18 respectively) with the same wording.
3. Do NOT edit `openspec/specs/js-engine-cache/spec.md`: the canonical spec is
   synced only through the OpenSpec archive workflow.
4. Run the gates below. Stop and report if the build disk reaches 78%.

**Tests:**
- `cargo test -p camel-language-js` green (all unit + integration tests).
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-language-js --no-deps` exits 0.

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-unwrap`, `cargo xtask lint-log-levels`,
  `cargo xtask lint-log-redaction`, `cargo xtask lint-context-citations`,
  `cargo xtask lint-unbounded-wait`, and `cargo xtask lint-test-sleep` each
  exit 0.
- Canonical `openspec/specs/js-engine-cache/spec.md` is byte-unchanged.
- No production behavior outside camel-language-js changed.

- [ ] 1.8
