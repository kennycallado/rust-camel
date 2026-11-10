# ci-rhai-reconcile-control Specification

## Purpose
Run the rhai reconcile-budget control in CI, so the regression reproduction
from bd rc-m01r9 stays wired: the control compiles under
`feature = "lang-rhai"` and proves the workload boots, runs, and replies
with the expected shape on every change to the guarded crates.
## Requirements
### Requirement: integration-rhai workflow runs the reconcile control

The repository SHALL contain `.github/workflows/integration-rhai.yml` with
a job that runs
`cargo test -p camel-integration-test --features lang-rhai --test reconcile_budget_test`
on `workflow_dispatch` and on `pull_request`/`push` to `main` whose changed
paths include `crates/camel-integration-test/**`, `crates/camel-core/**`,
`crates/languages/camel-language-rhai/**`, `crates/camel-api/**`,
`crates/camel-language-api/**`, `Cargo.toml`, `Cargo.lock`, or the workflow
file itself. The job SHALL NOT run the `#[ignore]`d 5001-member bench
(no `--ignored`), and its timing comment SHALL state observations only,
never an assertion.

#### Scenario: Regression in the reconcile workload fails CI

- **GIVEN** a change to `camel-language-rhai` or `camel-core` that breaks
  the reconcile workload under `lang-rhai` (compile error, route boot
  failure, or wrong reply shape)
- **WHEN** the `integration-rhai.yml` job runs on that pull request
- **THEN** the job fails with the control's failure instead of the control
  compiling out silently

#### Scenario: Control stays out of unrelated runs

- **GIVEN** a pull request that touches no guarded path
- **WHEN** CI runs
- **THEN** `integration-rhai.yml` does not trigger, and the ignored 5001
  bench never executes in any CI run

