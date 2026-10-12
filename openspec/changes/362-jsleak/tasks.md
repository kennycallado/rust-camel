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
   `no_recycle_storm_on_steady_state`, around line 1344), add the five tests
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
  the second `return_value.as_str() == Some("undefined,undefined")` AND
  `state.recycle_count == 0`. Expected BEFORE 1.3: FAIL on the counter (the
  first eval recycles, counter 1); the typeof assertion alone would pass, so
  the counter assertion is the red one. AFTER: pass.
- Do NOT add a duplicate non-configurable control: the existing
  `undeletable_global_recycles_realm` already covers it and must stay green.

**Acceptance:**
- FIVE added regressions fail before 1.3: the four
  `*_global_does_not_recycle_realm` tests plus
  `configurable_globals_are_scrubbed_between_evals`. Commands:
  `cargo test -p camel-language-js --lib global_does_not_recycle_realm` → 4
  failed; `cargo test -p camel-language-js --lib configurable_globals_are_scrubbed_between_evals`
  → 1 failed.
- Exactly two controls are green before 1.3 — the existing non-configurable
  control `undeletable_global_recycles_realm` and the existing `camel.body`
  control `no_recycle_storm_on_steady_state`. Commands:
  `cargo test -p camel-language-js --lib undeletable_global_recycles_realm` →
  pass; `cargo test -p camel-language-js --lib no_recycle_storm_on_steady_state`
  → pass.
- `cargo test -p camel-language-js --lib` compiles; the five failures are
  assertion-only, not compile errors.

- [x] 1.1

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

- [x] 1.2

### Task 1.3: Wire addition-aware verification and drift/recycle diagnostics

**Files:**
- `crates/languages/camel-language-js/src/engines/integrity.rs` (modified)
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)

**Steps:**
1. Replace the global-key check in `IntegrityBaseline::verify` (integrity.rs
   lines 182-186) with an addition-aware check that logs a distinct static
   cause on each drift branch. All drift messages share the prefix `JS realm
   integrity drift detected`; never log a key name, a value, or the source.
   Start with `let global = ctx.global_object();` then
   `let keys = global.own_property_keys(ctx).unwrap_or_default();` build a
   `HashSet<&PropertyKey>` from `keys`. If a `self.global_keys` entry is
   absent, emit
   `tracing::debug!("JS realm integrity drift detected: global key set")` and
   `return false;`. Then for each `key` in `&keys` not in `self.global_keys`:
   `let desc = global.borrow().properties().get(key);` and if
   `added_global_is_drift(desc.as_ref())`, emit the same global-key-set
   message and `return false;`.
2. Instrument the two currently-silent protected-root branches of `verify`
   (integrity.rs lines 187-193). Retain the
   `let Some(eval_object) = self.eval_object() else { return false; }` branch
   and prefix it with
   `tracing::debug!("JS realm integrity drift detected: eval baseline missing")`.
   Before the `_ => return false` arm of the `global.get(js_string!("eval"),
   ctx)` match that guards `strict_equals`, emit
   `tracing::debug!("JS realm integrity drift detected: eval identity")`.
   Leave the named-root loop's
   `tracing::debug!(root = root.name, "JS realm integrity drift detected")`
   (line 197) unchanged: `root.name` is a fixed compile-time label (`eval`,
   `Object.prototype`, `Array.prototype`, `Function.prototype`).
3. Keep `key_sets_equal` unchanged: `RootSnapshot::matches` still calls it
   (integrity.rs line 90). Update only the module doc comment at integrity.rs
   lines 1-18 to state that configurable global additions are ignored because
   the scrub removes them.
4. In `WorkerState::recycle` (worker.rs, after line 614
   `self.recycle_count += 1;`) add
   `tracing::debug!(recycle_count = self.recycle_count, "JS realm recycled");`.
   `tracing` is already a crate dependency (no import needed for the
   `tracing::debug!` macro path).

**Tests:**
- The four Task 1.1 realm tests now pass:
  `cargo test -p camel-language-js --lib global_does_not_recycle_realm`
  → 4 passed.
