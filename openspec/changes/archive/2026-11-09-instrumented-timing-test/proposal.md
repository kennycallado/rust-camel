# Proposal: instrumented-timing-test

## Why

Observed: CI run 42635743339 (push of 9bdfd099b, 2026-11-09) failed the
Coverage Check job. `scripts/coverage.sh` runs
`cargo llvm-cov --workspace --lib --summary-only`; the suite hit
`tests::group_timeouts_are_independent_per_clause`
(`crates/languages/camel-language-rhai/src/lib.rs:5065`) and cargo exited 101,
so the TOTAL/threshold code never ran. Every other job in the run was green.
Panic: `independent-timeout workload validation failed after four attempts;
required each < 160ms and sum > 200ms; attempt 3: count=196436, first=82.70ms`.

Suspected, not yet established: the test calibrates a Rhai loop count so one
clause runs ~120 ms (`target = 0.6 x 200ms`) and rescales up to four attempts
from the slower observed clause. Under `-C instrument-coverage` the
per-iteration cost may shift between calibration and measurement (coverage
counters; the builder is loaded), so the rescale chases a perturbed quantity;
the same test is already load-flaky under `-j4` (bd rc-gb4un). This mission
fixes the observable failure mode; root-causing the perturbation beyond what
the probe needs is out of scope.

## What Changes

- Test code only, in `crates/languages/camel-language-rhai/src/lib.rs`
  (`#[cfg(test)]` module): detect an instrumented run and split behavior.
  - Instrumented: a semantic completion variant — a two-clause group with
    per-clause timeouts must evaluate both clauses (`eval_log() == [0, 1]`)
    and return `None`, i.e. no clause is cut and no timeout false-fires.
    This is deliberately weaker than the strict path: it does not
    discriminate shared vs per-clause budget sizes.
  - Not instrumented: the existing strict timing variant, budgets unchanged
    (`TIMEOUT_MS = 200`, `OP_LIMIT`, `target`, `per_clause_bound`,
    `sum_floor` untouched).
- Test-only diagnostics: per-attempt measurements print under `--nocapture`
  on the success path too (today they surface only in the panic message).
- The test is never deleted; the non-instrumented budgets are never loosened.
- No production code change, no new dependency.
- `scripts/coverage.sh` fallback (skip by name) only if the test-side split
  cannot keep an honest assertion; expected unused.

Affected crates: `camel-language-rhai` (test module only).

## Acceptance criteria

- `cargo test -p camel-language-rhai --lib` green; the strict variant prints
  its per-attempt measurements under `--nocapture`.
- The instrumentation detection mechanism is verified empirically by a probe
  before it is relied on: normal run negative, instrumented run positive,
  normal run negative again; mechanism and cargo-llvm-cov version recorded.
- `bash scripts/gates-local.sh` exit 0; `cargo fmt --check --all` clean.
- Branch-specific coverage acceptance. Detector verified:
  `cargo llvm-cov -p camel-language-rhai --lib` green, repeated 3x (one
  concurrent with other load), the semantic variant exercised. Fallback
  applies: the scoped coverage run greens using the rc-gb4un named skip,
  and the park records why the test-side fix was rejected. In either
  branch the strict-path checks above stay mandatory.
- Sweep of `crates/languages/**` for the same calibration pattern performed;
  hazards listed in the mission park, not fixed here.

## Risk budget

Acceptable: a small static workload size for the instrumented variant only,
residual deadline dependence acknowledged — the variant stays bounded by the
200 ms per-clause production timeout, so instrumented headroom is verified by
repeated runs (including one concurrent with other load); if headroom proves
insufficient, only the instrumented path is adjusted or the order's fallback
applies. Out of bounds: production-code change in `camel-language-rhai`;
budget edits on the non-instrumented path; test deletion; `#[ignore]` or
quarantine as fixes (bd rc-gb4un stays open if a residual load-flake class
remains).
