## ADDED Requirements

### Requirement: Unified aggregate correlation-source lowering

The DSL aggregate step SHALL lower the correlation source identically on
the normal (builder) and canonical lowering paths: `header` alone lowers to
`CorrelationStrategy::HeaderName`, `correlation_key` alone lowers to
`CorrelationStrategy::Expression` with language `simple`, and when both are
present `correlation_key` SHALL override `header`. Validation SHALL require
at least one non-empty correlation source and SHALL reject empty
`correlation_key` strings. A single shared mapping helper SHALL produce the
strategy for both paths so they cannot diverge.

#### Scenario: Expression key through normal lowering

- **GIVEN** a declarative route with an aggregate step carrying
  `correlation_key: "${header.orderId}"` (with or without `header`)
- **WHEN** the route compiles through the normal builder path
  (`compile_declarative_route`)
- **THEN** the resulting `AggregatorConfig.correlation` is
  `CorrelationStrategy::Expression { expr: "${header.orderId}", language: "simple" }`

#### Scenario: Expression key through canonical lowering yields parity

- **GIVEN** the same declarative aggregate step as the normal-path scenario
- **WHEN** the route compiles through the canonical path
  (`compile_declarative_route_to_canonical` then canonical step lowering)
- **THEN** the resulting correlation strategy is identical to the normal
  path's (`Expression { expr: "${header.orderId}", language: "simple" }`)

#### Scenario: Header-only aggregate is accepted and stays header-based

- **GIVEN** a declarative route with an aggregate step carrying only
  `header: "orderId"` (no `correlation_key`)
- **WHEN** the route compiles through either lowering path
- **THEN** compilation succeeds and the correlation strategy is
  `CorrelationStrategy::HeaderName("orderId")`

#### Scenario: Both sources present — expression overrides header

- **GIVEN** a declarative aggregate step carrying both
  `header: "region"` and `correlation_key: "${header.orderId}"`
- **WHEN** the route compiles through either lowering path
- **THEN** the correlation strategy is `Expression` for the authored
  expression (the header is not used for correlation)

#### Scenario: Missing both sources fails validation

- **GIVEN** a declarative aggregate step with neither `header` nor
  `correlation_key` (or both empty)
- **WHEN** `validate_route` runs (normal or canonical entry point)
- **THEN** compilation fails with a `CamelError::Config` naming the
  correlation-source requirement

#### Scenario: Empty correlation_key string fails validation

- **GIVEN** a declarative aggregate step with `header: "region"` and
  `correlation_key: ""`
- **WHEN** `validate_route` runs
- **THEN** compilation fails with a `CamelError::Config` (an empty
  expression is not a correlation source, even beside a non-empty header)

#### Scenario: Header is optional at YAML parse

- **GIVEN** YAML with an aggregate step carrying only
  `correlation_key: "${header.orderId}"` (no `header` key)
- **WHEN** the YAML parses into a declarative route and compiles
- **THEN** parsing succeeds (header defaults to empty) and lowering yields
  `CorrelationStrategy::Expression`

#### Scenario: Builder canonicalization round-trips expression correlation

- **GIVEN** a builder-constructed `AggregatorConfig` whose correlation
  strategy is `Expression { expr, .. }`
- **WHEN** it canonicalizes to `CanonicalAggregateSpec` and recompiles
- **THEN** the recomputed correlation strategy is `Expression` for the same
  expression (the round trip preserves expression correlation)

#### Scenario: Canonical contract validation accepts expression-only

- **GIVEN** a `CanonicalStepSpec::Aggregate` with `header: ""` and a
  non-empty `correlation_key`
- **WHEN** `CanonicalRouteSpec::validate_contract` runs (canonical
  compilation / hot-reload entry)
- **THEN** validation passes (an empty header is legal exactly when the
  expression source is present)

#### Scenario: Canonical contract validation rejects missing both sources

- **GIVEN** a `CanonicalStepSpec::Aggregate` with an empty `header` and no
  `correlation_key`
- **WHEN** `CanonicalRouteSpec::validate_contract` runs
- **THEN** validation fails with a contract-violation error naming the
  correlation-source requirement

#### Scenario: Canonical contract validation rejects empty correlation_key

- **GIVEN** a `CanonicalStepSpec::Aggregate` with `header: "region"` and
  `correlation_key: Some("")`
- **WHEN** `CanonicalRouteSpec::validate_contract` runs
- **THEN** validation fails with a contract-violation error

#### Scenario: Generated schema marks header optional

- **GIVEN** the regenerated route-DSL schema artifacts (after `header`
  gains its serde default)
- **WHEN** the generated aggregate-verb schema is inspected
- **THEN** `header` is no longer in the required-properties list and the
  schema check gate passes

### Requirement: AggregatorConfigBuilder expression correlation setter

`camel-api` SHALL provide `AggregatorConfigBuilder::correlate_by_expr(expr,
language)`, an override setter that sets the pending config's correlation
strategy to `CorrelationStrategy::Expression` for the given expression and
language, leaving `header_name` untouched (same override semantics as the
existing `correlate_by` setter).

#### Scenario: Setter overrides correlation, leaves header_name untouched

- **GIVEN** an `AggregatorConfigBuilder` bootstrapped with
  `AggregatorConfig::correlate_by("orderId")`
- **WHEN** `.correlate_by_expr("${header.orderId}", "simple")` is applied
  and the config is built
- **THEN** `config.correlation` is `Expression { expr: "${header.orderId}",
  language: "simple" }` and `config.header_name` remains `"orderId"`
