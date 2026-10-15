## MODIFIED Requirements

### Requirement: Language evaluation errors fail the step

Each step verb that evaluates a language Expression or Predicate SHALL return
`Err(CamelError)` when evaluation returns `Err(LanguageError)`. No verb SHALL
substitute `null`, `false`, an empty string, or `Body::Empty` for an error. The
rule covers at least: `set_property`, `set_header`, `set_header_if_absent`,
`set_body` (dynamic form), `filter`, `choice/when`, `loop while`, `validate`,
catch `when`/`on_when`, finally `on_when`, `split`, `dynamic_router`,
`routing_slip`, `recipient_list`, `idempotent_consumer` message id, `sort` key,
and `claim_check` key. Script SHALL NOT provide a non-mutating fallback;
mutating script evaluation errors SHALL continue to fail the step.

#### Scenario: Mutating verb fails loudly

- **GIVEN** a route with `set_property: {name: x, rhai: '"abc".parse_float()'}`
- **WHEN** the step evaluates the expression and the Language returns an evaluation error
- **THEN** the step SHALL return `Err(CamelError)` and the property `x` SHALL NOT be set on the Exchange

#### Scenario: Predicate verb fails loudly

- **GIVEN** a route whose `filter` predicate raises an evaluation error
- **WHEN** the filter step runs
- **THEN** the step SHALL return `Err(CamelError)` and the Exchange SHALL NOT be dropped silently

#### Scenario: Routing verb fails loudly

- **GIVEN** a `dynamic_router`, `routing_slip`, `recipient_list`, `sort`, `idempotent_consumer`, or `claim_check` step whose language expression raises an evaluation error
- **WHEN** the step runs
- **THEN** the step SHALL return `Err(CamelError)`; the route SHALL NOT continue as if the expression evaluated to `null`

## ADDED Requirements

### Requirement: A value-source mapping is never a literal

A YAML or JSON mapping directly under `set_body` or `transform` SHALL be treated
as value-source configuration, never as an implicit literal object. A mapping
that fails `SetBodyConfig` deserialization SHALL produce
`Err(CamelError::RouteError)` during shared route lowering. Explicit
`value: {...}` objects SHALL remain literal bodies. Existing valid configs,
scalars, arrays, null and bare `${...}` string behavior SHALL remain unchanged.
Existing exactly-one-source validation SHALL remain unchanged.

The failure message SHALL begin with the verb followed by `:`; it SHALL include
the alternatives `` `value`, `language`+`source`, `simple`, `rhai`, `jsonpath`, `xpath` ``
in that order and the hint `To set an object literal body, wrap it:` followed by
`{verb}: { value: { ... } }`. Messages SHALL NOT include map values or raw serde
error details. Unknown keys SHALL be listed lexicographically, backticked,
separated by `, `, after `unknown key(s)`, before `in value-source mapping`.
When there are no unknown keys, a wrong-type field SHALL be identified by fixed
text `wrong type for key` followed by its backticked name. If several fields have
wrong types, the first in declaration order (`language`, `source`, `simple`,
`rhai`, `jsonpath`, `xpath`) SHALL be reported. Unknown keys SHALL take priority.
An unexpected object failure SHALL still be rejected without echoing values.

The generated JSON schema SHALL restrict the literal alternative to string,
number, boolean, array and null. Objects SHALL validate only as `SetBodyConfig`.
Both generated schema copies SHALL agree; camel-lint's container-key derivation
SHALL remain unchanged. The broad TS binding SHALL remain advisory; JSON schema
SHALL be documented as the enforcement artifact.

#### Scenario: Unknown map key is rejected on both verbs and formats

- **GIVEN** YAML or JSON containing `set_body: {constant: ok}` or its `transform` equivalent
- **WHEN** the route is parsed and lowered
- **THEN** it SHALL fail with `RouteError`, its verb prefix, offending key `constant`, all valid alternatives and a `value: {` hint; the invalid route SHALL NOT reach registration

#### Scenario: Mixed keys and sorted unknown keys

- **GIVEN** a value-source map with valid `simple` and unknown `zeta`, `alpha`, whose values contain secrets
- **WHEN** YAML or JSON lowering rejects it
- **THEN** the offending-key list SHALL be `` `alpha`, `zeta` `` and SHALL NOT include `simple` or any values; `simple` SHALL still appear in the valid-alternatives list

#### Scenario: Wrong type does not disclose its value

- **GIVEN** `set_body: {simple: 123}` or its JSON or `transform` equivalent
- **WHEN** the route is lowered
- **THEN** the message SHALL contain ``wrong type for key `simple` ``, valid alternatives and the wrapper hint, and SHALL NOT contain `123`

