# metrics-configuration Specification

## Purpose
Configuration of metric emission. `[observability.metrics]` gates metrics
independently of tracing, allows per-family opt-out, defaults component
telemetry off, and keeps the error family non-disableable.
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
exported unless a per-component override enables that component;
error-family emissions from components are not affected by this lever.
The effective switch per component is
`enabled && component_overrides.get(component).unwrap_or(components)`.

#### Scenario: default excludes component family

- **WHEN** no `[observability.metrics]` table is present and prometheus
  is enabled
- **THEN** pipeline families and gauges are exported and
  `camel_component_operations_total` is not

#### Scenario: default still excludes non-overridden components

- **WHEN** only `component_overrides.kafka = true` is configured with
  `components = false`
- **THEN** kafka component operations are exported and any component
  without an override is not

### Requirement: Configuration schema documents metrics levers

The configuration schema (`schema.md`) SHALL document the
`[observability.metrics]` table, its keys, defaults, the
`component_overrides` map, its precedence rule, the profile and include
interaction, and the non-disableable error family.

#### Scenario: schema validation

- **WHEN** a Camel.toml with `[observability.metrics]` is validated
- **THEN** validation passes against the documented schema and unknown
  keys are rejected

#### Scenario: overrides documented

- **WHEN** a reader opens the metrics section of `schema.md`
- **THEN** the table lists `component_overrides` with its default,
  precedence formula, merge semantics, and validation rule

### Requirement: Per-component metric overrides

The system SHALL accept an optional `[observability.metrics.component_overrides]`
table mapping a component identity to a boolean, and SHALL resolve one
effective switch per component, with the master switch applied last:

```text
effective(component) = enabled && component_overrides.get(component).unwrap_or(components)
```

An absent override inherits the global `components` default. An explicit
override wins over the global default in both directions. `enabled =
false` suppresses every component series regardless of overrides. The
error family SHALL remain non-disableable: failures still forward.

#### Scenario: enabled true, components false, absent override

- **WHEN** `enabled = true`, `components = false`, and no override exists
  for `http`
- **THEN** the `http` component-operations series is not exported

#### Scenario: enabled true, components false, override false

- **WHEN** `enabled = true`, `components = false`, and
  `component_overrides.http = false`
- **THEN** the `http` component-operations series is not exported

#### Scenario: enabled true, components false, override true

- **WHEN** `enabled = true`, `components = false`, and
  `component_overrides.http = true`
- **THEN** the `http` component-operations series is exported and other
  components without an override stay suppressed

#### Scenario: enabled true, components true, absent override

- **WHEN** `enabled = true`, `components = true`, and no override exists
  for `http`
- **THEN** the `http` component-operations series is exported

#### Scenario: enabled true, components true, override false

- **WHEN** `enabled = true`, `components = true`, and
  `component_overrides.http = false`
- **THEN** the `http` component-operations series is not exported while
  other components keep their global default

#### Scenario: enabled true, components true, override true

- **WHEN** `enabled = true`, `components = true`, and
  `component_overrides.http = true`
- **THEN** the `http` component-operations series is exported

#### Scenario: master switch off beats an enabled override

- **WHEN** `[observability.metrics] enabled = false` and
  `component_overrides.http = true`
- **THEN** no component-operations series is exported for `http`
- **AND** `camel_errors_total` still receives component failures

#### Scenario: unregistered component override is inert

- **WHEN** `component_overrides` names a component that is not
  registered in the runtime
- **THEN** config loading succeeds and no series or error is produced for
  that identity

### Requirement: Override identities are validated at load

The system SHALL validate every `component_overrides` key as a component
identity: non-empty ASCII matching `[a-z0-9][a-z0-9_-]*`. Invalid keys
SHALL fail configuration loading with an error naming the offending key
and the `observability.metrics.component_overrides` path.

#### Scenario: empty key rejected

- **WHEN** `[observability.metrics.component_overrides]` contains an
  empty or whitespace-only key
- **THEN** config validation fails and the error names the key

#### Scenario: invalid characters rejected

- **WHEN** an override key contains uppercase letters, `.`, `:`, or other
  non-identity characters
- **THEN** config validation fails and cites
  `observability.metrics.component_overrides`

#### Scenario: valid identity accepted

- **WHEN** every override key is a lowercase identity such as `http`,
  `kafka`, or `camel-https`
- **THEN** validation passes

### Requirement: Metrics table survives profiles and includes

The system SHALL resolve `[observability.metrics.component_overrides]`
through the same recursive table layering as the rest of the
observability config: tables merge key-by-key, scalars and arrays
replace. A profile-qualified table
(`[default.observability.metrics.component_overrides]`) or an included
file SHALL therefore contribute and override individual entries rather
than replace the whole map.

#### Scenario: profile-qualified overrides apply

- **WHEN** `[default.observability.metrics.component_overrides] http = true`
  is activated by the selected profile
- **THEN** the effective config resolves the `http` override to `true`

#### Scenario: profile overrides one entry and inherits the rest

- **WHEN** the base table sets `component_overrides.http = true` and
  `component_overrides.kafka = true`, and the selected profile sets only
  `component_overrides.http = false`
- **THEN** the effective map has `http = false` and `kafka = true`

#### Scenario: include contributes overrides

- **WHEN** an included file defines
  `[observability.metrics.component_overrides] kafka = true`
- **THEN** the merged config contains the `kafka` override

