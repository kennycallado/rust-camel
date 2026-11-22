# quality-gates Specification

## Purpose
TBD - created by archiving change gates-workspace-suites. Update Purpose after archive.
## Requirements
### Requirement: Local sweep runs the unified-feature camel-test suites under workspace feature unification

The component-lane registry (`AGENTS.md ## COMPONENT LANES`) SHALL declare a
`camel-test` lane with mode `local` whose command runs every suite listed in
`CAMEL_TEST_WORKSPACE_SUITES` (default:
`dev_watch_script_test dev_watch_proto_test`, embedded in the row) as
`cargo test --workspace --test <suite>` — once per listed suite, aggregating
failures — so the suites that exercise unified features
(`camel-core/lang-rhai` enabled through `camel-cli` /
`camel-integration-test`) run under the same feature unification as CI's
workspace-wide test job. The row SHALL carry an in-row allowlist of reviewed
local suites (initially the same two names): an override value naming a
suite outside that allowlist — including Docker-classified suites — SHALL be
rejected before any cargo test dispatch, so the override can only subset or
reorder the reviewed local set. The lane SHALL be skippable through the
standard lane selectors (`GATES_SKIP=lanes`, per-crate `GATES_SKIP`,
`GATES_LANES`). A suite set reduced to empty by an explicitly empty
`CAMEL_TEST_WORKSPACE_SUITES` SHALL fail the lane with a message naming the
variable, without dispatching any cargo test invocation from that row (the
sweep's own `cargo metadata` probe and the registry's compile-only
`camel-test` row are unaffected). The local sweep SHALL NOT widen this lane
to full `cargo test --workspace`; Docker and native-bridge suites remain
CI-owned.

#### Scenario: Touched camel-test dispatches the default suite list

- **GIVEN** a change whose diff touches `camel-test`, and
  `CAMEL_TEST_WORKSPACE_SUITES` is unset
- **WHEN** the lane phase of `scripts/gates-local.sh` runs
- **THEN** the new row executes exactly `cargo test --workspace --test
  dev_watch_script_test` and `cargo test --workspace --test
  dev_watch_proto_test` (in row order), the registry's compile-only
  `camel-test` row still runs alongside it, and the sweep fails if any
  dispatched suite fails

#### Scenario: Explicitly empty suite list fails the lane fail-closed

- **GIVEN** `CAMEL_TEST_WORKSPACE_SUITES` is set to the empty string
- **WHEN** the lane phase runs the `camel-test` row
- **THEN** the row fails with a message naming
  `CAMEL_TEST_WORKSPACE_SUITES`, and that row dispatches no cargo test
  invocation

#### Scenario: Override naming a non-allowlisted suite is rejected

- **GIVEN** `CAMEL_TEST_WORKSPACE_SUITES` names a suite outside the row's
  in-row allowlist (for example `redis_state_test`, a Docker-classified
  suite whose `redis` feature starts a container)
- **WHEN** the lane phase runs the `camel-test` row
- **THEN** the row fails with a message naming `CAMEL_TEST_WORKSPACE_SUITES`
  and the allowlist, and that row dispatches no cargo test invocation — the
  Docker-classified suite never runs locally through this lane

#### Scenario: Declared row never trips the NO LANE fail-safe

- **GIVEN** a change that touches `camel-test` (a crate with an integration
  surface)
- **WHEN** the lane phase maps changed paths to crates
- **THEN** `camel-test` has a declared row, so the sweep does not report
  `NO LANE for camel-test` and the row is not blocking

#### Scenario: Lane stays skippable through the standard selectors

- **GIVEN** any change
- **WHEN** the sweep runs with `GATES_SKIP=lanes` (or `GATES_LANES=0`)
- **THEN** the whole lane phase — including this row — is disabled and the
  sweep reports it as skipped rather than running cargo

