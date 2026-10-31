# Delta: language-value-boundary — missing-value semantics

## ADDED Requirements

### Requirement: Missing-value semantics follow a per-language contract

A value that is absent at evaluation time (a missing header, property, or body field)
SHALL behave per language as follows, and each behavior SHALL be pinned by a test in the
owning language crate:

- **MiniJinja (strict undefined, fail-closed)**: interpolation (`{{ headers.missing }}`)
  and conditional use (`{% if headers.missing %}`) of a missing path SHALL fail the
  render with a redacted evaluation error naming the undefined path or the undefined
  kind. Strict undefined SHALL NOT be silently relaxed; the documented opt-out SHALL be
  an explicit `default` filter or `is defined` test inside the template (verified against
  the pinned MiniJinja version).
- **Simple (lenient print, strict bool)**: a whole missing slot SHALL evaluate to
  `Value::Null`; a missing slot in a mixed template SHALL render as the empty string. A
  bare predicate on a missing value SHALL fail with a `bool` type mismatch — Simple SHALL
  NOT apply truthiness at the predicate boundary (truthiness applies only to `&&`/`||`
  operands, where null and empty-string are false).
- **Rhai (unit for absent reads, strict bool)**: `header("missing")` and
  `headers["missing"]` SHALL return Rhai UNIT, which SHALL land as `Value::Null` on the
  camel `Value` boundary. A predicate resolving to UNIT SHALL fail with a `bool` type
  mismatch, and an `if` whose condition is UNIT SHALL fail with a Rhai runtime error.

No language SHALL silently change an absent value into a non-empty substitution, and
MiniJinja SHALL remain the only language whose missing-value read fails the whole
evaluation.

#### Scenario: minijinja interpolation of a missing path fails the render

- **GIVEN** a template `{% autoescape "none" %}x{{ headers.missing }}x{% endautoescape %}`
  and an exchange with no `missing` header
- **WHEN** the expression is evaluated
- **THEN** the evaluation SHALL fail, and the error text SHALL reference the undefined
  path or undefined value without quoting exchange data

#### Scenario: minijinja conditional on a missing path fails the render

- **GIVEN** a template whose `{% if headers.missing %}` branch references a missing header
- **WHEN** the expression is evaluated
- **THEN** the evaluation SHALL fail with the same strict-undefined error shape as
  interpolation

#### Scenario: minijinja explicit opt-out renders without failing

- **GIVEN** a template that guards or defaults the read explicitly
  (`{{ headers.missing | default('') }}` and/or `{% if headers.missing is defined %}`)
- **WHEN** the expression is evaluated against an exchange with no `missing` header
- **THEN** the render SHALL succeed with the explicitly authored fallback output (pinned
  only if the pinned MiniJinja version supports it under strict undefined; otherwise the
  docs SHALL state that no opt-out exists)

#### Scenario: simple whole-slot and mixed-template missing values stay lenient

- **GIVEN** the expressions `${header.missing}` and `prefix-${header.missing}-suffix`
- **WHEN** each is evaluated against an exchange with no `missing` header
- **THEN** the whole slot SHALL evaluate to `Value::Null` and the mixed template SHALL
  render `prefix--suffix`

#### Scenario: simple bare predicate on a missing value is a type mismatch

- **GIVEN** a predicate `${header.missing}` created with `create_predicate`
- **WHEN** it is matched against an exchange with no `missing` header
- **THEN** the match SHALL fail with `LanguageError::TypeMismatch` naming `expected bool`,
  and SHALL NOT coerce to false

#### Scenario: rhai missing reads return unit and land as null

- **GIVEN** a read-only expression `header("missing")` and one `headers["missing"]`
- **WHEN** each is evaluated against an exchange with no `missing` header
- **THEN** each evaluation SHALL succeed and the value SHALL be `Value::Null` on the
  camel `Value` boundary (Rhai UNIT)

#### Scenario: rhai predicate on a missing value is a type mismatch

- **GIVEN** a bare predicate `header("missing")` (not an explicit `== null` comparison)
- **WHEN** it is matched against an exchange with no `missing` header
- **THEN** the match SHALL fail with `LanguageError::TypeMismatch` naming `expected bool`

#### Scenario: rhai if on a missing value fails with a runtime error

- **GIVEN** a read-only expression `if header("missing") { "yes" } else { "no" }`
- **WHEN** it is evaluated against an exchange with no `missing` header
- **THEN** the evaluation SHALL fail with a Rhai runtime error (UNIT is not a valid `if`
  condition), not silently take the else branch
