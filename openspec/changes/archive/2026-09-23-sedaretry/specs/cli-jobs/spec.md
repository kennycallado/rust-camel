## MODIFIED Requirements

### Requirement: job send-phase retry classification

The `camel job` send loop SHALL retry only endpoint-creation-family
errors that are NOT the SEDA no-active-consumers gate and NOT a SEDA
terminal-config error (an `EndpointCreationFailedWithSource` carrying
the seda crate's typed terminal-config marker, classified by
`camel_component_seda::is_seda_terminal_config_error`). Genuine SEDA
gate rejections arrive as source-preserving endpoint-creation failures
whose source chain carries the crate's private gate marker — their
wordings ("has no active consumers" single mode, "has no active
subscribers" fanout mode) is owned by the seda crate, and the
classification is typed, never text-based. The gate and the
deterministic multipleConsumers+wait configuration conflict SHALL be
non-retryable — the gate rejection fires pre-enqueue but inside the
caller's pipeline, so a retry would re-execute already-run route steps
and duplicate their side effects, and the configuration conflict can
never succeed on any retry. Both error classes SHALL return as pipeline
failures (`SendError::Pipeline`) on the first attempt, without sleeping
or replaying the exchange. Every other endpoint-creation-family error —
the plain `EndpointCreationFailed` variant and source-preserving
`EndpointCreationFailedWithSource` failures that carry neither SEDA
marker — SHALL remain retryable for the existing bounded window: the
direct registration race, foreign source-preserving endpoint failures,
and (as a documented residual retained by bd rc-ucemm's scope) SEDA
queue-full and bounded enqueue/fanout timeout errors that share the
variant.

#### Scenario: SEDA single-mode gate fails fast

- **GIVEN** a started job context whose entry route sends to a SEDA
  endpoint with no active consumer
- **WHEN** the send loop receives the genuine gate rejection (a
  source-preserving endpoint-creation failure carrying the seda crate's
  gate marker, rendering `SEDA endpoint '...' has no active consumers`)
- **THEN** the classification reports non-retryable and the send returns
  `SendError::Pipeline` on the first attempt

#### Scenario: SEDA fanout-mode gate fails fast

- **GIVEN** a started job context whose entry route sends to a fanout SEDA
  endpoint with no active subscribers
- **WHEN** the send loop receives the genuine gate rejection (rendering
  `SEDA endpoint '...' has no active subscribers`)
- **THEN** the classification reports non-retryable and the send returns
  `SendError::Pipeline` on the first attempt

#### Scenario: SEDA terminal-config error fails fast

- **GIVEN** a started job context whose send target is
  `seda:<name>?multipleConsumers=true` with an active consumer on the
  endpoint, so the send's forced `waitForTaskToComplete=Always` triggers
  the deterministic configuration conflict
- **WHEN** the send loop receives the pipeline error (a
  source-preserving endpoint-creation failure carrying the seda crate's
  terminal-config marker)
- **THEN** the classification reports non-retryable, the send returns
  `SendError::Pipeline` on the first attempt without sleeping (the
  in-process send completes in under 1 second — a retry burn would
  consume the full 3 second window), and the error carries the terminal-
  config marker per `is_seda_terminal_config_error`

#### Scenario: terminal-config job exits Failed without retry burn

- **GIVEN** a job document sending to `seda:<name>?multipleConsumers=true`
  whose consumer route is started
- **WHEN** `camel job` runs the document as a subprocess
- **THEN** the process exits with code 1, the JSON report carries outcome
  `Failed` with the pinned multipleConsumers+wait error text
  (`Endpoint creation failed: multipleConsumers=true with
  waitForTaskToComplete != Never is not supported`), and the total
  process elapsed time stays below 1.5 seconds — half the 3 second send
  retry window, proving no retry burn

#### Scenario: direct registration race stays retryable

- **GIVEN** a job send whose entry endpoint races consumer registration
- **WHEN** the send loop receives
  `EndpointCreationFailed("direct endpoint '...' not registered")`
- **THEN** the classification reports retryable and the loop keeps retrying
  within the bounded window

#### Scenario: generic endpoint-creation failure stays retryable

- **GIVEN** a job send that fails with any other endpoint-creation-family
  error — a plain `EndpointCreationFailed`, or a source-preserving
  `EndpointCreationFailedWithSource` carrying neither SEDA marker
- **WHEN** the classification runs
- **THEN** the error is retryable (the pre-existing typed-match breadth is
  unchanged except for the SEDA gate and terminal-config exclusions)

#### Scenario: SEDA queue-full stays retryable

- **GIVEN** a job send whose route enqueues to a SEDA endpoint whose bounded
  queue is full
- **WHEN** the send loop receives
  `EndpointCreationFailed("SEDA queue '...' is full (size=...)")`
- **THEN** the classification reports retryable — queue-full shares the
  variant and is NOT one of the excluded classes (documented residual,
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
