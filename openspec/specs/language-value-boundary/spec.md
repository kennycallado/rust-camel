# language-value-boundary Specification

## Purpose
TBD - created by archiving change language-value-boundary. Update Purpose after archive.
## Requirements
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

### Requirement: Evaluation errors are route exceptions

A language evaluation error SHALL be visible to `do_try`/`catch` and to
`on_exception` like any other step failure. Exchange mutations from the failed
step SHALL NOT be applied. The rollback semantics that already apply to
`script:` SHALL extend to every verb in this capability.

#### Scenario: do_try catches an expression error

- **GIVEN** a `do_try` block whose guarded step contains a failing language
  expression
- **WHEN** the block runs
- **THEN** the `catch` clause SHALL receive the failure as a catchable
  exception, and the Exchange SHALL keep the state it had before the failed
  step

#### Scenario: on_exception observes the failure

- **GIVEN** a route with `on_exception` and a step whose language expression
  fails
- **WHEN** the step fails
- **THEN** `on_exception` SHALL run with the typed evaluation error as the
  route error

### Requirement: Evaluation errors carry typed diagnostics

A language evaluation error SHALL carry: the language name, the route id, the
step id or verb plus index, the script position (`line:col` where the Language
reports one), and an error class. The class set SHALL include at least:
`runtime`, `arithmetic`, `type-mismatch`, `function-not-found`, `limit`,
`timeout`, `conversion`, `parse`.

#### Scenario: Error content is complete

- **GIVEN** a route `r1` whose step `s2` evaluates a rhai expression that
  raises `ErrorArithmetic` at a known position
- **WHEN** the step fails
- **THEN** the route error SHALL identify language `rhai`, route `r1`, step
  `s2` with its verb, the position, and class `arithmetic`

### Requirement: Evaluation errors are matchable by variant

The engine SHALL expose a dedicated `CamelError` variant for language
evaluation failures (`ExpressionFailed`). Route error matching SHALL select it
by variant, without string matching. No compatibility alias to
`ProcessorError` SHALL exist (owner decision 2026-10-02: pre-1.0 clean break).

#### Scenario: catch matches the variant

- **GIVEN** a `catch: {exception: [ExpressionFailed]}` clause
- **WHEN** a guarded step's expression fails
- **THEN** the clause SHALL match on the error variant

### Requirement: Evaluation error messages exclude exchange data

Error text SHALL NOT contain exchange data: header values, property values,
body content, or strings the script operated on. Every language error class
whose text can embed values SHALL be reduced to class plus position. Script
source excerpts MAY appear in error text, because scripts are trusted operator
configuration (ADR-0032). This rule binds `CamelError` rendering, logs, and
dead-letter-channel payloads (ADR-0051).

#### Scenario: Failing parse input stays out of the error

- **GIVEN** a rhai expression `"SECRET".parse_float()` where `SECRET` came
  from exchange data
- **WHEN** the expression fails and the error is rendered into `CamelError`,
  a log line, or a DLC payload
- **THEN** the output SHALL NOT contain the text `SECRET`; it SHALL name the
  class and position instead

### Requirement: Language crates do not own evaluation-failure log levels

Language crates SHALL NOT choose the log level for evaluation failures
(ADR-0012 handler contract). They MAY emit `debug!` records. The route error
handler owns the operational signal and its level.

#### Scenario: No warn from the language crate

- **GIVEN** a language evaluation error that propagates to the route
- **WHEN** the failure is logged
- **THEN** the language crate SHALL NOT have emitted a `warn!` or `error!`
  record for it; the handler's record is the operational signal

### Requirement: In-script error handling suppresses handled errors

