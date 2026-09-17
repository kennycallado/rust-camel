## ADDED Requirements

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
