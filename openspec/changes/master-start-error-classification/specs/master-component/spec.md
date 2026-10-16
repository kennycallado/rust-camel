## MODIFIED Requirements

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
existing retry classification, once per failed attempt;
`event` = `start_error` with `reason` = `transient` or `permanent`
once per delegate task that ends with an error while the consumer is not
stopping, matching the same classification. Stopping an
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

## ADDED Requirements

### Requirement: Delegate task failures are classified

When a delegate task ends with an error and neither the stop token nor the parent consumer context is cancelled, the Master component SHALL classify the error with the same classification it applies to delegate construction errors. A transient error SHALL leave the delegate `Inactive` and be retried by the acquisition branch under the configured reconnect policy and the acquisition budget. When the acquisition budget refuses that retry, the leadership task SHALL end through the graceful tail and return the last delegate error. A permanent error SHALL end the leadership task through the graceful tail, and the task SHALL return that error. The graceful tail SHALL run the delegate teardown and the lease `step_down` before the task returns, even when the teardown returns an error. For each such failed delegate task the collector SHALL record `master_delegate_lifecycle_total` with `event` = `start_error`. Errors that `reconcile_event` propagates from its own teardown of a stale or dead delegate are outside this requirement.

#### Scenario: transient delegate failure is retried

- **GIVEN** a delegate whose `start()` fails with a transient error once and then succeeds
- **WHEN** the leadership task observes the finished delegate task
- **THEN** the Master consumer keeps running, the delegate is created a second time, and the collector records `start_error` with reason `transient` once and `started` twice

#### Scenario: permanent delegate failure ends the task with the error

- **GIVEN** a delegate whose `start()` fails with a permanent error
- **WHEN** the leadership task observes the finished delegate task
- **THEN** the lease `step_down` completes, the task returns the delegate error, and the collector records `start_error` with reason `permanent` once

#### Scenario: exhausted budget returns the last transient error

- **GIVEN** `max_attempts = 1` and a delegate whose `start()` fails with a transient error
- **WHEN** the leadership task observes the finished delegate task
- **THEN** no second create attempt happens, the leadership task ends, `stop()` returns that transient error, and the collector records `start_error` with reason `transient` once

#### Scenario: a delegate failing during shutdown is not classified

- **GIVEN** a stop request and a delegate whose task ends with a transient error because of that stop
- **WHEN** the graceful tail runs
- **THEN** no `start_error` observation is recorded and no new delegate is created
