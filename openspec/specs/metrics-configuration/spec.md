# metrics-configuration Specification

## Purpose
TBD - created by archiving change dashboard-observability. Update Purpose after archive.
## Requirements
### Requirement: Metrics enablement independent of tracing

The system SHALL provide an `[observability.metrics]` configuration table
whose `enabled` switch gates metric emission independently of
`[observability.tracer]` enabled; `tracer.enabled` gates spans ONLY.

#### Scenario: metrics on, tracer off

- **WHEN** `[observability.metrics] enabled = true` and
  `[observability.tracer] enabled = false` with prometheus on
- **THEN** metric families are exported and no spans are created

#### Scenario: metrics off, tracer on

- **WHEN** `[observability.metrics] enabled = false` and
  `[observability.tracer] enabled = true`
- **THEN** only the non-disableable families (errors) are exported and
  spans are created

### Requirement: Per-family opt-out with non-disableable errors

The system SHALL allow `[observability.metrics]` to disable the exchange
counter family, the duration histogram family, and component telemetry
independently; the ADR-0012 error family SHALL NOT be disableable by any
configuration.

#### Scenario: duration family disabled

- **WHEN** `[observability.metrics] duration = false`
- **THEN** `camel_exchange_duration_seconds` is not exported
- **AND** `camel_exchanges_total` and `camel_errors_total` still are

#### Scenario: error family cannot be disabled

- **WHEN** any combination of metrics levers is set
- **THEN** `camel_errors_total` remains exported

### Requirement: Component telemetry is opt-in

The system SHALL default `[observability.metrics] components` to false;
when false, the `camel_component_operations_total` family is not
exported; error-family emissions from components are not affected by
this lever.

#### Scenario: default excludes component family

- **WHEN** no `[observability.metrics]` table is present and prometheus
  is enabled
- **THEN** pipeline families and gauges are exported and
  `camel_component_operations_total` is not

### Requirement: Configuration schema documents metrics levers

The configuration schema (`schema.md`) SHALL document the
`[observability.metrics]` table, its keys, defaults, and the
non-disableable error family.

#### Scenario: schema validation

- **WHEN** a Camel.toml with `[observability.metrics]` is validated
- **THEN** validation passes against the documented schema and unknown
  keys are rejected

