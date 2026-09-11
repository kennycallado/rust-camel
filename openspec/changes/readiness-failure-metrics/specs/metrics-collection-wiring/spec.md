## MODIFIED Requirements

### Requirement: Metrics collector late binding via shared handle

The system SHALL route all metric emission through a single shared
`MetricsHandle` (an `ArcSwap`-backed cell seeded with `NoOpMetrics` at context
build) handed to the context slot, the route controller's
`tracer_metrics`, and the RuntimeBus collector, such that a collector
registered at ANY time — including after routes are added — is observed by
all subsequent emission calls.

#### Scenario: prometheus-only mode emits pipeline and component metrics

- **GIVEN** a Camel.toml with only `[observability.prometheus]` enabled and
  no `[observability.tracer]` table
- **WHEN** a route with a component error path runs and `/metrics` is scraped
- **THEN** disposition counters (exchanges, errors, duration) AND the
  component error metric are both non-empty

#### Scenario: late registration is observed

- **GIVEN** a context built with routes added and NO metrics service
- **WHEN** a lifecycle metrics service is registered after the routes exist
  and an exchange is processed
- **THEN** the newly registered collector observes the exchange emission

#### Scenario: controller path receives the live collector

- **GIVEN** a lifecycle metrics service registered on a built context
- **WHEN** a component resolves `RuntimeObservability` through
  `ControllerComponentContext`
- **THEN** the resolved collector is the registered collector, not `NoOp`

#### Scenario: readiness-phase producer failure is observable

- **GIVEN** a route whose target has no consumer and
  `failIfNoConsumers=true` (the default)
- **WHEN** one exchange is driven
- **THEN** the exchanges family counts the failure and the errors family is
  incremented, each exactly once per exchange, and the duration family
  remains call-time only
