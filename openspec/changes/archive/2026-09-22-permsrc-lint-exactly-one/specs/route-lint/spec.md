## ADDED Requirements

### Requirement: R-SCHEMA mirrors runtime exactly-one permission value sources

The route schema SHALL require each `security_policy.permission`
`resource`/`action` value source to specify exactly one non-null source
among `literal`, `header`, and `property` — mirroring the runtime
compilation contract (`yaml_source_to_value_source`, rc-gddb2
REJECT-ON-AMBIGUITY). An explicit `null` sibling SHALL count as absent
(mirroring serde `Option` deserialization). The R-SCHEMA rule SHALL render
violations as one Error per value spec, anchored on the value mapping, with
a message using the exact runtime error wording: naming the field
(`resource` or `action`), `must specify exactly one of: literal, header, or
property`, and the set of non-null sources found (`none set` when empty,
canonical `literal, header, property` order otherwise). Non-permission
`oneOf` failures SHALL keep their pre-change diagnostic shape.

#### Scenario: zero sources

- **GIVEN** a route whose `security_policy.permission.resource` is an empty mapping (or contains only null-valued sources)
- **WHEN** the R-SCHEMA rule validates the document
- **THEN** exactly one `R-SCHEMA` Error is emitted for that value spec, anchored on the resource mapping, with a message containing `resource`, `exactly one of: literal, header, or property`, and `(set: none set)`

#### Scenario: multiple sources

- **GIVEN** a route whose `security_policy.permission.action` sets two or more non-null sources (e.g. `literal` and `header`)
- **WHEN** the R-SCHEMA rule validates the document
- **THEN** exactly one `R-SCHEMA` Error is emitted for that value spec, anchored on the action mapping, with a message containing `action` and the found set in canonical order (e.g. `(set: literal, header)`)

#### Scenario: exactly one source

- **GIVEN** a route whose `security_policy.permission.resource` sets exactly one non-null source
- **WHEN** the R-SCHEMA rule validates the document
- **THEN** no `R-SCHEMA` diagnostic is emitted for that value spec

#### Scenario: explicit null sibling

- **GIVEN** a route whose `security_policy.permission.resource` sets one non-null source and one or more explicit `null` siblings
- **WHEN** the R-SCHEMA rule validates the document
- **THEN** no `R-SCHEMA` diagnostic is emitted for that value spec (runtime accepts it: serde treats null as absent)

#### Scenario: other oneOf failures unchanged

- **GIVEN** a document that fails a non-permission `oneOf` in the route schema (e.g. a malformed credential source)
- **WHEN** the R-SCHEMA rule validates the document
- **THEN** the emitted diagnostic is byte-identical to the pre-change collapsed oneOf diagnostic for that shape

#### Scenario: adjacent malformed shapes stay generic

- **GIVEN** a permission value spec that is a scalar (e.g. `resource: orders`) or carries an unknown key in addition to a valid source
- **WHEN** the R-SCHEMA rule validates the document
- **THEN** the emitted diagnostics are the generic type/`AdditionalProperties` diagnostics — no targeted exactly-one diagnostic is synthesized

#### Scenario: schema artifact sync

- **GIVEN** the exactly-one constraint is encoded in the generated schema
- **WHEN** `cargo xtask schema --check` runs
- **THEN** it passes: `schemas/dsl/route-schema.json` is the regenerated output and `crates/camel-lint/schema/route-schema.json` is byte-equal to it
