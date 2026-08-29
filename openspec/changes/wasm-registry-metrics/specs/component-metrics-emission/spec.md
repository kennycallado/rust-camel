## MODIFIED Requirements

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
