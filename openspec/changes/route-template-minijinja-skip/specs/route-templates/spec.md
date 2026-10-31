## ADDED Requirements

### Requirement: Minijinja sources are exempt from route-template placeholder substitution

The system SHALL detect expression objects carrying `language: "minijinja"`
and a string `source` while walking template JSON for placeholder
substitution, and SHALL leave that `source` value byte-identically
unchanged by the route-template placeholder engine. A declared
route-template parameter SHALL NOT be substituted inside a minijinja
source, including on exact name collision with a minijinja variable.
Expression sources of other languages (`rhai`, `simple`, `jsonpath`,
`xpath`) and plain template strings SHALL keep the existing substitution
and unknown/missing-parameter validation unchanged.

#### Scenario: minijinja source compiles with literal braces

- **GIVEN** a template whose route contains a script step with
  `language: "minijinja"` and source `{{ headers.name }}`, declared with no
  route-template parameters, and a `templated_routes` instantiation
- **WHEN** the template is materialized and compiled
- **THEN** the script source reaches the DSL compiler byte-identically and
  no `UnknownParameter` error is raised

#### Scenario: colliding parameter name not substituted inside minijinja

- **GIVEN** a template declaring the parameter `body` and a minijinja
  source containing `{{ body }}`
- **WHEN** the template is materialized with a value for `body`
- **THEN** the minijinja source is unchanged, while the same parameter
  still substitutes in plain template strings outside the minijinja node

#### Scenario: normal placeholders still substitute and validate

- **GIVEN** a template with `{{param}}` in plain strings (embedded and
  whole-node typed forms) and an `{{unknown}}` occurrence
- **WHEN** templates are materialized
- **THEN** declared parameters substitute as before and the unknown
  occurrence still fails with `UnknownParameter`

### Requirement: Escaped braces render literally in normal template strings

The system SHALL treat the sequence `\{{` in a normal template string as an
escape that renders a literal `{{`, and the escaped text SHALL NOT be
subject to parameter substitution or whole-node typed substitution.

#### Scenario: escaped braces render literally and block substitution

- **GIVEN** a template declaring the parameter `body` with a plain string
  containing `\{{body}}`
- **WHEN** the template is materialized with a value for `body`
- **THEN** the output contains the literal text `{{body}}` and the
  parameter value is not substituted
