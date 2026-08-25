## ADDED Requirements

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
