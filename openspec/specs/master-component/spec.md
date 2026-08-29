# master-component Specification

## Purpose
TBD - created by archiving change master-metrics-wiring. Update Purpose after archive.
## Requirements
### Requirement: Master component emits leadership transition metrics for observed state edges

The master component SHALL emit the counter
`master_leadership_transitions_total` through the configured
`MetricsCollector` exactly once for each observed leadership-state
transition — `event` = `acquired` when the observed leading state
changes from not-leading to leading, `event` = `lost` when it changes
from leading to not-leading — with labels `lock` (the lock name) and
`route_id`. The initial leadership-watch snapshot establishes the
first state; a snapshot already leading SHALL be counted as one
`acquired` transition. Emission SHALL occur immediately before
`reconcile_event` processes the transitioning event. Repeated
identical watch deliveries and synthetic `StartedLeading`
re-dispatches produced by the delegate retry loop SHALL NOT emit.
Emission SHALL NOT be suppressed when subsequent delegate
construction fails.

#### Scenario: leadership acquired

- **GIVEN** a master consumer whose leadership watch snapshot or delivery changes the observed state to leading for lock `my-lock` on route `route-1`
- **WHEN** the supervision loop observes the not-leading → leading edge
- **THEN** the collector records `master_leadership_transitions_total` = 1 with labels `[("lock", "my-lock"), ("route_id", "route-1"), ("event", "acquired")]` before `reconcile_event` runs

#### Scenario: leadership lost

- **GIVEN** a master consumer that is leading whose leadership watch delivers a `StoppedLeading` event for lock `my-lock`
- **WHEN** the supervision loop observes the leading → not-leading edge
- **THEN** the collector records `master_leadership_transitions_total` = 1 with labels including `[("event", "lost")]` before `reconcile_event` runs

#### Scenario: repeated identical delivery does not re-emit

- **GIVEN** a master consumer that is not leading whose leadership watch delivers a `StoppedLeading` event
- **WHEN** the supervision loop observes the event with no leading-state change
- **THEN** no `master_leadership_transitions_total` observation is recorded

#### Scenario: synthetic retry re-dispatch does not emit

- **GIVEN** a master consumer that is leading whose delegate construction failed transiently, causing the retry tick to re-dispatch a synthetic `StartedLeading`
- **WHEN** the retry tick dispatches `reconcile_event` again
- **THEN** no additional `master_leadership_transitions_total` observation is recorded for the re-dispatch

#### Scenario: transition counted despite delegate construction failure

- **GIVEN** an observed not-leading → leading edge whose delegate endpoint construction returns a permanent error
- **WHEN** `reconcile_event` processes the event and propagates the error
- **THEN** the `acquired` transition was already recorded exactly once

### Requirement: Master component emits delegate lifecycle metrics with a uniform label schema

The master component SHALL emit the counter
`master_delegate_lifecycle_total` through the configured
`MetricsCollector` where every observation carries exactly the labels
`lock`, `route_id`, `event`, and `reason`. The component SHALL emit
`event` = `started` with `reason` = `none` when a delegate consumer is
spawned and the state becomes `Active`; `event` = `stopped` with
`reason` = `none` when teardown is attempted on an `Active`
delegate — the drain completing, or the drain timing out and the
delegate being aborted;
`event` = `create_error` with `reason` = `transient` or `permanent`
when delegate endpoint or consumer construction fails, matching the
existing retry classification, once per failed attempt. Stopping an
already `Inactive` delegate SHALL NOT emit.

#### Scenario: delegate started

- **GIVEN** a `StartedLeading` reconciliation whose delegate endpoint and consumer construct successfully
- **WHEN** the delegate consumer is spawned and the state transitions to `Active`
- **THEN** the collector records `master_delegate_lifecycle_total` = 1 with labels `[("lock", lock), ("route_id", route), ("event", "started"), ("reason", "none")]`

#### Scenario: delegate stopped after active

- **GIVEN** a master consumer in `DelegateState::Active`
- **WHEN** `stop_delegate` performs teardown on the active delegate (the drain completing, or the drain timing out and aborting the delegate)
- **THEN** the collector records `master_delegate_lifecycle_total` = 1 with labels including `[("event", "stopped"), ("reason", "none")]`

#### Scenario: inactive stop emits nothing

- **GIVEN** a master consumer in `DelegateState::Inactive`
- **WHEN** `stop_delegate` runs
- **THEN** no `master_delegate_lifecycle_total` observation is recorded