An in-script error-handling construct that handles an error SHALL suppress it.
Only uncaught errors reach the route. EXCEPTION (boundary refusal): a
stream-body read refusal on an operation OUTSIDE the language's registered
guard surface (in rhai: not `to_string`, `to_debug`, or the operators `==`,
`!=`, `<`, `>`, `+`, `-`, `*`, `/`) is a boundary refusal, not a script
error; in-script handling SHALL NOT suppress it. An outside-surface
operation that fails BEFORE materializing the value (in rhai: indexing a
stream body raises the engine's indexing error without cloning it) is an
ordinary script error and in-script handling SHALL suppress it. For rhai,
the statement form is the documented in-script error-handling construct; a
custom expression form SHALL NOT be added for it.

#### Scenario: rhai try statement handles the failure

- **GIVEN** a rhai script that wraps a failing call in a
  `try { ... } catch (err) { ... }` statement and returns a fallback value
- **WHEN** the step evaluates the script
- **THEN** the step SHALL succeed with the fallback value; no route error
  SHALL be raised

### Requirement: Structured values round-trip natively

For every value `v` composed of strings, booleans, integers, finite floats,
unit/null, arrays, and objects (recursively), storing `v` through the boundary
and reading it back in a later evaluation of the same Language SHALL return an
equal value with its type preserved. The same SHALL hold for converting any
JSON value into the Language and back, except JSON integers greater than
`i64::MAX`, which the inbound conversion SHALL refuse with a typed error
(owner decision 2026-10-02: no lossy float conversion).

#### Scenario: Map survives a property round-trip

- **GIVEN** a rhai script step that stores `#{"a": 1, "b": [2, 3]}` into a
  property
- **WHEN** a later rhai script reads that property and reports its type
- **THEN** the later script SHALL see a map equal to the stored value, and the
  reported type SHALL be `map`

#### Scenario: Large unsigned integer is refused

- **GIVEN** an exchange property holding a JSON integer greater than
  `i64::MAX`
- **WHEN** a rhai script reads that property
- **THEN** the evaluation SHALL fail with a typed conversion error; the engine
  SHALL NOT store a lossy float

### Requirement: Unrepresentable values are refused

Conversion from a Language value to an engine value SHALL be fallible. Values
the engine cannot represent — not-a-number, positive or negative infinity,
function pointers, closures, timestamps, and custom types — SHALL yield a
typed `conversion` error that names the source type and the target (property
name, header name, or body). A `to_string()` fallback SHALL NOT be used as a
conversion.

#### Scenario: NaN property is refused

- **GIVEN** a rhai script step whose result is `0.0/0.0`
- **WHEN** the engine converts the result into a property
- **THEN** the step SHALL fail with a `conversion` error naming the type and
  target; the property SHALL NOT hold `null` or a string

#### Scenario: No debug-string fallback

- **GIVEN** a rhai value of a custom or function type returned at the boundary
- **WHEN** the engine converts it
- **THEN** the engine SHALL NOT store the value's `Display` or `Debug` text

### Requirement: char and Blob are documented one-way conversions

A single-character value SHALL convert to a one-character JSON string. A byte
sequence value SHALL convert to a JSON array of integers 0-255. Both are
one-way type changes and SHALL be documented in the Language user docs.

#### Scenario: Blob becomes a byte array

- **GIVEN** a rhai `Blob` of bytes `[104, 105]` returned by a script step
- **WHEN** the engine converts it into a property
- **THEN** the property SHALL hold the JSON array `[104, 105]`

### Requirement: Absent keys and null values are distinguishable

Reading a property or header SHALL return the Language's unit/null value for
both an absent key and a stored `null`. A `has_property(name)` and a
`has_header(name)` function SHALL exist so scripts can tell the two states
apart.

#### Scenario: has_property separates absent from null

- **GIVEN** no property `p` set, and a property `q` explicitly set to `null`
- **WHEN** a script evaluates `has_property("p")` and `has_property("q")`
- **THEN** both `property("p")` and `property("q")` SHALL return the unit
  value, while `has_property("p")` SHALL return `false` and
  `has_property("q")` SHALL return `true`

### Requirement: Read-only verb results map to native targets

A read-only verb result SHALL map to its target without degradation: a
property or header target receives the JSON value as-is (an object stays an
object); a body target receives `Text` for a string result, `Empty` for a
unit/null result, and `Json` for object, array, number, or boolean results.

#### Scenario: Map result stays an object property

- **GIVEN** a `set_property` step whose rhai expression evaluates to
  `#{ "k": 42 }`
- **WHEN** the step succeeds
- **THEN** the property SHALL hold the JSON object `{"k": 42}`, not a string

### Requirement: Body exposure is native and non-destructive

The `body` variable exposed to scripts SHALL present `Text` and `Xml` bodies
as a string, `Json` bodies as the native structured value, and `Empty` as the
unit value. `Bytes` SHALL be exposed as a byte-sequence value. `Stream` SHALL
NOT be materialized implicitly: reading the stream body's VALUE through
`body` SHALL fail with a `conversion` error unless the route converts the
body first (owner decision 2026-10-02: fail loudly, no cap). A guard
failure on the registered guard surface that the script handles with
in-script error handling SHALL be suppressed per the in-script
error-handling requirement. Two outside-surface cases differ: an operation
that fails BEFORE materializing the value (indexing) is an ordinary script
error and SHALL be suppressible in-script; an operation that materializes
the value first (an unsupported operator with a marker operand) is a
boundary refusal and SHALL fail the step even when handled in-script. Static type introspection and
no-op accesses that materialize no value MAY be exempt: in rhai,
`type_of(body)` on a stream returns the static marker name `"StreamBodyRef"`,
and a bare discarded statement read `body;` is optimized away by the engine's
default optimization level — both without consuming or changing the stream.

#### Scenario: JSON body reads as a map

- **GIVEN** an exchange whose body is `Body::Json` holding `{"n": 1}`
- **WHEN** a rhai script reads `body`
- **THEN** it SHALL see a map, not an empty string, and `type_of(body)` SHALL
  be `map`

#### Scenario: Stream body fails loudly

- **GIVEN** an exchange whose body is `Body::Stream`
- **WHEN** a script reads the stream body's value through `body`
  (method call, indexing, capture into a variable, function argument, or
  result position)
- **THEN** the evaluation SHALL fail with a `conversion` error; the stream
  SHALL NOT be silently read as an empty string

#### Scenario: Stream type introspection is exempt

- **GIVEN** an exchange whose body is `Body::Stream`
- **WHEN** a rhai script evaluates `type_of(body)` without reading the value
- **THEN** the script SHALL succeed, returning the static marker name
  `"StreamBodyRef"`; the stream SHALL NOT be consumed or changed

#### Scenario: Handled stream-read guard is suppressed

- **GIVEN** an exchange whose body is `Body::Stream` and a script that wraps
  a stream-read guard failure in in-script error handling and returns a
  fallback value
- **WHEN** the step evaluates the script
- **THEN** the step SHALL succeed with the fallback value; the handled read
  SHALL NOT fail the step

#### Scenario: Handled indexing error is suppressed

- **GIVEN** an exchange whose body is `Body::Stream` and a script that wraps
  an indexing access (`body[0]`, which the engine rejects before reading the
  value) in in-script error handling and returns a fallback
- **WHEN** the step evaluates the script
- **THEN** the step SHALL succeed with the fallback value; the stream SHALL
  be unconsumed

#### Scenario: Discarded statement read is a no-op

- **GIVEN** an exchange whose body is `Body::Stream` and a rhai script whose
  only body access is a bare discarded statement expression (`body;`)
- **WHEN** the step evaluates the script on the engine's default optimization
  level
- **THEN** the step SHALL succeed; no value SHALL be materialized and the
  stream SHALL NOT be consumed

### Requirement: Mutating scripts write back the body only when assigned

A mutating script step SHALL write the body back only if the script assigned
it. Assignment SHALL be detected inside the language evaluator, before
boundary conversion. A script that only sets headers or properties SHALL leave
the body bit-identical, variant included (`Json`, `Xml`, `Bytes`, `Stream`,
`Empty`).

#### Scenario: Header-only script preserves a JSON body

- **GIVEN** an exchange with a `Body::Json` body and a `script:` step that
  only sets one header
- **WHEN** the step succeeds
- **THEN** the body SHALL still be `Body::Json` and bit-identical to the input

#### Scenario: Header-only script preserves an XML body variant

- **GIVEN** an exchange with a `Body::Xml` body and a `script:` step that
  only sets one header
- **WHEN** the step succeeds
- **THEN** the body SHALL still be `Body::Xml`; exposing it to the script as
  a string SHALL NOT change its variant

#### Scenario: Same-value assignment writes nothing

- **GIVEN** a property holding the integer `1` and a `script:` step that
  assigns the same value `1` to it again
- **WHEN** the step succeeds
- **THEN** the property SHALL keep its original stored value handle; the
  re-assignment SHALL be detected as a no-op. Assigning a value of a
  DIFFERENT type with the same numeric meaning (for example `1.0` where `1`
  was stored) SHALL be detected as a change and written back with the new
  type

### Requirement: Mutating scripts write back only changed entries

A mutating script step SHALL write back only the header and property entries
whose values the script changed. Unchanged entries SHALL keep their original
JSON value and type. Keys the script removed SHALL be removed from the
Exchange. The step SHALL validate every pending change, body included, before
committing any of them; a conversion failure SHALL leave the Exchange with no
partial mutation.

#### Scenario: Untouched map property stays an object

- **GIVEN** a property holding `{"a": [1,2]}` and a `script:` step that
  modifies a different property
- **WHEN** the step succeeds
- **THEN** the untouched property SHALL still hold the JSON object `{"a": [1,2]}`

#### Scenario: Conversion failure commits nothing

- **GIVEN** a `script:` step that changes one property to a valid value and
  another to a value the engine cannot represent
- **WHEN** the write-back conversion fails
- **THEN** neither change SHALL be applied; the Exchange SHALL keep its
  pre-step state

### Requirement: Read-only expressions cannot mutate the Exchange

In read-only evaluation modes, Exchange-mutating script calls (`set_header()`,
`set_property()`) SHALL be rejected at expression compilation time with an
error that points the author to the mutating verb (`script:`). The rejection
is immediate: no deprecation period (owner decision 2026-10-02).

#### Scenario: set_property in a read-only expression is a compile error

- **GIVEN** a `set_property` step whose rhai expression contains
  `set_property("k", 1)`
- **WHEN** the route is added
- **THEN** route addition SHALL fail with a compile error naming the call and
  pointing to `script:`

### Requirement: Read surface parity across evaluation modes

Read-only and mutating evaluation SHALL expose the same read surface: `body`,
`headers`, and `properties` variables, plus `header()`, `property()`,
`has_header()`, and `has_property()` functions.

#### Scenario: property() works in a filter predicate

- **GIVEN** a route whose `filter` predicate reads `property("level")`
- **WHEN** the predicate evaluates
- **THEN** the read SHALL succeed with the same value a `script:` step would
  see

### Requirement: Predicate results are strictly boolean

A predicate result SHALL be a boolean. A non-boolean result SHALL be a
`type-mismatch` error. Truthiness coercion SHALL NOT be applied (owner
decision 2026-10-02: strict, no staging).

#### Scenario: Non-boolean predicate result fails

- **GIVEN** a `filter` predicate whose script returns the string `"false"`
- **WHEN** the predicate evaluates
- **THEN** the step SHALL fail with a `type-mismatch` error; the string SHALL
  NOT be coerced to a boolean

### Requirement: Predicate errors propagate

A predicate evaluation error SHALL propagate per the error requirements of
this capability for `filter`, `choice/when`, `loop while`, `validate`, catch
`when`/`on_when`, and finally `on_when`. For catch `when`, the predicate error
SHALL replace the original error, and the original error SHALL be preserved as
its cause.

#### Scenario: catch-when failure chains the cause

- **GIVEN** a `do_try` block that fails with error A, and a `catch` clause
  whose `when` predicate itself fails
- **WHEN** the clause evaluates
- **THEN** the resulting route error SHALL be the predicate error, and error A
  SHALL be retrievable as its cause

### Requirement: Evaluation failure metrics

Each language evaluation failure SHALL increment a metric labeled with at
least `{language, route, verb, class}`, per the project's metric label rules.

#### Scenario: Metric records class and verb

- **GIVEN** a route whose `set_property` rhai expression fails with class
  `arithmetic`
- **WHEN** the step fails
- **THEN** the evaluation-failure metric SHALL increment with labels language
  `rhai`, the route id, verb `set_property`, class `arithmetic`

### Requirement: Evaluation failure span attributes

The failing step's span SHALL record the error class and the script position
as span attributes.

#### Scenario: Span carries class and position

- **GIVEN** tracing enabled and a step whose expression fails at line 3,
  column 8
- **WHEN** the step fails
- **THEN** the step span SHALL carry the class and position `3:8` as
  attributes

### Requirement: camel test shows typed evaluation errors

`camel test` output for a failed exchange SHALL show the full typed evaluation
error: class, position, and step identification, not a generic failure line.

#### Scenario: Test output names the failing expression

- **GIVEN** a test route whose `filter` expression fails at a known position
- **WHEN** `camel test` runs the route
- **THEN** the failure output SHALL include the class, the position, and the
  step id

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

