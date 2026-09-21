# Delta: dsl

## ADDED Requirements

### Requirement: on_exceptions exception-kind vocabulary coverage

The `error_handler.on_exceptions` kind vocabulary SHALL include every
`CamelError` variant that is raised during data-plane pipeline execution
and reaches the configured route error handler. Specifically,
`UnsupportedMediaType` (415-class, raised by the media negotiation gate),
`NotAcceptable` (406-class, same gate), and `ProcessorErrorWithSource`
(raised by source-carrying producers such as bean, exec, surrealdb) SHALL
be accepted kind values matching their own variants structurally. Variants
raised only at startup fail-fast (`ConfigValidation`, `EndpointUri`) SHALL
be documented as intentionally unmatchable and SHALL remain rejected as
unknown kinds. A test SHALL pin the current variant inventory: every
enumerated `CamelError` variant SHALL be classified as matchable,
intentionally unmatchable, or deferred-pending-decision, so vocabulary and
classification stay consistent.

#### Scenario: UnsupportedMediaType is matchable

- **GIVEN** an `on_exceptions` clause with `kind: "UnsupportedMediaType"`
- **WHEN** the route compiles and a step fails with
  `CamelError::UnsupportedMediaType { consumed, declared }`
- **THEN** the clause matches that error, and it does not match an
  unrelated variant such as `ValidationError`

#### Scenario: NotAcceptable is matchable

- **GIVEN** an `on_exceptions` clause with `kind: "NotAcceptable"`
- **WHEN** the route compiles and a step fails with
  `CamelError::NotAcceptable { accept, produced }`
- **THEN** the clause matches that error, and it does not match an
  unrelated variant such as `ValidationError`

#### Scenario: ProcessorErrorWithSource is matchable and distinct from the ProcessorError alias

- **GIVEN** an `on_exceptions` clause with `kind: "ProcessorErrorWithSource"`
- **WHEN** the route compiles and a step fails with
  `CamelError::ProcessorErrorWithSource(msg, source)`
- **THEN** the clause matches that error
- **WHEN** a clause with `kind: "ProcessorError"` is evaluated against the
  same error
- **THEN** it does not match, because `variant_name()` aliasing to
  `"ProcessorError"` (doTry catch-by-variant compat) does not extend to
  structural `on_exceptions` matching

#### Scenario: intentionally unmatchable kinds stay rejected

- **GIVEN** `on_exceptions` clauses with `kind: "ConfigValidation"`,
  `kind: "EndpointUri"`, or `kind: "TemplateReload"`
- **WHEN** the route compiles
- **THEN** compilation fails with the unknown-kind error listing the
  supported kinds

#### Scenario: vocabulary-variant guard test pins the classification inventory

- **GIVEN** the classification test table enumerating every `CamelError`
  variant as matchable, intentionally unmatchable, or
  deferred-pending-decision
- **WHEN** the vocabulary changes without a matching classification entry,
  or a table entry claims matchability for a kind absent from the
  vocabulary
- **THEN** the guard test fails, so the table and the vocabulary stay
  consistent for the enumerated inventory; keeping the inventory itself
  current for newly added variants is a reviewed manual step anchored by
  the camel-api exhaustive `variant_name` test
