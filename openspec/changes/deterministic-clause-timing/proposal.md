# Proposal: deterministic-clause-timing

## Why

bd rc-abggo (P3, discovered-from rc-gb4un). The strict branch of
`group_timeouts_are_independent_per_clause`
(`crates/languages/camel-language-rhai/src/lib.rs`) calibrates a rhai loop to
~120 ms of wall clock, asserts each clause < 160 ms and the pair > 200 ms, and
retries up to four times with rescaling. Under a busy builder
(`-j4 workspace --lib`) that can still miss — mission 528 found rc-gb4un
exactly this way, and mission 557 (`e2dcd3012`) made only the
instrumentation path deterministic. The retries make the flake rare, not
impossible. A test that needs a quiet machine to pass is a liability in a
fleet that runs gates under load by design.

The strict branch is the only test that proves **per-clause budgets are
independent**: the group lets both clauses run, and a shared budget would cut
the pair. That property must not be lost.

## What Changes

Replace the strict branch's wall-clock mechanism (calibration + duration
assertions + retry loop) with **exact rhai operation counts**:

- An operation probe (test-only, in the same `mod tests`): a
  `create_base_engine` engine with an `on_progress` callback capturing the
  running operation total, mirroring the production scope shape
  (`body = "x"`, empty header/property maps). The probe returns the EXACT
  integer operation cost of a workload script.
- The strict branch then asserts, in exact integers: a two-workload-clause
  group under a per-clause budget `L` (chosen strictly between one and two
  workload costs) completes with `eval_log = [0, 1]`; a single evaluation
  carrying both workloads under the same `L` (the shared-budget
  counterfactual) is cut with `ExpressionErrorClass::Limit`.
- The measured quantities (single workload ops, double workload ops, budget,
  per-iteration cost) are printed via `eprintln!` on every run.
- The instrumentation (`semantic`) branch stays unchanged.

Excluded: production code changes, Cargo/dependency changes, changes to the
timeout-cut test (`group_clause_timeout_is_indexed_and_stops`) and the
existing operation-budget tests (`group_budgets_do_not_accumulate_across_clauses`,
`group_budget_workloads_are_validated_standalone`, `group_budget_breach_is_indexed`).

## Acceptance criteria

- 20 consecutive runs of the test, all green, measured op counts shown.
- Same under induced CPU load (documented in the mission report).
- Negative proof: with the two clauses temporarily made to share one budget
  (temporary production edit, reverted), the test fails.
- `cargo llvm-cov -p camel-language-rhai --lib --no-report -- --nocapture`
  green with `branch=semantic-instrumented` observed.
- `cargo test -p camel-language-rhai --lib` green; `bash scripts/gates-local.sh`
  exit 0; `cargo fmt --check --all` clean.

## Risk budget

- Acceptable: losing the wall-clock dimension of THIS test — the timeout cut
  path keeps coverage in `group_clause_timeout_is_indexed_and_stops`, and the
  residual risk (a regression moving `tokio::time::timeout` outside the
  clause loop) is documented in design.md.
- Out of bounds: production changes, dependency changes, sleeps, lowering
  existing timeout/operation constants elsewhere in the suite.
