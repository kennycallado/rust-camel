## ADDED Requirements

### Requirement: Bounded per-test execution in the Rust library-test job

The Unit Tests (ubuntu-latest) CI job SHALL execute the container-free
Rust library-test scope (`--workspace --lib`) under cargo-nextest 0.9.143
(pinned in CI) with a checked-in `ci`
profile that terminates any single test at an effective ceiling of
approximately 90 seconds (slow-timeout period 30s x terminate-after 3),
retries each failing test exactly once for diagnostics, and treats a
retry-passing test as a FAILED run (`flaky-result = "fail"`).

#### Scenario: Hung test terminates at the ceiling

- **GIVEN** a unit test that never completes (unbounded wait)
- **WHEN** the pilot job runs under the ci profile
- **THEN** each timed-out attempt is terminated at approximately 90
  seconds with an explicit slow-timeout report, remaining selected tests
  continue because `fail-fast = false`, and the final job conclusion is
  failure

#### Scenario: Retry-pass fails the gating job

- **GIVEN** a test that fails on first attempt and passes on retry
- **WHEN** the pilot job runs with retries = 1 and flaky-result = "fail"
- **THEN** the run is reported FLAKY and the job conclusion is failure,
  so a flaky test cannot merge silently

#### Scenario: Container suites stay on cargo test

- **GIVEN** camel-test integration binaries that share containers through
  process-local fixtures
- **WHEN** the pilot lands
- **THEN** the testcontainers, K3s, and bridge suites in the Full Tests
  job keep their existing `cargo test` invocations unchanged

#### Scenario: Selection parity with the cargo baseline

- **GIVEN** the pilot's first run on main
- **WHEN** the nextest-reported selected test count is compared with the
  cargo `--workspace --lib` baseline
- **THEN** the counts match exactly, and any divergence blocks the
  rollout decision recorded in bd rc-mhsn