#### Scenario: endpoint construction fails transiently

- **GIVEN** a `StartedLeading` reconciliation whose delegate endpoint construction fails with a transient error
- **WHEN** the failure is classified by `is_retryable_camel_error` and swallowed for retry
- **THEN** the collector records `master_delegate_lifecycle_total` = 1 with labels including `[("event", "create_error"), ("reason", "transient")]`

#### Scenario: endpoint construction fails permanently

- **GIVEN** a `StartedLeading` reconciliation whose delegate endpoint construction fails with a permanent error
- **WHEN** the failure is classified and propagated for fail-fast
- **THEN** the collector records `master_delegate_lifecycle_total` = 1 with labels including `[("event", "create_error"), ("reason", "permanent")]`

#### Scenario: consumer construction fails transiently

- **GIVEN** a `StartedLeading` reconciliation whose endpoint constructs successfully but consumer construction fails with a transient error
- **WHEN** the failure is classified and swallowed for retry
- **THEN** the collector records `master_delegate_lifecycle_total` = 1 with labels including `[("event", "create_error"), ("reason", "transient")]`

#### Scenario: consumer construction fails permanently

- **GIVEN** a `StartedLeading` reconciliation whose endpoint constructs successfully but consumer construction fails with a permanent error
- **WHEN** the failure is classified and propagated for fail-fast
- **THEN** the collector records `master_delegate_lifecycle_total` = 1 with labels including `[("event", "create_error"), ("reason", "permanent")]`

#### Scenario: retry accumulation stays honest

- **GIVEN** one observed not-leading → leading transition followed by three failed construction attempts that the retry loop re-dispatches
- **WHEN** all attempts have been processed
- **THEN** exactly one `master_leadership_transitions_total` observation with `event` = `acquired` and exactly three `create_error` observations are recorded

### Requirement: Delegate reconciliation is epoch-idempotent

The Master component SHALL skip delegate recreation when a
`StartedLeading` delivery arrives while the delegate is Active and the
delegate's recorded leader epoch equals the currently published leader
epoch, and SHALL re-reconcile (drain, recreate, restamp the epoch bridge)
when the epochs differ, subject to the acquisition budget (see
"Delegate acquisition budget is exact" for the refused case). Stale-stamp
detection SHALL also be tick-driven: while leading with an Active
delegate whose stamp differs from the published epoch, the supervision
retry tick SHALL dispatch a reconciliation so the delegate is restamped
even when no watch delivery arrives.

#### Scenario: same-term duplicate delivery is a no-op

- **GIVEN** the delegate is Active at epoch E and the published leader
  epoch is still E
- **WHEN** a duplicate `StartedLeading` delivery is dispatched (watch
  re-delivery without a lease takeover)
- **THEN** the Active delegate is not stopped or recreated
  (`create_consumer_calls` stays 1) and no `stopped`+`started` lifecycle
  metric pair is emitted

#### Scenario: term bump while Active forces re-reconciliation

- **GIVEN** the delegate is Active at epoch E
- **WHEN** the published leader epoch becomes E' > E (coalesced flap across
  a takeover) and a `StartedLeading` delivery is dispatched
- **THEN** the delegate is drained and recreated exactly once and the new
  epoch-stamping bridge carries E'

#### Scenario: renewal-path epoch advance restamps on the next tick

- **GIVEN** the delegate is Active at epoch E and the published leader
  epoch advances to E' > E with no watch delivery (renewal-path clamp
  adoption of an out-of-band lease term)
- **WHEN** the supervision retry tick fires while the stamp is stale
- **THEN** a reconciliation is dispatched and the delegate is drained,
  recreated exactly once, and restamped at E'

#### Scenario: dead delegate with stale stamp resets instead of stopping

- **GIVEN** a bounded budget exhausted by the current acquisition epoch,
  an Active delegate at epoch E whose task has finished, and a published
  epoch E' > E with no watch delivery
- **WHEN** the supervision retry tick fires
- **THEN** the stale-stamp reconciliation runs before finished-handle
  teardown — the term bump resets the budget, the dead delegate is
  drained, and a new delegate is created stamped at E' (the consumer does
  not stop on the stale exhausted budget)

#### Scenario: real leadership edges still reconcile exactly once

- **GIVEN** the delegate is Inactive after leadership loss
- **WHEN** a genuine not-leading → leading edge is observed
- **THEN** the delegate is created exactly once regardless of the guard

### Requirement: Delegate acquisition budget is exact

