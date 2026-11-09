# Design: instrumented-timing-test

## Approach

`group_timeouts_are_independent_per_clause`
(`crates/languages/camel-language-rhai/src/lib.rs`, `#[cfg(test)]` module,
~line 5001) calibrates a Rhai loop count so one clause runs ~120 ms
(`target = 0.6 x TIMEOUT_MS`), then proves a two-clause group with per-clause
timeouts lets both clauses complete: each clause < 160 ms
(`per_clause_bound`), sum > 200 ms (`sum_floor`), `first_match` returns
`None`, `eval_log() == [0, 1]`. Observed: in CI run 42635743339 the
4-attempt rescale loop never landed the workload inside the required window
while the coverage harness ran the suite. Suspected: under
`-C instrument-coverage` the per-iteration cost shifts between calibration
and measurement, so the rescale chases a perturbed quantity (the test is
also load-flaky under `-j4`, bd rc-gb4un). Fix, test-side only: detect an
instrumented run and split the variant.

1. **Detection — runtime env only, probed first.** Candidates: (A1)
   `CARGO_LLVM_COV`, (A2) `LLVM_PROFILE_FILE` — whichever the probe verifies
   is exported to the test process by cargo-llvm-cov. The probe runs the
   scoped crate suite three times — normal (expect negative),
   `cargo llvm-cov -p camel-language-rhai --lib` with a throwaway test
   printing the env (expect positive), normal again (expect negative) — and
   records the cargo-llvm-cov version and, for the park, whether
   `cfg(coverage)` is also visible (observation only). Compile-time
   `#[cfg(coverage)]` is explicitly NOT the committed mechanism: the crate's
   Cargo.toml carries `[lints] workspace = true`, a local `[lints.rust]`
   check-cfg registration would conflict with the inherited workspace lints
   ("cannot override `workspace.lints` in `lints`"), and no source-level
   check-cfg declaration exists. If the probe verifies neither env var, the
   order's fallback applies — no manifest surgery is in scope. Runtime
   detection has zero normal-path surface.
2. **Instrumented path — semantic completion variant (weaker by design).**
   A small static loop count (`SEMANTIC_LOOP_COUNT = 20_000`, normal runtime
   a few ms — an order of magnitude of headroom under instrumentation) drives
   `budget_loop_script`; the group is `rhai_group(&lang, [script, script])`
   with the same per-clause timeout language. Assertions, wall-clock-free:
   `first_match(&exchange)` returns `None` (a cut clause surfaces as an
   `ExpressionError` of class `Timeout` indexed to that clause, so `None`
   proves no timeout fired on either clause) and `eval_log() == [0, 1]`
   (both clauses evaluated, in order). Honest scope: this is a completion
   smoke test of the group with per-clause timeouts armed — two short
   clauses would also complete under a shared or absent timeout, so it does
   NOT discriminate shared vs per-clause budgets; only the unchanged strict
   path attempts that discrimination. Residual deadline dependence: fixed
   iterations are not fixed runtime — the variant stays bounded by the
   200 ms per-clause production timeout (which includes `spawn_blocking`
   waits), so a pathologically throttled runner can still miss the deadline.
   Mitigation: the headroom is ~10x and verified by three consecutive
   instrumented runs, at least one concurrent with other build load; if it
   proves insufficient, only the instrumented path is adjusted (its own
   timeout/count), or the order's fallback applies.
3. **Non-instrumented path — strict variant, byte-identical budgets.**
   `TIMEOUT_MS = 200`, `OP_LIMIT = 1_000_000_000`, `target = 0.6 x timeout`,
   `per_clause_bound = 0.8 x timeout`, `sum_floor = timeout` unchanged;
   calibration, the 4-attempt rescale loop, and the strict assertions
   unchanged.
4. **Test-only diagnostics.** Each attempt `eprintln!`s its measurement line
   (count, both clauses, outcomes) so `--nocapture` shows calibration on the
   success path, not only inside the panic message. Both variants print
   which branch ran (`strict` / `semantic-instrumented`).

**Fallback (expected unused).** If the probe verifies neither mechanism,
`scripts/coverage.sh` skips the test by name
(`-- --skip group_timeouts_are_independent_per_clause`) with a comment
pointing at rc-gb4un, and the park records why the test-side fix was
rejected. Nothing else in the script may change.

## Affected crates

- `camel-language-rhai`: test module only (`src/lib.rs`, `#[cfg(test)]`).
  No manifest edit: the detection mechanism is runtime env, and the crate's
  `[lints] workspace = true` rules out a local lint override.

## Architecture boundaries

Languages tier only; no Runtime, DSL, Components, or Services surface; no
production code. Canon check: `test-determinism` requirements (sleep-as-sync,
bounded await loops, quarantine) stay binding and unengaged — no sleep, no
new unbounded await, no ignore/quarantine; `lint-test-sleep` and
`ignore-test-policy` are unaffected. No canon owns
instrumentation-calibrated timing-test policy, so the change sets
`skip_specs: true` (precedent: `test-count-honesty-a`, which likewise
preserved existing canon). Testing contracts ADR-0064/ADR-0069/ADR-0072 are
untouched: this is a Languages-tier unit test fixing its own harness.
`lint-unbounded-wait` and `lint-test-sleep` scans stay clean because the
change only adds a branch, a static count, and eprintln diagnostics inside
an existing test body.

## Verification

1. `cargo test -p camel-language-rhai --lib` green;
   `cargo test -p camel-language-rhai --lib group_timeouts -- --nocapture`
   green with per-attempt measurements printed and branch `strict`.
2. Branch-specific coverage verification. Detector verified:
   `cargo llvm-cov -p camel-language-rhai --lib` green, 3 consecutive runs
   (one concurrent with other load), branch `semantic-instrumented` printed;
   summary tail quoted in the park (crate-scoped only — never
   `--workspace`). Fallback applies: the scoped coverage run greens with
   `-- --skip group_timeouts_are_independent_per_clause` and the rc-gb4un
   comment, and the park records the probe evidence and why the test-side
   fix was rejected. Strict-path checks (item 1) stay mandatory in either
   branch.
3. `bash scripts/gates-local.sh` exit 0 (network gates stay CI-owned);
   `cargo fmt --check --all` clean.
4. Sweep `crates/languages/**` (and the rhai crate) for the same
   calibration-then-tight-assert pattern; hazards listed in the park, not
   fixed here (bd follow-ups if confirmed).
