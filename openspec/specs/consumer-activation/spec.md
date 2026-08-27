# consumer-activation Specification

## Purpose
TBD - created by archiving change cohort-activation-barrier. Update Purpose after archive.
## Requirements
### Requirement: Cohort activation barrier for first dispatch

camel-core SHALL park the first pipeline dispatch of every consumer-envelope
drain task until the context's startup cohort has completed, so that exchanges
emitted during context boot (after consumer readiness, while later siblings
are still Registered or Starting) cannot execute pipelines or drive
control-plane commands against mid-start siblings.

#### Scenario: Post-readiness emission parks until cohort completes

- **GIVEN** a booting context with routes A (consumer emits immediately after
  readiness) and B (startup in progress, mid-Starting)
- **WHEN** A's drain task receives the envelope while the sequential StartRoute
  loop has not finished
- **THEN** the envelope is held undispatched (received but not executed) until
  the cohort loop completes, and emission-side semantics (channel
  backpressure, ordering) are unchanged

#### Scenario: Registered sibling stop no longer rejected

- **GIVEN** route A (Immediate consumer, deterministic first-tick emission)
  whose first exchange carries a controlbus stop for route B, and B was
  Registered when A's consumer emitted
- **WHEN** the cohort completes and A's parked exchange finally dispatches
- **THEN** the StopRoute command executes against a settled route table — no
  "invalid transition: Registered -> Stopped" rejection

#### Scenario: Barrier opens on every post-reset boot return

- **GIVEN** a context boot that can fail at any stage after the entry reset
  (service startup, validation, reconciliation, route-id listing, or the
  StartRoute loop with some routes already Started — today's semantics keep
  them running)
- **WHEN** the boot returns, successfully or with an error
- **THEN** the barrier has been activated on every such return path (single
  capture-result funnel: always activate, then return the original result),
  so parked envelopes of already-Started routes dispatch normally and no
  stale drain can strand — boot-failure behavior is not made worse than today

#### Scenario: Cancellation beats barrier wait

- **GIVEN** a drain task holding a parked envelope whose route is stopped
  while the cohort is still incomplete
- **WHEN** the pipeline cancel token fires
- **THEN** the drain task exits promptly instead of waiting for cohort
  activation, and a parked InOut reply waiter resolves to the channel-closed
  error

#### Scenario: Barrier covers every consumer-envelope drain topology

- **GIVEN** routes using the Concurrent consumer model, the Sequential model,
  and the restart-path aggregate drain
- **WHEN** their drain tasks receive consumer envelopes during boot
- **THEN** all three envelope dispatch sites park on the same barrier; the
  aggregator late-exchange branch (fed only post-dispatch) is not gated and
  no topology bypasses activation

#### Scenario: Per-boot re-arm with open-at-steady-state

- **GIVEN** a context that booted successfully (barrier open), then fully
  stops and boots again via start_context
- **WHEN** the second boot begins (auto-startup routes re-issued StartRoute)
  and completes
- **THEN** the barrier re-closed at boot entry governs the new cohort and
  opens after its loop; a single-route hot-reload or controller-level start
  between full boots sees the open barrier and dispatches without added
  latency

#### Scenario: Opened barrier is level-triggered for late waiters

- **GIVEN** the barrier already open
- **WHEN** any drain task newly awaits activation
- **THEN** the await resolves immediately (watch level-triggered semantics) —
  no missed-notification window exists

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

### Requirement: Bare-controller activation of the cohort barrier

camel-core SHALL expose a public, idempotent activation method for
the cohort barrier on `DefaultRouteController`, performing the same
gate-opening act the CamelContext lifecycle performs through the
controller actor handle, so that consumers driving a bare
`DefaultRouteController` (registered, started, and dispatched
outside a full context lifecycle) can release parked pipeline
dispatch after their startup completes. The method SHALL NOT change
barrier semantics for the context path: reset stays boot-scoped,
activation stays level-triggered, and the method SHALL NOT
re-close or re-arm the gate.

#### Scenario: Bare-controller dispatch releases after activation

- **GIVEN** a bare `DefaultRouteController` with a route added and started, and no CamelContext lifecycle involved
- **WHEN** a consumer sends an exchange and the controller's activation method has been called after startup
- **THEN** the parked dispatch proceeds and the sender's reply resolves within normal call timeouts, instead of parking until the drain timeout

#### Scenario: Unactivated bare controller parks dispatch

- **GIVEN** a bare `DefaultRouteController` with a route added and started, and the activation method never called
- **WHEN** a consumer sends an exchange
- **THEN** pipeline dispatch stays parked (barrier contract unchanged) — activation is the bare consumer's explicit responsibility

#### Scenario: Activation is idempotent and level-triggered

- **GIVEN** a bare controller whose barrier was already activated
- **WHEN** the activation method is called again
- **THEN** the call returns without effect (no re-arm, no reset) and any newly parked dispatch resolves immediately

#### Scenario: Context path unchanged

- **GIVEN** a CamelContext boot with its startup cohort completing normally
- **WHEN** the context lifecycle activates the barrier through the actor handle
- **THEN** the barrier opens, parked dispatch proceeds, and any additional activation call has no effect and requires no ordering relative to the context's act

