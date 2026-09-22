## ADDED Requirements

### Requirement: job send-phase retry classification

The `camel job` send loop SHALL retry only `EndpointCreationFailed` errors
that are NOT the SEDA no-active-consumers gate. The SEDA gate wordings
("has no active consumers" single mode, "has no active subscribers" fanout
mode) SHALL be non-retryable — the rejection fires pre-enqueue but inside
the caller's pipeline, so a retry would re-execute already-run route steps
and duplicate their side effects. Gate errors SHALL return as pipeline
failures on the first attempt, without sleeping or replaying the exchange.
Every non-gate `EndpointCreationFailed` error SHALL remain retryable for the
existing bounded window — the direct registration race, and (as a
documented residual retained by bd rc-ucemm's scope) SEDA queue-full and
bounded enqueue/fanout timeout errors that share the variant.

#### Scenario: SEDA single-mode gate fails fast

- **GIVEN** a started job context whose entry route sends to a SEDA
  endpoint with no active consumer
- **WHEN** the send loop receives the pipeline error
  `EndpointCreationFailed("SEDA endpoint '...' has no active consumers")`
- **THEN** the classification reports non-retryable and the send returns
  `SendError::Pipeline` on the first attempt

#### Scenario: SEDA fanout-mode gate fails fast

- **GIVEN** a started job context whose entry route sends to a fanout SEDA
  endpoint with no active subscribers
- **WHEN** the send loop receives the pipeline error
  `EndpointCreationFailed("SEDA endpoint '...' has no active subscribers")`
- **THEN** the classification reports non-retryable and the send returns
  `SendError::Pipeline` on the first attempt

#### Scenario: direct registration race stays retryable

- **GIVEN** a job send whose entry endpoint races consumer registration
- **WHEN** the send loop receives
  `EndpointCreationFailed("direct endpoint '...' not registered")`
- **THEN** the classification reports retryable and the loop keeps retrying
  within the bounded window

#### Scenario: generic endpoint-creation failure stays retryable

- **GIVEN** a job send that fails with any other `EndpointCreationFailed`
- **WHEN** the classification runs
- **THEN** the error is retryable (the pre-existing typed-match breadth is
  unchanged except for the SEDA gate exclusion)

#### Scenario: SEDA queue-full stays retryable

- **GIVEN** a job send whose route enqueues to a SEDA endpoint whose bounded
  queue is full
- **WHEN** the send loop receives
  `EndpointCreationFailed("SEDA queue '...' is full (size=...)")`
- **THEN** the classification reports retryable — queue-full shares the
  variant and is NOT one of the excluded gate wordings (documented residual,
  out of this change's scope)

#### Scenario: discard-if-no-consumers never reaches the classifier

- **GIVEN** a SEDA producer configured with `discardIfNoConsumers=true` and
  no active consumer
- **WHEN** the send executes
- **THEN** the produce succeeds (the exchange is discarded) with no
  `EndpointCreationFailed` at all — the retry classifier is not consulted
  and this change does not alter the discard path

#### Scenario: pre-SEDA side effect executes exactly once

- **GIVEN** a job document sending to an entry route whose steps have a
  side effect before a `to(seda:...)` step, and the SEDA endpoint has no
  active consumer
- **WHEN** the job send completes with the gate failure
- **THEN** the pre-SEDA side effect executed exactly once — no retry
  replayed the route pipeline — and the job reports outcome `Failed`
  with the gate error
