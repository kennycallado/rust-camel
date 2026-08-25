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