- `cargo test -p camel-language-js --lib` → all green, including the existing
  `no_recycle_storm_on_steady_state`, `frozen_prototype_recycles_realm`,
  `failed_eval_residue_is_cleaned`, `date_prototype_persists_until_recycle`,
  `undeletable_global_recycles_realm`, `poisoned_eval_recovers`, and the new
  `configurable_globals_are_scrubbed_between_evals`.

**Acceptance:**
- `cargo test -p camel-language-js --lib` exits 0.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0
  (no unused `key_sets_equal`).
- `cargo xtask lint-log-levels` and `cargo xtask lint-log-redaction` exit 0.
- Diagnostics contain no user-controlled key, source, or value.

- [x] 1.3

### Task 1.4: Diagnostic coverage for every drift branch (tracing-test)

**Files:**
- `crates/languages/camel-language-js/Cargo.toml` (modified)
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)
- `crates/languages/camel-language-js/src/engines/integrity.rs` (modified)

**Steps:**
1. Add `tracing-test = { workspace = true, features = ["no-env-filter"] }` to
   `[dev-dependencies]` in
   `crates/languages/camel-language-js/Cargo.toml` (the workspace entry exists
   at the root `Cargo.toml` line 194; `camel-health` is the existing
   precedent).
2. In `worker.rs` `#[cfg(test)] mod tests`, add the two traced tests below.
3. In `integrity.rs` `#[cfg(test)] mod tests`, add the third traced test. It
   builds the degenerate baseline directly: `let mut ctx =
   boa_engine::Context::default();`, then `let keys =
   ctx.global_object().own_property_keys(&mut ctx).unwrap();` (append
   `// allow-unwrap`), then construct `IntegrityBaseline { global_keys: keys,
   roots: Vec::new() }` (the fields are private, but this module owns the
   type) and call `.verify(&mut ctx)`.

**Tests:**
- `drift_and_recycle_emit_debug_without_user_data`:
  `#[tracing_test::traced_test]` + `#[test]`; `worker_state()`; eval
  `"Object.defineProperty(globalThis, 'pwn', { value: 1, configurable: false }); 0"`
  (forces global-key drift and a recycle); assert
  `logs_contain("JS realm integrity drift detected: global key set")` and
  `logs_contain("JS realm recycled")`; assert `!logs_contain("pwn")` and
  `!logs_contain("Object.defineProperty")`. Command
  `cargo test -p camel-language-js --lib drift_and_recycle_emit_debug_without_user_data`.
  Expected BEFORE 1.3: FAIL (no debug events). AFTER: pass.
- `eval_identity_drift_emits_debug_without_user_data`:
  `#[tracing_test::traced_test]` + `#[test]`; `worker_state()`; eval
  `"eval = 1; 0"` (replaces the global `eval` own value); assert
  `logs_contain("JS realm integrity drift detected: eval identity")` and
  `logs_contain("JS realm recycled")`; assert `!logs_contain("eval = 1")` (the
  source text is never logged). Command
  `cargo test -p camel-language-js --lib eval_identity_drift_emits_debug_without_user_data`.
  Expected BEFORE 1.3: FAIL. AFTER: pass.
- `empty_eval_root_verify_logs_eval_baseline_missing`:
  `#[tracing_test::traced_test]` + `#[test]` in integrity.rs; build the
  degenerate baseline as in step 3; assert `verify` returns `false` and
  `logs_contain("JS realm integrity drift detected: eval baseline missing")`.
  Command
  `cargo test -p camel-language-js --lib empty_eval_root_verify_logs_eval_baseline_missing`.
  Expected BEFORE 1.3: FAIL. AFTER: pass.

**Acceptance:**
- All three diagnostic tests pass, each asserting the static cause message and
  the absence of user-controlled strings.
- `cargo xtask lint-log-redaction` exits 0.

- [x] 1.4

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
- `context_rebuild_reinstalls_discarding_executor`: `let mut state =
  worker_state();` run 10 evals of `"Promise.resolve(1).then(() => {}); 0"` so
  the dropped counter is non-zero; then execute the same full-context
  reconstruction the panic path uses — `state =
  WorkerState::new(resolve_js_limits(&JsLimitsConfig::default()));` — and
  assert the rebuilt state's executor downcasts to `DiscardingJobExecutor`
  with `dropped_count() == 0` (fresh, never the retaining default). This is
  the cheapest meaningful coverage of the reconstruction path; do NOT add
  panic injection, because `worker_self_heals_after_panic` already drives the
  rebuild and would only re-assert this same constructor. Command
  `cargo test -p camel-language-js --lib context_rebuild_reinstalls_discarding_executor`.
  Expected after 1.5/1.6: pass.

