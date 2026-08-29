# metrics-collection-wiring Specification

## Purpose
TBD - created by archiving change metrics-handle-late-binding. Update Purpose after archive.
## Requirements
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

### Requirement: Second collector registration composes

The system SHALL, when a second metrics service registers, compose a
`CompositeMetricsCollector` over the previously stored collector and the new
one, such that BOTH collectors receive every subsequent emission. Composition
MUST NOT replace the first collector.

#### Scenario: otel and prometheus both registered

- **GIVEN** a context where an otel-backed service registers first and a
  prometheus service registers second
- **WHEN** an exchange completes with an error
- **THEN** both collectors observe the exchange duration and the error
  increment

### Requirement: Tracer pipeline implied by prometheus enablement

The system SHALL enable the tracer pipeline when
`[observability.prometheus]` is enabled, so that the non-disableable
error family is always exported; the new `[observability.metrics]`
levers (metrics-configuration capability) may suppress individual
non-error families but SHALL NOT disable the pipeline itself.

#### Scenario: prometheus-only config enables the pipeline

- **GIVEN** a config with prometheus enabled and no tracing table
- **WHEN** `effective_tracer_config` resolves
- **THEN** the resolved tracer config has `enabled = true`

#### Scenario: explicit tracing disable wins

- **GIVEN** a config with prometheus enabled and
  `[observability.tracer] enabled = false`
- **WHEN** `effective_tracer_config` resolves
- **THEN** the resolved tracer config has `enabled = false`
- **AND** spans are suppressed while the pipeline stays enabled
  (`pipeline_enabled = true`) so metric families — errors
  unconditionally — keep flowing

#### Scenario: prometheus-only still runs the pipeline with metrics opt-outs

- **GIVEN** no metrics family opt-outs are set
- **WHEN** only `[observability.prometheus]` is enabled
- **THEN** the tracer pipeline is enabled and component metrics reach
  `GET /metrics`
- **AND** `camel_errors_total` is exported regardless of any
  `[observability.metrics]` lever combination

### Requirement: Snapshot injection path removed

The system SHALL NOT carry a `metrics_collector` field on `TracerConfig`;
collector injection MUST occur exclusively through the lifecycle/registration
path or the builder `.metrics()` setter, both of which store into the shared
handle.

#### Scenario: config carries no collector

- **GIVEN** any CamelConfig instance
- **WHEN** the tracer config is constructed
- **THEN** it contains no collector reference and its Debug output requires
  no redaction

