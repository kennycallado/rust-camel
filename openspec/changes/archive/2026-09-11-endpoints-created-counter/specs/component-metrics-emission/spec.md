## ADDED Requirements

### Requirement: Endpoint creation is visible

The runtime SHALL count every endpoint creation in the resolver path as
`camel_core_endpoints_created_total{component=<scheme>}` (recorded in dot
form as `camel.core.endpoints_created_total`). The `component` label value
is the URI scheme of the created endpoint; its value set is bounded by the
registered component schemes and is an open label by construction (rc-haik).

#### Scenario: creating endpoints for two components increments each label once

- **GIVEN** a runtime with components registered under two schemes
  (`direct` and `mock`)
- **WHEN** one endpoint is created per scheme through the resolver
- **THEN** the counter carries one increment under `component="direct"` and
  one increment under `component="mock"`

#### Scenario: repeated dynamic URIs for the same component accumulate on that label

- **GIVEN** a runtime with a component registered under one scheme
- **WHEN** several endpoints with distinct dynamic URIs of that scheme are
  created through the resolver
- **THEN** the counter accumulates under the single
  `component=<scheme>` label
