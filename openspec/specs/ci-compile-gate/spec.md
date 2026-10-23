# ci-compile-gate Specification

## Purpose
CI coverage for the integration-test compile gate. The `full-tests-linux` job
compiles the gated test set behind the `integration-tests` feature, so a
compile break fails CI instead of surfacing only for a human locally.
## Requirements
### Requirement: CI compiles the gated integration test set

The `full-tests-linux` CI job SHALL include a named step, placed
before "Test (full workspace)", running
`cargo check -p camel-test --features integration-tests --tests`
(compile-only; no Docker or runtime required). The step SHALL carry a
comment referencing bd rc-8g35d (the compile-broken
`master_kubernetes_test` that stayed invisible to CI) and rc-khjnb.

#### Scenario: Gated test compile drift fails CI

- **GIVEN** a commit that breaks compilation of any test binary gated
  behind the `integration-tests` feature (for example
  `master_kubernetes_test`)
- **WHEN** the CI pipeline runs the compile-gate step
- **THEN** the step fails at the compile error instead of staying
  green until a human hits it locally

### Requirement: CI compiles the narrow cxf integration feature

The `full-tests-linux` CI job SHALL include a named, unconditional
step, placed adjacent to the wide integration-tests compile gate before
"Test (full workspace)", running
`cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run`
(compile and link only; no test execution, no Docker or bridge runtime
required). The step SHALL carry a comment referencing bd rc-z1ea2 and
stating that the `--all-features` clippy gate and the wide
`--features integration-tests` steps cannot detect defects confined to
the `integration-tests-cxf` feature line, because both activate
`dep:camel-component-cxf` as part of a wider feature union. The step
SHALL NOT be gated on paths, labels, `continue-on-error`, or any
condition that allows an otherwise-green PR to skip it.

#### Scenario: Healthy tree compiles the narrow selection

- **GIVEN** the repository tree with the `integration-tests-cxf`
  feature intact (`integration-tests-cxf = ["dep:camel-component-cxf"]`
  in `crates/camel-test/Cargo.toml`)
- **WHEN** the CI pipeline runs the narrow compile step on a PR
- **THEN** the `cxf_test` test binary compiles and links successfully
  under the narrow feature selection (the same selection the
  published-bridge smoke compiles) and the step exits 0 without
  executing any test

#### Scenario: Narrow feature dependency breakage fails the PR

- **GIVEN** a change that leaves the `cxf_test` target importing
  `camel_component_cxf` while the `integration-tests-cxf` feature no
  longer activates `dep:camel-component-cxf` (or otherwise breaks the
  narrow feature graph)
- **WHEN** the CI pipeline runs the narrow compile step
- **THEN** the step exits nonzero and the PR fails, instead of staying
  green until the post-release published-bridge smoke hits the same
  breakage (for the dependency-removal case specifically, the failure
  is a rustc unresolved-crate error on `camel_component_cxf`)

