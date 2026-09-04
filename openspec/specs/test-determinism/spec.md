# test-determinism Specification

## Purpose
TBD - created by archiving change controlbus-flake-fix. Update Purpose after archive.
## Requirements
### Requirement: Wall-clock-independent controlbus stop test

The `controlbus_stops_route` integration test in camel-test SHALL drive its timer
routes with tokio paused time (`CamelTestContextBuilder::with_time_control()` +
`TimeController::advance`) instead of relying on real wall-clock deadlines, so its
pass/fail outcome is independent of CPU contention on the host.

#### Scenario: Stop action completes under virtual time

- **GIVEN** an auto-start timer route and a one-shot trigger route whose pipeline
  calls the ControlBus stop action on the auto route
- **WHEN** the test pumps virtual time in bounded increments (advance + yield
  bursts) and the trigger tick fires
- **THEN** `mock:stop-done` receives its exchange and the auto route's status
  reaches Stopped, with no real-time sleep or wall-clock deadline on the pass path

#### Scenario: Bounded failure instead of hang

- **GIVEN** the stop trigger pipeline never delivers its exchange (regressed
  pipeline)
- **WHEN** the bounded virtual-time pump exhausts its iteration budget
- **THEN** the test fails immediately with an explicit assertion message naming
  the virtual-time budget, rather than hanging on an unbounded endpoint poll; and
  teardown runs on real time because the test resumes the clock before stopping
  the harness

#### Scenario: Flake class eliminated under CPU saturation

- **GIVEN** the host CPU is deliberately saturated while the test runs
- **WHEN** the test executes repeatedly (20 consecutive runs)
- **THEN** all runs pass, because the test clock no longer advances in real time

### Requirement: Event-driven settling for seda integration tests

The seda integration tests in camel-test that assert an exact exchange count on
a bounded-repeat timer pipeline SHALL settle by awaiting exchange arrival
(`await_exchanges`) before stopping the harness, not by sleeping a fixed
wall-clock window, so their pass/fail outcome does not depend on CPU contention
from sibling tests.

#### Scenario: Exact count settles before teardown

- **GIVEN** a timer producer with `repeatCount=K` feeding a seda endpoint whose
  consumer writes to a mock endpoint
- **WHEN** the test awaits K exchanges (Notify-based, generous deadline) and
  then stops the harness
- **THEN** the exact exchange-count assertion passes with exactly K exchanges,
  regardless of how long delivery took within the deadline

#### Scenario: Deadline is a backstop, not a gate

- **GIVEN** sibling tests in the binary run in parallel and delay delivery
  beyond the old fixed sleep window
- **WHEN** delivery still completes within the generous deadline
- **THEN** the test passes — the flake mechanism (window shorter than actual
  delivery under load) is removed

#### Scenario: Full binary stable under repetition

- **GIVEN** the complete seda_test binary (all six tests) which previously
  failed ~4/8 full-binary runs on main
- **WHEN** the binary runs 10 consecutive times
- **THEN** all 10 runs are green

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

