# test-result-integrity Specification

## Purpose
TBD - created by archiving change test-result-integrity. Update Purpose after archive.
## Requirements
### Requirement: A red full-workspace run executes every test target that compiles

The `full-tests-linux` CI job SHALL run `Test (full workspace)` as
`cargo test --workspace --no-fail-fast` and
`Test camel-cli (exec flavor)` with `--no-fail-fast` appended. Each step
SHALL carry a comment referencing bd rc-9wx6u and stating the CI-time
budget: a red run is then expected to take about as long as a green one
(~85–115 min, inside the existing `timeout-minutes: 150`), and cargo
prints `error: N targets failed:` listing every failed target, instead
of stopping at the first failing binary with no count of the targets
that never ran. The flag SHALL NOT be advertised as guaranteeing
execution through compile failures or the job timeout: it eliminates
fail-fast aborts of *running* targets only.

#### Scenario: one failing binary no longer hides the rest

- **GIVEN** a commit whose workspace compiles and contains at least one
  failing test binary and at least one later-ordered passing test target
- **WHEN** the `Test (full workspace)` step runs
- **THEN** every test target that compiles executes (fail-fast
  aborting of remaining targets does not occur), the step exits nonzero,
  and cargo's closing error lists all failed targets

#### Scenario: green run target set is unchanged

- **GIVEN** a commit whose whole workspace passes
- **WHEN** the `Test (full workspace)` step runs
- **THEN** the executed target set is the same as before the flag
  existed (`--no-fail-fast` only changes failure behaviour), and the
  step stays inside the job's 150-minute timeout

#### Scenario: compile failure still stops execution

- **GIVEN** a commit where a test target fails to compile
- **WHEN** the `Test (full workspace)` step runs
- **THEN** the step fails at the compile error (execution of dependent
  targets does not happen and is not promised); this behaviour is
  unchanged from fail-fast and is covered by the compile-gate canon,
  not by this requirement

### Requirement: A failing earlier step must not skip the workspace test steps

The `Test (full workspace)` and `Test camel-cli (exec flavor)` steps in
the `full-tests-linux` job SHALL carry `if: ${{ !cancelled() }}` so a
failing earlier step (an integration-test leg or a setup step) no longer
silently skips them. Neither step SHALL gain any feature, path, label,
or outcome-keyed condition that can skip it on an otherwise-red run. The
job-level conclusion SHALL satisfy: **job success implies both required
test steps concluded `success`** (no claim is made about every other
step or about runs killed by the job timeout).

#### Scenario: integration-test failure does not skip the workspace suite

- **GIVEN** a commit where an earlier integration-test step (for example
  `JMS integration tests`) fails
- **WHEN** the `full-tests-linux` job runs
- **THEN** the workspace suite and the exec-flavor leg still execute
  (the run reports the whole workspace), and the job conclusion remains
  `failure` because the earlier step failed

#### Scenario: cancellation yields unusable evidence

- **GIVEN** a `full-tests-linux` run cancelled before completion
- **WHEN** the job conclusion is read
- **THEN** it reports `cancelled` — evidence unusable for a tested
  verdict (the release preflight treats it as untested), whether or not
  some tests executed before cancellation; and a fully green run's
  `success` reflects that both test steps ran and passed

### Requirement: The release checklist treats skipped or cancelled Full Tests as untested

`.opencode/prompts/bump-version.md` SHALL contain a checklist step,
placed before the local tag step, stating that a skipped or cancelled
Full Tests run means the commit is *not tested*, and giving the exact
check: `gh run list --workflow ci.yml --commit <SHA>` must show a
completed **successful push run on main** (PR-triggered rows do not
qualify — they test a synthetic merge commit), and the
`Full Tests (ubuntu-latest)` job with
its `Test (full workspace)` and `Test camel-cli (exec flavor)` steps
must all show `success` (via `gh run view <run-id> --json jobs`). The
step SHALL note that the release workflow enforces the same server-side
with the built-in `GITHUB_TOKEN` requiring the `actions: read`
permission scope, and that no secret setup is needed. The existing Rule
0b content in the version-bump prompt SHALL remain unchanged.

#### Scenario: checklist catches a skipped Full Tests before tagging

- **GIVEN** a commit whose latest CI run skipped the Full Tests job's
  workspace step because an earlier step failed
- **WHEN** the release checklist step is followed for that commit
- **THEN** the check reports the commit as not tested (no successful
  run with a green, fully-executed Full Tests job), and the human stops
  instead of tagging

