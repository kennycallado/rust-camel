# Tasks: flakytests

## camel-language-js (amortization test de-flake, rc-86u4n)

### Task 1.1: Restructure the amortization measurement (warm-up + 6×150 batches, median via pure helper) and add the wall-clock-free negative test

**Files:**
- `crates/languages/camel-language-js/tests/js_engine_cache_amortization.rs` (modified)

**Steps:**
1. Add a test-file-local helper `fn assert_amortization_ratio(eval1_us: f64, batches: &[f64]) -> f64` with doc comment: sorts a copy of `batches`, computes the median (even count = mean of the two middle elements — `batches.len()` is 6, so elements `[n/2 - 1]` and `[n/2]` of the sorted slice), computes `ratio = eval1_us / median`, and `assert!(median < eval1_us / 4.0, "...")` where the panic message contains the 4x-floor wording, `eval1_us`, EVERY batch value, the median, and the computed ratio. Returns `ratio`.
2. Restructure `amortization_ratio_at_least_4x`: keep the existing eval1 block and both sanity asserts (`return_value`/`body` == `"hello"`) unchanged; after eval1, run an untimed warm-up loop of exactly 100 `engine.eval(SRC, make_exchange()).unwrap()` iterations; then run exactly 6 timed batches of 150 evals each — per batch, `Instant::now()` before, 150 evals, `elapsed.as_secs_f64() * 1e6 / 150.0` as the per-eval µs value pushed into a `Vec<f64>`.
3. Replace the existing steady-state mean computation and assertion with a call to `assert_amortization_ratio(eval1_us, &batches)`; keep the result in a variable `ratio` and replace the existing `println!` with one line per batch value plus a summary line printing `eval1_us`, the median (recompute via sorting inside the print, or have the helper also expose the median — keep ONE helper; printing the median again in the summary line is fine), and `ratio`.
4. Add the negative test `#[test] fn assertion_fails_on_regressed_steady_state()`: call `assert_amortization_ratio(2000.0, &[1900.0, 1950.0, 2000.0, 2050.0, 2100.0, 2150.0])` inside `std::panic::catch_unwind(std::panic::AssertUnwindSafe(...))`; assert the catch result is `Err` (the helper panicked) and the panic payload string contains `"4x"`.
5. Extend the module doc comment (`//!` header): keep the existing history table and recalibration prose; add a "Methodology (2026-11-12 amendment)" paragraph stating: explicit 100-eval untimed warm-up window; steady state measured as 6 batches of 150 evals; the assertion is the median of the batches under 25% of eval 1 (≥ 4x), via the pure helper; the CI motivation (run 43033768484: single mean 514.6 µs/eval vs dev ~350 µs, ratio 3.9x vs 4x floor, healthy cache); the labeled hypothesis that contention inflates a minority of batches; and a note that sustained whole-window contention still trips the floor (accepted false-positive direction). Do NOT add measured distribution tables in this task — Task 1.2 appends them from real runs.

**Tests:**
- `assertion_fails_on_regressed_steady_state`: setup: helper from step 1 exists → action: `catch_unwind(AssertUnwindSafe(|| assert_amortization_ratio(2000.0, &[1900.0, 1950.0, 2000.0, 2050.0, 2100.0, 2150.0])))` → assert: result is `Err` and the payload string contains `"4x"` (regressed distribution must panic; command: `cargo test -p camel-language-js --test js_engine_cache_amortization assertion_fails_on_regressed_steady_state -- --exact`; expected: pass after implementation, fail-to-compile before).
- `amortization_ratio_at_least_4x`: setup: restructured test from steps 2-3 → action: full run (cold eval1 + 100 warm-up + 6×150 timed batches) → assert: helper's median floor assertion holds (measured ratio ≥ 4x; dev-box calibration 5.5–6.2x) and the correctness sanity asserts (`"hello"`) still hold; command: `cargo test -p camel-language-js --test js_engine_cache_amortization amortization_ratio_at_least_4x -- --exact --nocapture`; expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-language-js --test js_engine_cache_amortization` exits 0 with both tests passing.
- `cargo fmt --check` exits 0 on the worktree.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0.
- No production file changed (`git status --short` shows only the test file).
- Commit created for this task.

