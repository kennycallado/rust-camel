## ADDED Requirements

### Requirement: One error per exhausted retry sequence

The system SHALL increment `camel_errors_total` exactly once for an
exhausted `NetworkRetryPolicy` retry sequence, and SHALL NOT increment it
per retry attempt.

#### Scenario: exhausted sequence counts once

- **WHEN** a policy-driven operation with `max_attempts = 5` fails all
  attempts
- **THEN** `camel_errors_total` for that operation increases by exactly 1
- **AND** `camel_retry_attempts_total{scheme,operation}` increases by 5

#### Scenario: call-site double counting removed

- **WHEN** a component error path wraps its failure in a retry helper
- **THEN** the component error path does not also call `increment_errors`
  for attempts the helper will retry

### Requirement: Per-attempt retry telemetry

The system SHALL expose a counter `camel_retry_attempts_total` with
labels `scheme` and `operation`, incremented once per retry attempt
(including the first), where `scheme` and `operation` values are closed
sets declared by each component.

#### Scenario: attempts visible separately from errors

- **WHEN** an operation retries twice then succeeds
- **THEN** `camel_retry_attempts_total{scheme,operation}` increases by 3
- **AND** `camel_errors_total` does not increase

### Requirement: Circuit-breaker rejections are not errors

The system SHALL count open-breaker fast-fails
(`CamelError::CircuitOpen`) on
`camel_circuit_breaker_rejections_total{route}` and SHALL NOT count them
on `camel_errors_total`.

#### Scenario: open breaker rejection

- **WHEN** an exchange fails fast because the route's circuit breaker is
  open
- **THEN** `camel_circuit_breaker_rejections_total{route}` increases by 1
- **AND** `camel_errors_total` does not increase

#### Scenario: error classification unchanged for callers

- **WHEN** a breaker is open
- **THEN** callers still receive `CamelError::CircuitOpen` classified
  `"circuit_open"` — only metric routing changes

### Requirement: Composite collector delegates error-semantics methods

`CompositeMetricsCollector` (and the MetricsHandle fan-out) SHALL
delegate `increment_retry_attempt` and
`increment_circuit_breaker_rejection` to every member, preserving the
multi-backend fan-out contract of metrics-collection-wiring.

#### Scenario: both backends observe attempts

- **WHEN** two collectors are registered and a retry attempt occurs
- **THEN** both collectors observe the attempt increment
