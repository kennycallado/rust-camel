## MODIFIED Requirements

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

The source-preserving endpoint-creation failure
(`EndpointCreationFailedWithSource`) SHALL be matched by the existing
`EndpointCreationFailed` kind value: that kind value denotes the
endpoint-creation failure family, so existing catch clauses keep catching
endpoint-creation failures when a producer migrates to the
source-preserving variant. This compatibility grouping is an intentional
exception to the variant-exact `ProcessorErrorWithSource` precedent (which
matches only its own kind value and is NOT matched by `ProcessorError`);
the exception exists because endpoint-creation catch clauses predate the
variant split and must not silently stop firing. No distinct
`EndpointCreationFailedWithSource` kind value SHALL exist. The inventory
guard test SHALL classify the variant as grouped under
`EndpointCreationFailed`.

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

#### Scenario: EndpointCreationFailed kind matches both endpoint-creation variants

- **GIVEN** an `on_exceptions` clause with `kind: "EndpointCreationFailed"`
- **WHEN** the clause is evaluated against a plain
  `CamelError::EndpointCreationFailed(d)` and against a
  `CamelError::EndpointCreationFailedWithSource(d, source)`
- **THEN** the clause matches both errors (compatibility grouping — the
  kind value denotes the endpoint-creation failure family)

#### Scenario: no distinct EndpointCreationFailedWithSource kind value

- **GIVEN** an `on_exceptions` clause with
  `kind: "EndpointCreationFailedWithSource"`
- **WHEN** the route compiles
- **THEN** compilation fails with the unknown-kind error listing the
  supported kinds

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

#### Scenario: inventory guard classifies the grouped endpoint-creation variant

- **GIVEN** the classification test table enumerating every `CamelError`
  variant
- **WHEN** the `EndpointCreationFailedWithSource` variant is checked
- **THEN** the table classifies it as grouped under the
  `EndpointCreationFailed` kind value, so a future variant added without
  a classification entry fails the guard
