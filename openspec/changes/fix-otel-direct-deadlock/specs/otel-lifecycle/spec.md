# otel-lifecycle Delta — fix-otel-direct-deadlock

## ADDED Requirements

### Requirement: TracingProcessor preserves the tower readiness contract

The `TracingProcessor` SHALL invoke the SAME inner processor instance it readied
in `poll_ready` when `call` executes. It SHALL NOT clone the inner processor and
re-drive readiness on the clone, because stateful producers hold reservations
acquired during `poll_ready` that only the readied instance can consume.

#### Scenario: direct InOut hop with tracing enabled completes and repeats

- **GIVEN** a route whose step is `to: "direct:echo"` with tracing enabled (via
  a process-local no-op or in-memory OTel provider — no OTLP network endpoint
  is required, the defect is exporter-independent) and a registered
  `direct:echo` consumer route
- **WHEN** an InOut exchange traverses the entry pipeline
- **THEN** the exchange SHALL complete with the consumer route's effects within
  the test timeout
- **AND** a SECOND exchange sent afterwards SHALL also complete within the test
  timeout (the permit-wedge failure mode would hang it)

#### Scenario: TracingProcessor instance is reusable across sequential cycles

- **GIVEN** a single `TracingProcessor` instance wrapping an inner processor
- **WHEN** `ready().await` then `call(exchange_a).await` completes, and the
  SAME instance is again driven `ready().await` then `call(exchange_b).await`
- **THEN** the second cycle SHALL also complete within the test timeout and
  return the inner's response for `exchange_b` (the wrapper must not become
  one-shot after the first call)

#### Scenario: inner service holding a poll-boundary permit is not re-readied on a clone

- **GIVEN** a mock inner service whose `poll_ready` acquires the sole permit of
  a shared `Semaphore::new(1)` into instance state and whose `Clone` shares the
  semaphore without the permit
- **WHEN** `TracingProcessor` wrapping that mock is driven `ready().await` then
  `call(exchange).await`
- **THEN** the call SHALL complete within the test timeout (no permanent
  `Pending` on the clone's acquire)
- **AND** the exchange result SHALL be the mock's response

#### Scenario: span lifecycle unchanged by the ownership restructure

- **GIVEN** a configured OTel in-memory provider and a `TracingProcessor`
  wrapping an inner processor
- **WHEN** an exchange completes successfully and a second one fails
- **THEN** each step SHALL produce exactly one span with status Ok / error
  respectively, as before the restructure
