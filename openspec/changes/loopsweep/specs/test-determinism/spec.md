## ADDED Requirements

### Requirement: Deadline-bounded await loops in test bodies

Every `loop` in a test function body that carries an `.await` SHALL be
bounded by a deadline: either an enclosing `tokio::time::timeout` over the
whole wait, or an attempt-bounded retry wrapped in an overall timeout —
both failing loudly when the awaited progress never arrives. A
per-iteration timeout whose expiry silently exits the loop SHALL be used
only for true quiescence drains, where expiry is the normal end of the
wait; a loop that waits for a completion marker or other externally
driven progress SHALL NOT use the silent per-iteration form — its expiry
means the progress never arrived, which is a failure. A loop that cannot
carry a deadline without killing the behavior under test (a spawned
long-lived service loop per ADR-0069 §13.2 R1) SHALL be marked
`// allow-test-wait: <reason>` with a non-empty justification. The
`lint-unbounded-wait` ratchet ceiling SHALL decrease when loop sites are
converted and SHALL never increase in this change.

#### Scenario: readiness loop fails loudly instead of hanging

- **GIVEN** a readiness-polling loop that awaits externally driven
  progress (e.g. Redis failover recovery) and the progress never arrives
- **WHEN** the test runs
- **THEN** it fails within the enclosing timeout budget with a message
  naming what was awaited, rather than parking forever on the inner await

#### Scenario: converted sites leave the unadjudicated count

- **GIVEN** the seed inventory at 9ad4290f with 37 loop-class sites and
  a ratchet ceiling of 548
- **WHEN** all conversions and justified markers are applied and
  `xtask lint-unbounded-wait` runs
- **THEN** each of the 37 sites is no longer counted (converted or
  suppressed with its recorded reason) and the ratchet ceiling is
  lowered to 511

#### Scenario: escape marker requires justification

- **GIVEN** a spawned long-lived service loop where an internal deadline
  would terminate the service under test
- **WHEN** the site is marked `// allow-test-wait: <reason>`
- **THEN** the reason is non-empty, references the ADR-0069 R1
  service-loop exception, and the owning test bounds its readiness
  assertions and teardown

#### Scenario: no behavior drift in passing tests

- **GIVEN** a test containing a converted loop that previously passed
- **WHEN** the bounded version runs and readiness arrives normally
- **THEN** the test still passes, with the loop exiting through its
  original success path