- [x] 1.1

### Task 1.2: Stability evidence — 20 unloaded + 20 loaded runs, record measured distributions in the doc comment

**Files:**
- `crates/languages/camel-language-js/tests/js_engine_cache_amortization.rs` (modified)

**Steps:**
1. Run 20 consecutive unloaded runs of the timing test, capturing stdout per run into `target/flakytests-evidence/unloaded-<NN>.log` (create the directory; `target/` is gitignored — logs are never committed): `for i in $(seq -w 1 20); do cargo test -p camel-language-js --test js_engine_cache_amortization amortization_ratio_at_least_4x -- --exact --nocapture > target/flakytests-evidence/unloaded-$i.log 2>&1 || echo "RUN $i FAILED"; done`. Verify all 20 logs end in `test result: ok` and extract per-run `eval1`, batch values, median, and ratio lines.
2. Run 20 loaded runs under representative synthetic CPU contention with interruption-safe cleanup: `N=$(nproc)`; record the topology (`echo "nproc=$N" > target/flakytests-evidence/load-topology.txt`); initialize the PID file fresh (`: > target/flakytests-evidence/spinners.pids`); install the cleanup trap BEFORE spawning and cover interruption signals (`trap 'xargs -r kill < target/flakytests-evidence/spinners.pids' EXIT INT TERM`); spawn one busy-loop spinner per available CPU (`for i in $(seq 1 "$N"); do (while :; do :; done) & echo $! >> target/flakytests-evidence/spinners.pids; done`) so every CPU has a competitor; wait 2 seconds; run the 20-run loop into `target/flakytests-evidence/loaded-<NN>.log`; the trap kills only the recorded spinner PIDs, on the normal exit path and on INT/TERM alike. Verify all 20 logs end in `test result: ok`.
3. Compute per-condition summaries from the captured logs: for each condition, per run the eval1 value, ALL SIX per-batch µs/eval values, the median, and the ratio; then per condition the 20 ratio values with min/median/max and the per-batch µs/eval range across all runs. If ANY run failed, or the loaded-run median ratio is < 4.0, STOP and report `stability-evidence-failed: <details>` instead of editing the doc comment.
4. Append the measured tables to the module doc comment under the methodology paragraph: one table per condition (unloaded, loaded), one row per run — run# → eval1 µs, all six per-batch µs/eval values, steady median µs, ratio — plus a one-line summary per condition (min/median/max ratio; per-batch µs/eval range across all runs; `nproc` used for the loaded condition). Values come from the logs of steps 1-2 — no invented or placeholder numbers.
5. Re-run the full test binary once after the doc-comment edit (`cargo test -p camel-language-js --test js_engine_cache_amortization`) to confirm the edit touched only comments and the tests still pass.

**Tests:**
- `amortization_ratio_at_least_4x` (existing, from Task 1.1): the 40 evidence runs of step 1-2 are the test executions → assert per the spec's evidence clause: 20/20 unloaded pass, 20/20 loaded pass, loaded-run median ratio ≥ 4.5 (the blessed 4x test floor is unchanged; 4.5 is the evidence margin over it, against the 5.5–6.2x dev calibration); command: the loops of steps 1-2; expected: pass on the dev box per the calibration.

**Acceptance:**
- `target/flakytests-evidence/` holds 40 logs, each ending `test result: ok. 1 passed`.
- The doc comment contains both measured tables with real per-run batch values (no `TBD`/placeholder).
- `cargo fmt --check` and `cargo clippy -p camel-language-js --all-targets -- -D warnings` exit 0.
- `cargo check -p camel-language-js` exits 0 in the feature worktree (never the main checkout).
- `bash scripts/gates-local.sh` exits 0 in the feature worktree.
- `git status --short` shows only the test file modified; commit created for this task.

- [x] 1.2
