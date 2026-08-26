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

