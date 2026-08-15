# dsl Specification

## Purpose
TBD - created by archiving change remove-header-dsl-verb. Update Purpose after archive.
## Requirements
### Requirement: remove_header declarative step

The DSL SHALL provide a `remove_header` declarative step that removes a single
named header from the exchange input message headers.

#### Scenario: remove an existing input header

- **GIVEN** a route containing `- remove_header: { key: CamelHttpPath }` and
  an exchange whose input message has header `CamelHttpPath` set
- **WHEN** the step executes
- **THEN** the header `CamelHttpPath` is absent from `exchange.input.headers`
  and all other input headers are preserved

#### Scenario: remove a non-existent key is a no-op

- **GIVEN** a route containing `- remove_header: { key: X-Not-Present }` and
  an exchange whose input message does NOT have header `X-Not-Present`
- **WHEN** the step executes
- **THEN** the exchange passes through unchanged and the step completes
  successfully (no error)

#### Scenario: input-only removal preserves output header

- **GIVEN** a route containing `- remove_header: { key: X-Shared }` and an
  InOut exchange where BOTH the input message and the output message carry
  header `X-Shared`
- **WHEN** the step executes
- **THEN** header `X-Shared` is removed from `exchange.input.headers` but is
  PRESERVED on `exchange.output.headers` (removal is input-only, matching
  SetHeader semantics)

#### Scenario: empty or whitespace-only key is rejected at compile time

- **GIVEN** a route containing `- remove_header: { key: "   " }` (whitespace
  only, treated as empty by `trim().is_empty()`)
- **WHEN** the route is compiled
- **THEN** compilation fails with an error message mentioning `remove_header`
  and the empty-key constraint (same guard as `set_header`)

### Requirement: remove_header in the route JSON schema

The route JSON schema (`route-schema.json`) SHALL include `remove_header` as a
valid step so that `camel-lint` accepts routes that use it.

#### Scenario: lint accepts a route with remove_header

- **GIVEN** the canonical route JSON schema and a route YAML containing a
  `remove_header` step with a non-empty `key`
- **WHEN** the route is validated against the schema
- **THEN** validation passes (the step is recognized as a permitted step kind)

### Requirement: CircuitBreaker route-level fallback sub-pipeline

The DSL SHALL accept an optional `fallback:` sub-pipeline (a list of steps) on the
route-level `circuit_breaker` configuration. When the circuit is open, the runtime SHALL
execute the fallback sub-pipeline instead of failing with `CircuitOpen`. Absent or empty
`fallback` SHALL produce identical behavior to today (no fallback processor configured).
The fallback sub-pipeline SHALL be part of the canonical route contract: it serializes
as a list of canonical steps and survives the canonical roundtrip.

#### Scenario: fallback declared in YAML parses and compiles

- **GIVEN** a YAML route with `circuit_breaker: { failure_threshold: 1, open_duration_ms: 60000, fallback: [ cache_peek_stale: { repository: persistent, key: "tile-xyz" } ] }`
- **WHEN** the route is parsed and compiled
- **THEN** parsing succeeds, the compiled route's circuit breaker runs the fallback
  sub-pipeline when the circuit is open, and the route schema
  (`route-schema.json`, both copies) validates the shape

#### Scenario: absent or empty fallback leaves behavior unchanged

- **GIVEN** a YAML route with `circuit_breaker: { failure_threshold: 1, open_duration_ms: 60000 }` (no `fallback` key)
- **WHEN** the route is compiled
- **THEN** `CircuitBreakerConfig.fallback` is `None` and route behavior is
  indistinguishable from a route compiled before this requirement existed

#### Scenario: open circuit executes the fallback sub-pipeline

- **GIVEN** a route whose circuit breaker has a `fallback: [ cache_peek_stale: { repository: persistent, key: "tile-xyz" } ]` and whose circuit is open
- **WHEN** an exchange arrives
- **THEN** the fallback sub-pipeline runs and the exchange body is produced by it
  (for the peek: the post-expiry cached value), instead of `CircuitOpen` propagating

#### Scenario: stopped fallback yields a clean outcome

- **GIVEN** a route whose circuit is open and whose fallback sub-pipeline stops (e.g.
  `cache_peek_stale` MISS with the default `on_miss: stop` policy)
- **WHEN** the fallback sub-pipeline signals stop
- **THEN** the circuit breaker fallback path surfaces `Ok(exchange)` with the Exchange
  state intact — no `CircuitOpen` and no error escapes, because the composed fallback
  pipeline already translates Stop to `Ok` at its `into_tower_result` boundary
  (ADR-0024/0025 single-translation-site)

#### Scenario: canonical roundtrip preserves fallback steps

- **GIVEN** a canonical route spec with `circuit_breaker.fallback` containing canonical steps
- **WHEN** the spec is serialized and deserialized back
- **THEN** the fallback step list is preserved, and routes serialized before this
  requirement (no `fallback` key) deserialize with an empty fallback

#### Scenario: unknown fields remain rejected

- **GIVEN** a YAML route with `circuit_breaker: { failure_threshold: 1, unknown_key: 1 }`
- **WHEN** the route is parsed
- **THEN** parsing fails with an unknown-field error (`deny_unknown_fields` intact)

#### Scenario: invalid nested fallback steps are rejected

- **GIVEN** a canonical route spec whose `circuit_breaker.fallback` contains an invalid
  step (e.g. an empty body where one is required)
- **WHEN** the canonical contract is validated
- **THEN** validation fails naming the fallback step (the same validations that apply to
  top-level steps recurse into `circuit_breaker.fallback`)

#### Scenario: builder reverse path fails closed on opaque fallback

- **GIVEN** a programmatic `CircuitBreakerConfig` with a compiled fallback processor
  (`BoxProcessor`)
- **WHEN** camel-builder attempts to reconstruct a canonical route spec from it
- **THEN** the reconstruction fails with an explicit error naming the opaque fallback
  (no silent loss, ADR-0016); DSL and canonical authored routes are unaffected

