# component-metrics-emission Specification

## Purpose
TBD - created by archiving change dashboard-observability. Update Purpose after archive.
## Requirements
### Requirement: Route state gauge

The system SHALL export `camel_route_state{route,state}` maintained from
`RouteStatusProjection` transitions, such that a scrape reflects the
current state of every route.

#### Scenario: route starts

- **WHEN** a route reaches its startup-complete state (`Started` in the
  `RouteStatusProjection` closed set)
- **THEN** `camel_route_state{route,state="Started"}` equals 1 for that
  route and its previous state gauge equals 0

### Requirement: Build and uptime info

The system SHALL export `camel_build_info{version,git_sha}` (value 1)
and `camel_uptime_seconds` on every scrape under prometheus.

#### Scenario: fresh scrape after restart

- **WHEN** the application restarts and is scraped
- **THEN** `camel_build_info` identifies the new build and
  `camel_uptime_seconds` is near zero, distinguishing restart-reset
  counters from idle health

### Requirement: Queue depth visible for buffered stages

The system SHALL call `MetricsCollector::set_queue_depth` from SEDA
consumers, the aggregator, and the resequencer, exporting
`camel_queue_depth{queue}` that is non-zero under load and drains toward
zero when idle.

#### Scenario: SEDA backlog under load

- **WHEN** a SEDA endpoint receives faster than its consumer drains
- **THEN** `camel_queue_depth` for that queue is positive during the
  backlog and reaches zero after draining

### Requirement: Uniform component operations family

Components SHALL emit
`camel_component_operations_total{component,operation,outcome}` with
outcome ∈ {success, failure} for their principal operations, gated by
the `[observability.metrics].components` lever; components audited as
holding-but-not-emitting (wasm, opensearch, seda, surrealdb remainder,
cxf remainder) SHALL emit this family for errors at minimum.

#### Scenario: healthy route observable

- **WHEN** a route using a swept component processes exchanges
  successfully with the components lever on
- **THEN** `camel_component_operations_total{...,outcome="success"}`
  increases for the component's principal operation

#### Scenario: dead-observability component now emits errors

- **WHEN** a swept component's operation fails with the components lever
  off
- **THEN** the failure is observable on the component's error-family
  emission despite the lever being off

#### Scenario: registry context honors the components lever

- **GIVEN** a `RegistryComponentContext` constructed with the lever
  snapshot on and a wired collector
- **WHEN** `component_metrics_enabled()` is queried and the facade is
  built through `component_metrics()`
- **THEN** the lever reports on and the facade emits the
  component-operations family through the wired collector
- **AND** constructed with the lever off, the family is suppressed
  while the error family still flows

### Requirement: Label values are closed sets

Metric label values emitted through `record_counter` and
`record_histogram` SHALL be string literals, values derived from
`OptionKind::Enum` metadata (ADR-0041), or explicitly allowlisted with a
bd reference; an xtask lint SHALL enforce this workspace-wide.

#### Scenario: lint rejects raw label value

- **WHEN** a label value is a raw runtime string (path, status,
  hostname) without an allow annotation
- **THEN** `cargo xtask lint-metric-labels` fails citing the site

#### Scenario: literal passes

- **WHEN** a label value is a string literal
- **THEN** the lint passes

#### Scenario: enforcement scope

- **WHEN** the lint walks the workspace
- **THEN** it enforces `record_counter`, `record_histogram`,
  `record_component_operation`, and `increment_retry_attempt` label
  positions; the `increment_errors` route-label position stays outside
  enforcement (route ids are user-defined by design — follow-up work
  audits its bounded sets separately)

