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

