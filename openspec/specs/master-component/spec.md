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