**Acceptance:**
- `cargo test -p camel-language-js --lib discarding_executor_installed` and
  `cargo test -p camel-language-js --lib context_rebuild_reinstalls_discarding_executor`
  pass.
- `rg -n "Context::default" crates/languages/camel-language-js/src/engines/worker.rs`
  returns no production use (test-only references are allowed).
- The `expect` line carries `// allow-unwrap`; `cargo xtask lint-unwrap`
  exits 0.

- [x] 1.5

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

- [x] 1.6

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

- [x] 1.7

## camel-language-js: docs and gates

### Task 1.8: Update crate docs and run the local gates

**Files:**
- `crates/languages/camel-language-js/CONTEXT.md` (modified)
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)
- `crates/languages/camel-language-js/src/engines/integrity.rs` (modified)

**Steps:**
1. Update `CONTEXT.md` "Sandbox posture" without narrowing the recycle
   triggers to global keys alone. State that the addition-aware global-key
   check removes configurable additions (top-level `var`/`function`/`async
   function`, undeclared assignment) between evaluations without a recycle,
   and that a recycle still happens on the global-key drift causes: a
   non-configurable addition, a vanished baseline key, or an unreadable
   added-global descriptor. Retain the protected-invariant / named-root
   recovery wording: `eval` identity drift (and a missing `eval` baseline)
   and descriptor/prototype drift in the named `Object`/`Array`/`Function`
   roots also recycle, enforced by install-verify plus the integrity check.
   State that promise jobs are discarded at enqueue because `script:` is
   synchronous (ADR-0006) and their callbacks do not run. Add the declared
   non-guarantee: a script that now scrubs-and-reuses instead of recycling
   keeps its untracked state within the realm generation, exactly as existing
   `camel.body`-style scripts already do.
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

- [x] 1.8

## mission handoff (non-code)

### Task 1.9: Upstream Boa issue draft and ROOT inbox handoff (no submit)

**Files:**
- `/home/kenny/dev/rust-camel/.opencode/fleet/inbox/362-jsleak-parked.json` (new; conductor-owned, outside the worktree)

**Steps:**
1. Draft a minimal upstream Boa issue for the 0.22.0 realm leak. Title:
   `boa_engine 0.22.0: Context::create_realm() leaks the previous realm (~310KB per rebuild)`.
   Body: the crate version, host OS/toolchain, and a step-by-step
   reproduction — `Context::default()`; loop `ctx.create_realm()`, enter the
   new realm, evaluate `1`, drop all realm handles, optionally call
   `boa_gc::force_collect()`; observe RSS growing ~310KB per iteration
   (6.2GB over 20k iterations) while the old realm is never reclaimed.
2. Attach the `boaleak` minimal-repro reference (probe
   `/tmp/nix-shell.X9cy9Q/opencode/boaleak`, KB source
   `js-leak-research-2026-10-11`) and the expected behavior: the previous
   realm, its globals, and its intrinsics become collectable once
   unreferenced.
3. Record the draft in the mission report. The conductor owns the final
   handoff and writes it as a JSON object to the ROOT inbox path in `Files`.
   Do NOT run `gh issue create`, `gh api`, `git push`, or any network submit.

**Tests:**
- Non-code: `test -f /home/kenny/dev/rust-camel/.opencode/fleet/inbox/362-jsleak-parked.json`
  exits 0 and
  `python3 -c "import json,sys; json.load(open(sys.argv[1]))" /home/kenny/dev/rust-camel/.opencode/fleet/inbox/362-jsleak-parked.json`
  exits 0.
- `rg -n "create_realm" /home/kenny/dev/rust-camel/.opencode/fleet/inbox/362-jsleak-parked.json`
  finds the reproduction.

**Acceptance:**
- The parked JSON exists at the ROOT inbox path, parses, and contains the
  issue title and body with the `create_realm` reproduction and the `boaleak`
  reference.
- No network or submit command was run (no `gh` invocation).
- The conductor owns the final handoff; this task only prepares and parks the
  draft.

- [x] 1.9
