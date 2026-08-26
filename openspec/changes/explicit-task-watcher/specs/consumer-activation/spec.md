## ADDED Requirements

### Requirement: Outer Explicit-consumer task termination is observed

camel-core SHALL observe unaccounted termination (panic or abort that no
task-body path accounted for) of the outer Explicit consumer task across its
whole lifetime, and publish the route failure through the standard channels
(crash notification + runtime failure publication → FailRoute), so a
consumer task that dies between `mark_ready()` and normal completion cannot
leave the route confirmed Started with a dead handle.

#### Scenario: Panic between readiness and completion fails the route

- **GIVEN** an Explicit consumer whose `start()` calls `mark_ready()` and
  then panics before returning
- **WHEN** the outer task terminates abnormally while the handshake already
  resolved Ok
- **THEN** the outer-task watcher publishes the failure (FailRoute) and the
  route reaches Failed — no Started-with-dead-handle state

#### Scenario: Panic in the finally-stop also fails the route

- **GIVEN** an Explicit consumer that started Ok and readied, whose
  `stop()` (the task body's final action) panics
- **WHEN** the outer task terminates abnormally after the controller
  installed the handle
- **THEN** the same failure publication occurs — the watch covers the whole
  task lifetime, not only the readiness window

#### Scenario: Normal completion is silent

- **GIVEN** an Explicit consumer that completes its task body normally
  (with or without a background task handle)
- **WHEN** the outer task finishes and sets the Accounted outcome state
- **THEN** the watcher exits without publishing any failure — no false
  positive, and no duplicate publication racing the background-task monitor

#### Scenario: Cancelled termination is silent

- **GIVEN** a route being stopped, whose consumer cancel token fired before
  the outer task terminated
- **WHEN** the outer task is aborted or exits during the stop flow
- **THEN** the watcher exits without publishing — stop-owned terminations
  are not failures

#### Scenario: Background-monitor failure is published exactly once

- **GIVEN** an Explicit consumer with a background task handle whose
  background task fails and the outer body then completes normally
- **WHEN** the background monitor publishes and the finally-stop then
  panics (the outer task terminates with the outcome already Accounted)
- **THEN** exactly one FailRoute is published — the outer watcher stays
  silent because the body accounted the failure before the fallible
  cleanup (sole-publisher discipline preserved)
