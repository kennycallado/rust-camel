# lifecycle-correctness Specification

## Purpose
TBD - created by archiving change immediate-start-errors. Update Purpose after archive.
## Requirements
### Requirement: Immediate consumer start failures transition the route to Failed

When a `ConsumerStartupMode::Immediate` consumer's `start()` returns an error
promptly (before the immediate grace budget elapses after the startup
invocation signal), a detached failure watcher SHALL observe the error latch
and transition the Route Lifecycle Projection to `Failed` through the
RuntimeBus, with the consumer task aborted and the consumer cancellation
token cancelled. The grace budget SHALL begin only at the startup invocation
signal (a latch the consumer task fires immediately before calling
`start()`) — never before it — so a scheduling delay between watcher spawn
and consumer task start cannot consume the budget. The lifecycle operation
that spawned the consumer
(`start_route`, `resume_route`, aggregate start) SHALL return `Ok` without
waiting for the outcome — preserving the fire-and-forget startup timing
Immediate consumers had before this change, so an Immediate consumer's
startup emission can never delay the controller actor or race a sibling
route's two-phase start commit.

#### Scenario: Fast-failing Immediate consumer fails loudly, asynchronously

- **GIVEN** a route whose consumer declares Immediate mode and whose
  `start()` returns `Err` promptly
- **WHEN** `start_route` is invoked
- **THEN** `start_route` returns `Ok` without grace delay
- **AND** the Route Lifecycle Projection eventually becomes `Failed`
  (within the grace budget plus a bounded retry)
- **AND** the spawned consumer task is aborted and the consumer
  cancellation token cancelled

#### Scenario: Fast-failing Immediate consumer on resume transitions to Failed

- **GIVEN** a suspended route whose recreated consumer declares Immediate
  mode and whose `start()` returns `Err` promptly
- **WHEN** `resume_route` is invoked
- **THEN** `resume_route` returns `Ok` without grace delay
- **AND** the route eventually becomes `Failed` and the failed resume
  leaves no detached consumer or child tasks

#### Scenario: Aggregate route with failing Immediate consumer

- **GIVEN** an aggregate-output route whose Immediate consumer's `start()`
  returns `Err` promptly
- **WHEN** the aggregate start is invoked
- **THEN** the operation returns promptly and the aggregate route
  eventually becomes `Failed`

#### Scenario: CamelContext::start does not fail fast on Immediate errors

- **GIVEN** a context with an auto-startup route whose Immediate consumer
  will fail promptly
- **WHEN** `CamelContext::start()` is invoked
- **THEN** it returns `Ok` (sibling routes still start) and the affected
  route eventually becomes `Failed`

#### Scenario: No startup reentrancy regression

- **GIVEN** an Immediate loop-style consumer (timer) that emits an exchange
  during `start()` driving a ControlBus stop against a sibling
  auto-startup route
- **WHEN** the context starts
- **THEN** the controller actor is never delayed by the Immediate
  handshake and the sibling's lifecycle commits without an invalid
  `Registered -> Stopped` transition
- **AND** the regression test synchronizes deterministically with a
  barrier over the sibling's start()-execution window (amended post-2.2,
  verified: the original AND-clause parked the emission inside the
  sibling's uncommitted `Starting` startup window — unsatisfiable in
  combination with a successful StopRoute, because the RuntimeBus
  pre-validation rejects Stop against a `Starting` aggregate before the
  command ever reaches the controller actor, route_runtime.rs:117-129 —
  so the barrier now pins the start()-execution window instead): the
  sibling is a loop-style IMMEDIATE consumer whose `start()` fires a
  `start_entered` test oneshot as its FIRST action and then parks on a
  `hold_rx` gate — its StartRoute COMMITS `Started` while `start()`
  stays parked; the emitting consumer's first emission is gated until
  the test observes that commit, and the emission's StopRoute dispatch
  is observed while the hold is still engaged — so the StopRoute
  provably lands inside the sibling's start()-execution window
  (post-commit) on every run, not by timing luck, and is honored into a
  final `Stopped`; the actor-never-delayed arm keeps the discriminating
  sub-grace timeout (the emitter's StartRoute must resolve within half
  the immediate grace, 25ms)

#### Scenario: Prompt Ok and loop-style consumers wait for nothing

- **GIVEN** any Immediate consumer (prompt Ok or loop-style lifetime)
- **WHEN** its lifecycle operation is invoked
- **THEN** the handshake adds no grace wait to the operation — the outcome
  latch path exists solely for the detached watcher

#### Scenario: Explicit consumers unchanged

- **GIVEN** a route whose consumer declares Explicit mode
- **WHEN** its `start()` fails before calling `mark_ready`
- **THEN** behavior is identical to before this change (bind failure
  propagated synchronously through the startup receiver; the controller
  actor still awaits the Explicit handshake)

### Requirement: Failure watcher sole execution with Phase 2a supersede tolerance

The failure watcher SHALL be the SOLE executor of the failed Immediate
consumer's `FailRoute` command (the consumer task's error path logs the
error and sends the CrashNotification but does not execute a RuntimeBus
command with this event's `command_id` — one id, one executor, so no
concurrent same-id execution can strand a dedup reservation). The watcher
SHALL bound itself to at most ONE defensive retry of the `FailRoute`
command after the grace boundary (same `command_id`, against transient
persistence errors; the RuntimeBus deduplicates successful commands, so a
duplicate landing after the route already reached `Failed` is a harmless
no-op). The startup-confirmation race is NOT the watcher's problem: the
lifecycle start handler SHALL absorb it — when the confirm-start persist
hits an optimistic-version conflict and the reloaded aggregate is in the
terminal `Failed` state, the handler SHALL treat the start as superseded
by the watcher and return `Ok` with the route durably `Failed`. On retry
exhaustion the watcher SHALL emit the existing system-broken error log
naming the route and terminate (the consumer task's synchronous `error!`
log is the guaranteed observability floor; its CrashNotification send is
best-effort — the watcher's bounded error-path wait can abort mid-send —
with no silent loss).

#### Scenario: Phase 2a recognizes the watcher's terminal-Failed supersede

- **GIVEN** a watcher whose `FailRoute` commits between the runtime side
  effect and the confirm-start persist (the aggregate transitions to
  terminal `Failed`, bumping the version)
- **WHEN** the confirm-start persist hits the optimistic-version conflict
- **THEN** the start handler re-loads the aggregate, recognizes terminal
  `Failed`, and returns `Ok` with the route durably `Failed` (the start
  was legitimately superseded — never a lost update)

#### Scenario: Phase 2a does not swallow non-Failed conflicts

- **GIVEN** a concurrent lifecycle command (e.g. Stop) causes the
  confirm-start persist to conflict with a reloaded state that is NOT
  terminal `Failed`
- **WHEN** the confirm-start persist fails
- **THEN** the start handler returns `Err` exactly as before this change
  (the supersede discriminator is terminal-Failed only — it cannot mask
  unrelated conflicts)

#### Scenario: Watcher terminates on bounded exhaustion

- **GIVEN** a route whose `FailRoute` command is rejected on the primary
  attempt and the single defensive retry
- **WHEN** the retry fails
- **THEN** the watcher emits the system-broken error log and terminates
- **AND** the consumer task's `error!` log (guaranteed, synchronous)
  remains the observability floor — no silent loss

#### Scenario: Duplicate FailRoute is a no-op

- **GIVEN** a route already transitioned to `Failed` by an earlier attempt
- **WHEN** the watcher's retry lands with the same `command_id`
- **THEN** the RuntimeBus deduplicates it and the route status is unchanged

