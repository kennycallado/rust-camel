# json-schema-validation Specification

## Purpose
TBD - created by archiving change jsonschema-format-enforcement. Update Purpose after archive.
## Requirements
### Requirement: JSON Schema format enforcement by default

The REST `request_schema` validation path (`JsonSchemaValidateService`)
SHALL compile its schema with format validation enabled, so a declared
`format` constrains the payload on every JSON Schema draft, with the
schema's `$schema` key still selecting draft semantics (2020-12 when
absent).

#### Scenario: format email rejects a malformed value

- **GIVEN** a REST operation whose `request_schema` declares
  `{"type": "object", "properties": {"email": {"type": "string", "format": "email"}}}`
- **WHEN** a request body carries `"email": "not-an-email"`
- **THEN** validation fails with `CamelError::ValidationError`

#### Scenario: format email accepts a valid value

- **GIVEN** the same `request_schema`
- **WHEN** a request body carries `"email": "user@example.com"`
- **THEN** validation succeeds and the route proceeds

#### Scenario: pattern enforcement is unchanged

- **GIVEN** a `request_schema` declaring a `pattern` property
- **WHEN** a request body violates the pattern
- **THEN** validation fails as before the change

#### Scenario: unknown format stays an annotation

- **GIVEN** a `request_schema` declaring `{"type": "string", "format": "unknown-format-x"}`
- **WHEN** a request body carries any string
- **THEN** validation succeeds; unknown formats are never errors (the
  jsonschema crate's `ignore_unknown_formats` default)

#### Scenario: draft-07 schema enforces format

- **GIVEN** a `request_schema` with `$schema` set to the draft-07
  meta-schema URI and a `format: email` property
- **WHEN** a request body carries a malformed email
- **THEN** validation fails

#### Scenario: schema without $schema enforces format as 2020-12

- **GIVEN** a `request_schema` with no `$schema` key and a
  `format: email` property
- **WHEN** a request body carries a malformed email
- **THEN** validation fails (2020-12 semantics with format enforcement)

### Requirement: Validator component format enforcement default

The validator component SHALL compile JSON and YAML schemas with format
validation enabled by default, with `$schema` auto-detection preserved.

#### Scenario: validator JSON schema enforces format by default

- **GIVEN** a `validator://` endpoint over a JSON schema declaring
  `format: email`
- **WHEN** a body carries a malformed email
- **THEN** validation fails

#### Scenario: validator YAML schema enforces format by default

- **GIVEN** a `validator://` endpoint over a YAML schema declaring
  `format: email`
- **WHEN** a body carries a malformed email
- **THEN** validation fails

### Requirement: Validator component format opt-out

The validator component SHALL accept `[components.validator]
validate-formats = false` in application configuration, restoring
annotation-only format behavior; the default SHALL be `true`. Only an
exact boolean is accepted; any other value SHALL fail config load with an
error naming `components.validator.validate-formats`.

#### Scenario: opt-out restores previous accept behavior

- **GIVEN** `[components.validator]` with `validate-formats = false` and a
  validator endpoint over a schema declaring `format: email`
- **WHEN** a body carries a malformed email
- **THEN** validation succeeds (format is an annotation, as before the
  change)

#### Scenario: invalid opt-out value fails config load

- **GIVEN** `[components.validator]` with `validate-formats = "no"`
- **WHEN** configuration loads
- **THEN** load fails with an error naming
  `components.validator.validate-formats`

### Requirement: REST request_schema has no format opt-out

REST `request_schema` format enforcement SHALL be always-on. The REST DSL
SHALL NOT introduce a per-operation format opt-out key.

#### Scenario: request_schema format enforcement cannot be disabled per operation

- **GIVEN** a REST operation with `request_schema` declaring
  `format: email`
- **WHEN** the route is authored
- **THEN** no DSL key exists to disable format enforcement for that
  operation; the behavior is documented as always-on

