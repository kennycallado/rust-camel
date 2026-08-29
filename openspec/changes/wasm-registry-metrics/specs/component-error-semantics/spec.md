## ADDED Requirements

### Requirement: Registry-resolved components deliver errors to the wired collector

The wasm registry component context SHALL return the collector it was
constructed with from `ComponentContext::metrics()` — the late-bound
handle per ADR-0066 — and SHALL NOT substitute `NoOpMetrics` when a
collector was provided.

#### Scenario: registry context returns the wired collector

- **GIVEN** a `RegistryComponentContext` constructed with a recording
  collector
- **WHEN** `metrics()` is called
- **THEN** the returned collector is the one provided at construction

#### Scenario: error family flows through the facade

- **GIVEN** a `RegistryComponentContext` constructed with a recording
  collector
- **WHEN** a failed operation is reported through the blanket
  `RuntimeObservability::component_metrics()` facade
- **THEN** `increment_errors` reaches the wired collector, not NoOp

#### Scenario: no collector wired keeps NoOp semantics

- **GIVEN** a `RegistryComponentContext` constructed without a
  collector (compile-time security scan path)
- **WHEN** `metrics()` is called
- **THEN** a NoOp collector is returned and nothing panics
