# ci-clippy-gate Specification

## Purpose
TBD - created by archiving change ci-clippy-parity. Update Purpose after archive.
## Requirements
### Requirement: ci.yml clippy runs the registry command set

The `quality` job's "clippy" step in `.github/workflows/ci.yml` SHALL run
exactly the clippy command set declared by the `clippy` gate in
`AGENTS.md ## QUALITY GATES`: the workspace invocation with
`--all-features`, the four excludes (`camel-cli`,
`camel-component-kafka`, `security-keycloak`, `security-wasm-policy`),
`--all-targets`, and `-D warnings`, followed by the three single-crate
invocations (`camel-component-kafka` with `--all-targets`; `camel-cli`
with defaults; `camel-cli` with `--no-default-features
--features flavor-regular,exec --all-targets`). The step SHALL carry a
comment naming `AGENTS.md ## QUALITY GATES` as the source of truth and
referencing bd rc-2s1vm, instructing that both files change in the same
commit. No other workflow behaviour (triggers, timeouts, matrix,
permissions, other steps) SHALL change.

#### Scenario: Test-only lint reaches CI

- **GIVEN** a change whose only clippy finding fires in `#[cfg(test)]`
  code or a `tests/` target (for example `clippy::len_zero` in a new
  test, as in bd rc-e6kz2)
- **WHEN** the CI `quality` job runs the clippy step
- **THEN** the step fails the PR with that lint instead of staying
  green while only the local sweep catches it

#### Scenario: Registry and workflow disagree

- **GIVEN** a change that alters a clippy flag or exclude in the
  `AGENTS.md ## QUALITY GATES` registry without updating the `ci.yml`
  step to the same command (or the reverse)
- **WHEN** `cargo xtask lint-gate-parity` runs in the local sweep
- **THEN** it exits 1 naming the `clippy` gate with the paste-ready
  expected and actual commands, and the change is rejected before merge