Every delegate acquisition dispatch — the initial-snapshot reconciliation,
the observed-edge reconciliation, each retry-tick re-dispatch, and any
duplicate `StartedLeading` delivery that arrives while the delegate is
Inactive — SHALL count against the configured `max_attempts`, and within
the same acquisition epoch a consulted-exhausted budget SHALL perform no
create attempt. An acquisition epoch begins at an observed
not-leading → leading edge, at the initial snapshot, or at a term bump
detected by the epoch-idempotence guard — each resets the budget
unconditionally, while recreation after a reset remains subject to the
budget consult. An enabled bounded policy
SHALL perform at most N create attempts per acquisition epoch at
`max_attempts = N`, and exactly N under persistent transient failure
before further creates are refused; at `max_attempts = 0` (default)
retries SHALL remain unlimited; a disabled policy SHALL perform no create
attempt.

#### Scenario: transient failure at max_attempts = 1 attempts once

- **GIVEN** `max_attempts = 1` and a `StartedLeading` reconciliation whose
  delegate construction fails with a transient error
- **WHEN** the failure is classified and the budget is consulted
- **THEN** exactly one `create_error` observation with reason `transient`
  is recorded and the consumer stops with the budget-exhausted warning —
  no second attempt

#### Scenario: exhausted budget refuses a duplicate delivery

- **GIVEN** `max_attempts = 1`, one transient create failure already
  counted, and the delegate Inactive
- **WHEN** a duplicate `StartedLeading` watch delivery arrives before the
  retry tick stops the consumer
- **THEN** the delivery counts but performs no create attempt — no
  additional `create_error` observation is recorded

#### Scenario: term bump at exhausted budget re-acquires fresh

- **GIVEN** `max_attempts = 1` and a delegate that acquired successfully on
  its only attempt (budget exhausted) and runs Active at epoch E
- **WHEN** the published leader epoch becomes E' > E and the duplicate
  `StartedLeading` delivery reaches the guard
- **THEN** the term bump resets the budget, the delegate is drained and
  recreated, and the new bridge is stamped at E'

#### Scenario: persistent transient failure at max_attempts = 2 attempts exactly twice

- **GIVEN** `max_attempts = 2` and a `StartedLeading` reconciliation whose
  delegate construction keeps failing with a transient error
- **WHEN** the failure persists past the budget
- **THEN** exactly two `create_error` observations with reason
  `transient` are recorded and the consumer stops with the
  budget-exhausted warning

#### Scenario: disabled policy performs no create

- **GIVEN** a reconnect policy with `enabled = false`
- **WHEN** a not-leading → leading edge is observed
- **THEN** no create attempt is performed, no lifecycle `started`
  observation is recorded, and the consumer stops at the first retry tick

#### Scenario: unlimited default keeps retrying

- **GIVEN** `max_attempts = 0` (unlimited default) and a `StartedLeading`
  reconciliation whose delegate construction fails transiently
- **WHEN** retry ticks fire
- **THEN** re-dispatch continues without budget exhaustion

### Requirement: Master coordination documentation

The camel-master README SHALL document (i) that delegate drain runs after
leadership is already lost, so it can overlap a successor's lease, and
that the `x-camel-leader-epoch` fencing token enables split-brain safety
when a sink opts into rejecting stale epochs — enforcement is opt-in per
sink (ADR-0035) — rather than drain-before-takeover ordering providing
it; and (ii) how two `master:X:` routes in one process coordinate, per
backend: on Kubernetes they share one cached elector per lock name —
mutual exclusion is per-process, so when this process leads, both routes'
delegates run and only one process holds the lease; the default Noop
platform reports every route as leader and provides no cross-route or
cross-process exclusion. The documented
`delegate_retry_max_attempts` default SHALL be `0` (unlimited).

#### Scenario: operator reads the drain-and-fencing docs

- **GIVEN** the camel-master README
- **WHEN** an operator reads the "How It Works" section
- **THEN** it states the drain/lease overlap possibility, states that the
  epoch token enables split-brain safety when a sink opts into rejecting
  stale epochs (ADR-0035), and the options table shows
  `delegate_retry_max_attempts` default `0` = unlimited

#### Scenario: operator reads the lock-sharing docs

- **GIVEN** the camel-master README
- **WHEN** an operator runs two `master:X:` routes in one process
- **THEN** the README has told them that on Kubernetes the routes share
  one cached elector (exclusion is per-process; both delegates run while
  this process leads; one process holds the lease) and that the default
  Noop platform reports every route as leader with no exclusion