#### Scenario: Wrapped object and existing non-object literals remain valid

- **GIVEN** either verb in either format with `{value: {constant: ok}}`, or a scalar, array (including nested objects), null or bare `${body}` string
- **WHEN** the route is lowered
- **THEN** the wrapped object SHALL become `ValueSourceDef::Literal` with the original object; all other forms SHALL retain their existing literal or Simple-expression behavior

#### Scenario: Schema rejects bare maps and preserves containers

- **GIVEN** regenerated DSL and lint schema copies
- **WHEN** schema validation checks a bare `{constant: ok}` mapping, a wrapped object and valid configs for both verbs
- **THEN** the bare map SHALL fail, wrapped objects and valid configs SHALL pass, the copies SHALL agree and the lint container-key set SHALL remain unchanged

### Requirement: script requires a mutating expression

A script step SHALL require its registered Language to support
`create_mutating_expression`. This policy SHALL apply identically to YAML,
JSON, Rust builder, canonical/runtime commands and hot-reload DryCompile.
Compilation SHALL NOT replace a script with read-only body evaluation.

For `LanguageError::NotSupported`, both builder variants SHALL return
`CamelError::RouteError` with this byte-identical message (substituting the
requested registered language name for `{lang}`):

```
script: language '{lang}' does not support mutating expressions; a `script` step requires a language that implements MutatingExpression. To replace the body with a read-only expression, use a `set_body`/`transform` step with this language instead
```

The message SHALL NOT list built-in language names or source values. Unknown
language resolution errors and other mutating-expression creation errors SHALL
retain existing handling. Languages that support mutation SHALL retain existing
script behavior and error propagation, including custom registered Languages.
Malformed script syntax SHALL remain a hard parse failure; improving the opaque
untagged error is outside this requirement.

#### Scenario: Read-only scripts fail before execution on all surfaces

- **GIVEN** equivalent scripts using simple or jsonpath in YAML, JSON, builder, canonical/runtime commands or hot-reload DryCompile
- **WHEN** the route is compiled for add or replacement
- **THEN** compilation SHALL fail with the shared capability message, no fallback step SHALL execute and no body SHALL be replaced; a rejected replacement SHALL NOT replace the previously valid route

#### Scenario: Both builder variants produce identical errors

- **GIVEN** `BuilderStep::Script` and `BuilderStep::DeclarativeScript` with the same read-only language and source
- **WHEN** both are compiled with the same registry
- **THEN** both SHALL return `RouteError` with byte-identical messages

#### Scenario: Mutating scripts remain supported

- **GIVEN** a registered rhai, js or custom Language supporting `MutatingExpression`
- **WHEN** a script is compiled and executed
- **THEN** it SHALL execute the mutating expression with existing eval metadata and SHALL propagate failures without using `SetBody`

### Requirement: Breaking route examples preserve semantics and coverage

All current in-repository bare-map and non-mutating script examples identified
by the accepted inventory SHALL be migrated. Object fixtures SHALL use explicit
`value:` wrappers without changing their current literal JSON objects. Read-only
script examples SHALL use `transform: {language, source}` to retain body-evaluation
behavior. Parser-only script samples SHALL represent valid mutating scripts.
Foreign Apache Camel syntax and historical change archives SHALL NOT be migrated.

The basic and transformation docs SHALL include the proposal's migration text.
Related README, EIP, OpenSearch, minijinja, AST comments, route-definition comments
and DSL glossary SHALL match the policy. The glossary gate SHALL be marked
BREAKING. Tests removing `script_readonly` rows SHALL retain a surviving
`secret=abc` redaction seed and its redaction assertions.

#### Scenario: Inventory migrations include newly discovered sites

- **GIVEN** B1-B3, S1, T1-T4, four `camel-test/tests/cxf_test.rs` constant-map routes and `camel-dsl/README.md`'s simple script example
- **WHEN** the change is implemented
- **THEN** all SHALL be migrated and a per-site post-change DSL parser sweep of route files and extracted route examples under `examples/`, `benchmarks/scenarios/` and `docs/` SHALL report zero failures caused by this change; incomplete-fragment exclusions and unrelated failures SHALL be recorded

#### Scenario: Redaction coverage survives fallback removal

- **GIVEN** the expression-failure suite no longer includes read-only script rows
- **WHEN** the surviving error cases execute
- **THEN** at least one SHALL carry the `secret=abc` seed and existing redaction assertions SHALL verify that the secret is absent from the exposed diagnostics
