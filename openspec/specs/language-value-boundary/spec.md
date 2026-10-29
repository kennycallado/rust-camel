# language-value-boundary Specification

## Purpose
The boundary between script languages and the engine value model. Evaluation
failures become typed, matchable route exceptions without exchange data;
structured values round-trip natively; mutating scripts write back only what
they changed; and Rhai JSON helpers follow strict RFC 8259 rules.
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
`timeout`, `conversion`, `parse`. The xpath language SHALL classify its
failure paths as: input-size failure → `limit`, body coercion failure →
`conversion` (the `Conversion` class covers result conversion and input-body
coercion), malformed XML body → `parse` (the `Parse` class covers expression
and document sources), genuine evaluation failure → `runtime`.
When a language error carries a contractually redacted payload
(`EvalFailure.detail`, or the type-name-only `TypeMismatch`/`ConversionError`
payloads), `to_expression_failed` SHALL carry that fixed detail as the
mapped `ExpressionFailed`'s `cause` instead of `None`, wrapped in a
`ProcessorErrorWithSource` carrier with explicit mapper provenance (a
private marker behind its `#[source]`), so the cause renders as
`Processor error: <detail>`. Mapper provenance SHALL be identifiable
exactly — by marker downcast (`is_expression_detail_cause`), never by
variant or string shape.
Free-form payloads (`EvalError`, `ParseError` reason) SHALL keep
`cause: None`. Newly generated language diagnostics SHALL never place the
raw body or provider error text into the cause; an original route error
chained by catch handling keeps its own provenance and is not re-sanitized.
`chain_predicate_error` SHALL replace the cause with the original route
error when it is absent or mapper-generated; any caller-supplied cause
chain arriving through `PredicateSource::Async` SHALL pass through
untouched.

#### Scenario: Error content is complete

- **GIVEN** a route `r1` whose step `s2` evaluates a rhai expression that
  raises `ErrorArithmetic` at a known position
- **WHEN** the step fails
- **THEN** the route error SHALL identify language `rhai`, route `r1`, step
  `s2` with its verb, the position, and class `arithmetic`

#### Scenario: xpath size failure classifies as limit with a fixed cause

- **GIVEN** an xpath expression with `max_input_bytes` configured below the
  body size
- **WHEN** the step evaluates against an oversized well-formed XML body
- **THEN** the mapped `ExpressionFailed` SHALL carry class `limit`, and its
  cause payload SHALL be exactly `input exceeds maximum allowed size`
  (rendered through the carrier as `Processor error: input exceeds maximum
  allowed size`)

#### Scenario: xpath body coercion classifies as conversion

- **GIVEN** an xpath expression and a body that is neither XML nor coercible
  to XML
- **WHEN** the step evaluates
- **THEN** the mapped `ExpressionFailed` SHALL carry class `conversion` and
  the fixed cause payload `body is not XML and cannot be coerced`, with no
  provider error text appended

#### Scenario: malformed XML is distinguishable from a size failure without logs

- **GIVEN** a configured xpath input limit `L` near 100 KB, a well-formed
  XML body of length greater than `L`, and a malformed body of length less
  than `L` (both roughly 100 KB)
- **WHEN** the same xpath configuration evaluates both bodies
- **THEN** the size-rejected failure SHALL carry class `limit` with cause
  payload `input exceeds maximum allowed size`, the malformed failure SHALL
  carry class `parse` with cause payload `body is not valid XML`, and the
  two SHALL be tellable apart by class and cause without enabling `RUST_LOG`

#### Scenario: genuine evaluation failure keeps the runtime class

- **GIVEN** an xpath expression that compiles but fails during sxd evaluation
- **WHEN** the step evaluates against a valid XML body
- **THEN** the mapped `ExpressionFailed` SHALL keep class `runtime`

#### Scenario: free-form error text keeps an empty cause

- **GIVEN** a language failure carried as a free-form `EvalError` payload
- **WHEN** `to_expression_failed` maps it
- **THEN** the `cause` SHALL remain `None`

#### Scenario: redacted body content never enters the cause

- **GIVEN** a body containing a sentinel string and any reclassified xpath
  failure path triggered on that body
- **WHEN** the mapped `ExpressionFailed` cause is rendered, including through
  `Error::source()` traversal
- **THEN** the sentinel SHALL NOT appear; only the fixed class payloads SHALL

#### Scenario: catch-predicate failure still chains the original route error

- **GIVEN** a doTry route whose doCatch `onWhen` xpath predicate fails to
  evaluate (malformed body)
- **WHEN** the try body raises the original route error and the predicate
  evaluation fails
- **THEN** the surfaced `ExpressionFailed` SHALL carry the predicate's class
  and its `cause` chain SHALL still render the original route error,
  reachable via `Error::source()` traversal

#### Scenario: caller-supplied cause chains pass through untouched

- **GIVEN** a `PredicateSource::Async` closure that fails with an
  `ExpressionFailed` carrying its own non-synthetic cause chain — including
  a hand-built `ProcessorError` or `ProcessorErrorWithSource` without mapper
  provenance
- **WHEN** doTry chains the original route error
- **THEN** the caller-supplied cause SHALL remain unchanged

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

A mutating script step SHALL write the body back only if the post-evaluation
body is **observably changed after normalization** relative to the normalized
pre-run snapshot. The body is observably changed after normalization when the
normalized post-run body differs from the normalized pre-run snapshot, with the
same value conversion applied to both sides BEFORE converting to a `Body`; an
unchanged value that changes representation (for example JSON `10.0` surviving
as an integral number through JavaScript) SHALL therefore compare equal and
SHALL NOT be written back. For JavaScript, the snapshot SHALL be captured
inside the same worker evaluation job that runs the script, immediately before
execution, ONLY for mutating evaluation (`sentinel.is_none()`); read-only
evaluation does not capture it and sets the field to the inbound body. The
snapshot is carried on the existing `JsEvalResult`; no separate blocking job
and no new `JsEngine` trait method SHALL be added. Rhai already captures its
pre-evaluation snapshot inside its mutating evaluation (`eval_mut_sync`); this
change does not alter that mechanism. The body assign-only detection binds
every mutating language, including `rhai` and `js`. A script that only sets
headers or properties SHALL leave the body bit-identical, variant included
(`Json`, `Xml`, `Bytes`, `Stream`, `Empty`). Changed-entry parity for the
header/property MAPS under JavaScript (whole-map rewriting, `1` vs `1.0`
numeric drift in maps) is NOT fixed by this change and is explicitly deferred
to rc-zwy7e; this change fixes the JavaScript BODY only.

For JavaScript, assigning `null` to the body SHALL make it `Body::Empty`.
Because JavaScript exposes both `Bytes` and `Empty` as `null`, a `null`
assignment on a JavaScript `Bytes` or `Empty` body SHALL be a no-op (the
normalized snapshot is also null). For Rhai, `Bytes` is exposed as a Blob, so
`body = ()` on a Rhai `Bytes` body is a real assignment and SHALL make it
`Body::Empty`; a Rhai `Empty` body is already unit, so `body = ()` SHALL leave
it `Body::Empty`.

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

#### Scenario: JS header-only script preserves an XML body

- **GIVEN** an exchange with a `Body::Xml` body and a `js` `script:` step that
  only sets one header
- **WHEN** the step succeeds
- **THEN** the body SHALL still be `Body::Xml`, so a later HTTP reply keeps its
  `application/xml` content type

#### Scenario: JS header-only script preserves an integral-float JSON body

- **GIVEN** an exchange with a `Body::Json` body containing the number `10.0`
  and a `js` `script:` step that only sets one header
- **WHEN** the step succeeds
- **THEN** the body SHALL still be `Body::Json` with the original `10.0` value

#### Scenario: Same-value assignment writes nothing

- **GIVEN** a property holding the integer `1` and a **rhai** `script:` step
  that assigns the same value `1` to it again
- **WHEN** the step succeeds
- **THEN** the property SHALL keep its original stored value handle; the
  re-assignment SHALL be detected as a no-op. Assigning a value of a
  DIFFERENT type with the same numeric meaning (for example `1.0` where `1`
  was stored) SHALL be detected as a change and written back with the new
  type. (JavaScript header/property map parity is deferred to rc-zwy7e.)

#### Scenario: JS null body assignment yields Empty

- **GIVEN** an exchange with a `Body::Text` body and a `js` `script:` step
  that assigns `camel.body = null`
- **WHEN** the step succeeds
- **THEN** the body SHALL be `Body::Empty`, not `Body::Json(Null)`

#### Scenario: JS null on Bytes or Empty is a no-op

- **GIVEN** an exchange with a `Body::Bytes` body and a `js` `script:` step
  that assigns `camel.body = null`
- **WHEN** the step succeeds
- **THEN** the body SHALL remain `Body::Bytes`; a null assignment on a JS
  `Body::Empty` body SHALL likewise remain `Body::Empty` (JS exposes both as
  `null`)

#### Scenario: Rhai unit assignment on Bytes is a change

- **GIVEN** an exchange with a `Body::Bytes` body and a rhai `script:` step
  that assigns `body = ()`
- **WHEN** the step succeeds
- **THEN** the body SHALL become `Body::Empty` (Rhai exposes `Bytes` as a Blob,
  so the unit assignment is a real change)

#### Scenario: Rhai unit assignment on Empty is unchanged

- **GIVEN** an exchange with a `Body::Empty` body and a rhai `script:` step
  that assigns `body = ()`
- **WHEN** the step succeeds
- **THEN** the body SHALL remain `Body::Empty`

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

In Rhai read-only expressions and predicates, Exchange-mutating script calls
(`set_header()`, `set_property()`) and any assignment or update to a root
variable named `body`, `headers`, or `properties` (direct, dotted,
computed/index, or nested) SHALL be rejected at expression compilation time.
The check is **conservative**: a local variable that shadows one of those names
SHALL also be rejected. A local JavaScript variable named `body`/`headers`/
`properties` is NOT rejected; JavaScript's compile-time walk covers only the
statically-decidable `camel.*` mutation surface
(`camel-language-js/src/expression.rs::reject_read_only_mutation`: `camel.body =
...` including compound assignment and update, `delete camel.body`,
`camel.set_property(...)`, `camel.headers.set/remove`,
`camel.properties.set/remove`, static computed members, and destructuring
targets) and is unchanged by this change. JavaScript's UNDECIDABLE dynamic
forms (`camel[expr]`, an aliased method reference, host-style
`Object.defineProperty` / `Reflect.set`) stay refused at RUNTIME by the
read-only snapshot's throwing traps, never silently discarded. This change adds
the **root-name assignment/update walk to Rhai**; Rhai has no runtime snapshot
layer, so the walk is its only rejection mechanism for those forms. Rhai
method-call mutations on those roots (`headers.clear()`, `headers.remove(x)`)
are a KNOWN REMAINING GAP (rc-qpjps) and are not covered by this change. The
Rhai error SHALL point the author to a mutating surface that exists: a
`script:` step assigning `headers["k"] = value`, `properties["k"] = value`, or
`body = value`. The rejection is immediate: no deprecation period (owner
decision 2026-10-02).

#### Scenario: set_property in a read-only expression is a compile error

- **GIVEN** a `set_property` step whose rhai expression contains
  `set_property("k", 1)`
- **WHEN** the route is added
- **THEN** route addition SHALL fail with a compile error naming the call and
  pointing to the mutating map-assignment surface

#### Scenario: index assignment to headers is a compile error

- **GIVEN** a `set_property` step whose rhai expression contains
  `headers["x"] = "1"`
- **WHEN** the route is added
- **THEN** route addition SHALL fail with a compile error; the write SHALL NOT
  be silently discarded

#### Scenario: body assignment is a compile error

- **GIVEN** a `set_property` step whose rhai expression contains
  `body = "changed"`
- **WHEN** the route is added
- **THEN** route addition SHALL fail with a compile error

#### Scenario: nested computed assignment is a compile error

- **GIVEN** a `set_property` step whose rhai expression contains
  `properties["a"]["b"] = "c"`
- **WHEN** the route is added
- **THEN** route addition SHALL fail with a compile error

#### Scenario: compound update is a compile error

- **GIVEN** a `set_property` step whose rhai expression contains
  `headers["x"] += 1`
- **WHEN** the route is added
- **THEN** route addition SHALL fail with a compile error

#### Scenario: a shadowing local name is rejected conservatively

- **GIVEN** a rhai expression `let headers = #{}; headers["x"] = 1`
- **WHEN** the expression is created
- **THEN** creation SHALL fail with the same compile error (documented
  conservative false-positive)

### Requirement: Read surface parity across evaluation modes

Read-only and mutating evaluation SHALL expose the same read surface: `body`,
`headers`, and `properties` variables, plus `header()`, `property()`,
`has_header()`, and `has_property()` functions. The Rhai read-only scope SHALL
include the `properties` variable, and the Rhai script-step (mutating) engine
SHALL register `header()` and `property()` backed by the pre-evaluation
snapshot maps. The `has_header()`/`has_property()` functions named in this
requirement are explicitly deferred to rc-w1mds; this change does not implement
them and SHALL NOT be read as making them pass.

#### Scenario: property() works in a filter predicate

- **GIVEN** a route whose `filter` predicate reads `property("level")`
- **WHEN** the predicate evaluates
- **THEN** the read SHALL succeed with the same value a `script:` step would
  see

#### Scenario: property() works in a rhai script step

- **GIVEN** a rhai `script:` step whose source is `property("level")`
- **WHEN** the step evaluates
- **THEN** the read SHALL succeed with the same value the read-only predicate
  would see

#### Scenario: properties map is readable in a rhai read-only expression

- **GIVEN** a rhai expression `properties["level"]` and an exchange with
  property `level` set
- **WHEN** the expression evaluates
- **THEN** the read SHALL succeed with the property value

#### Scenario: rhai script readers see the pre-run snapshot

- **GIVEN** a rhai `script:` step with source
  `headers["k"] = "new"; header("k")` and an exchange whose header `k` is
  `"old"`
- **WHEN** the step evaluates
- **THEN** `header("k")` SHALL return `"old"` (the pre-evaluation snapshot),
  while the assigned map write is still propagated; this differs from
  JavaScript, whose `camel.headers.get("k")` observes the live mutated map

#### Scenario: JS readers see live mutations

- **GIVEN** a js `script:` step with source
  `camel.headers.set("k", "new"); camel.headers.get("k")` and an exchange whose
  header `k` is `"old"`
- **WHEN** the step evaluates
- **THEN** `camel.headers.get("k")` SHALL return `"new"` (live behavior,
  unchanged)

### Requirement: Predicate results are strictly boolean

A predicate result SHALL be a boolean. A non-boolean result SHALL be a
`type-mismatch` error. Truthiness coercion SHALL NOT be applied (owner
decision 2026-10-02: strict, no staging).

JSONPath predicates SHALL additionally support an explicit `predicate_mode`
with values `strict` and `matches`; `strict` SHALL be the default and SHALL
behave exactly as above. In `matches` mode, evaluation SHALL inspect the raw
match sequence before value-source normalization: zero matches (including a
missing path) SHALL produce `false`; exactly one boolean match SHALL produce
that boolean (a literal `false` SHALL stay `false`); every other non-empty
match sequence SHALL produce `true`. `matches` mode SHALL NOT import XPath
`boolean()` value rules: an empty string and `0` SHALL produce `true`.
JSONPath value-source (expression) typing SHALL remain unchanged in both
modes, and XPath behavior SHALL remain unchanged. Evaluation failures SHALL
remain typed errors in every mode and SHALL NOT be coerced to `false`.
Error messages SHALL name the expression and the value kind only, never
payload values.

#### Scenario: Non-boolean predicate result fails

- **GIVEN** a `filter` predicate whose script returns the string `"false"`
- **WHEN** the predicate evaluates
- **THEN** the step SHALL fail with a `type-mismatch` error; the string SHALL
  NOT be coerced to a boolean

#### Scenario: Explicit JSONPath matches mode coerces by cardinality

- **GIVEN** a JSONPath predicate created with `predicate_mode: matches`
- **WHEN** it evaluates a query returning, in turn: an object match, a
  multi-match sequence, a single empty string, the string `"false"`, the
  number `0`, an existing `null` value, a missing path, and a literal
  `false` boolean
- **THEN** the result SHALL be `true` for every non-empty match sequence
  (including the empty string, `"false"`, `0`, and the existing `null`),
  `false` for the missing path, and `false` for the sole literal `false`
  boolean

#### Scenario: Matches mode parity with XPath boolean()

- **GIVEN** equivalent JSON and XML fixtures with zero, one, and multiple
  item matches
- **WHEN** a `matches`-mode JSONPath predicate and an XPath
  `boolean(/order/items/item[price>10])` predicate evaluate the same fixture
  set
- **THEN** both SHALL return the same boolean for every fixture, including
  a sole `false` boolean equal to `boolean(false())`

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

### Requirement: Simple template expressions preserve literal text

`SimpleLanguage::create_expression` SHALL parse its source in TEMPLATE mode.
In template mode the source is literal text plus `${...}` interpolation slots
only. Characters and words outside a slot — including `=`, `<`, `>`, `!`,
`&&`, `||`, `contains`, `null`, `true`, `false`, single and double quotes, and
whitespace — SHALL be preserved verbatim and SHALL NOT be re-parsed as
predicate grammar. The whole source SHALL be outer-trimmed first so a YAML
block scalar's trailing newline is absorbed; whitespace inside the source SHALL
be preserved. An empty (after trim) source SHALL be a parse error. The
unclosed-`${` rule takes precedence over the raw-literal rule: a source
containing an unclosed `${` is governed by the unclosed rule, not classified
as a raw literal. An unclosed `${` SHALL always be a parse error: creation
SHALL fail with a message naming the construct (`unclosed interpolation`) and
the byte offset of the `${` in the trimmed source. An empty (or
whitespace-only) slot (`${}`) SHALL be a parse error naming the empty
interpolation. A `${` inside slot content (nested interpolation) SHALL be a
parse error. Literal `$`, `{`, and `}` outside a `${` remain verbatim text.
"Simple
template mode" is Simple's own literal-plus-`${}` grammar; it is NOT the
MiniJinja template engine (ADR-0047) and NOT RouteTemplate JSON-tree
substitution (ADR-0008).

#### Scenario: Operator-looking text stays literal

- **GIVEN** an expression created from `Got -> ${header.h}` with header `h`
  equal to `B`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the string `Got -> B`; no numeric comparison
  SHALL be attempted

#### Scenario: Keyword-looking text stays literal

- **GIVEN** an expression created from `value is null: ${header.h}` with
  header `h` equal to `B`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the string `value is null: B`, preserving the
  word `null`

#### Scenario: Whitespace and quoting in literal text are preserved

- **GIVEN** an expression created from `${header.h} 3 items` with header `h`
  equal to `B`, and an expression created from `literal 'quoted' text`
- **WHEN** each expression is evaluated
- **THEN** the results SHALL be `B 3 items` and `literal 'quoted' text`
  respectively

#### Scenario: Angle-bracket literals do not fail

- **GIVEN** an expression created from `<order>${header.h}</order>` with header
  `h` equal to `B`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `<order>B</order>` and no parse error SHALL be
  raised

#### Scenario: Outer trim absorbs a block-final newline

- **GIVEN** an expression source `"Got ${header.h}\n"` with header `h` equal to
  `B`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `Got B`, with no trailing newline, and the
  internal space of `Got ` SHALL be preserved

#### Scenario: Empty template is a parse error

- **GIVEN** an expression source `""` or `"   "`
- **WHEN** the expression is created
- **THEN** creation SHALL fail with a parse error

<!-- openspec-scenario-ops
renamed: Unclosed slot after a valid slot is a parse error -> Unclosed slot is always a parse error
-->

#### Scenario: Unclosed slot is always a parse error

- **GIVEN** expression sources `Got ${body`, `${header.h} ${`, and `${`
- **WHEN** each expression is created
- **THEN** creation SHALL fail with a parse error whose message names
  `unclosed interpolation` and the byte offset of the offending `${` in the
  trimmed source (4, 12, and 0 respectively); an unclosed `${` SHALL NOT be
  treated as literal text in any position

#### Scenario: Empty slot is a parse error

- **GIVEN** expression sources `${}` and `${   }`
- **WHEN** each expression is created
- **THEN** creation SHALL fail with a parse error whose message names the
  empty interpolation

#### Scenario: Nested interpolation is a parse error

- **GIVEN** expression sources `${${header.a}}` and `${header.a${b}`
- **WHEN** each expression is created
- **THEN** creation SHALL fail with a parse error

### Requirement: Simple expressions treat whole literals as raw text

An expression source with no `${...}` slot SHALL evaluate to the raw source
text. Quoting, boolean words, numbers, and `null` SHALL NOT be interpreted as
literals of another type. This is a deliberate value-expression semantic; the
predicate grammar is unchanged. `$$` is NOT a Simple-template escape:
At the Simple source level, after substitution, `$${header.h}` is a literal
`$` followed by the live `${header.h}` slot, not a literal `${header.h}`.
In a route file, the env pass first turns `$$` into `$`: `$${header.h}`
reaches Simple as the whole slot `${header.h}` and retains its native value.
To render `$` plus a slot from a route file, write `$$$${header.h}`.
A literal `${...}` is produced only through a path that
does not run the template gate — a nested literal (not gated) or an explicit
script string.

#### Scenario: Quoted text keeps its quotes

- **GIVEN** an expression created from `'abc'`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the string `'abc'`, quotes included

#### Scenario: Boolean, number, and null words are raw text

- **GIVEN** expressions created from `true`, `42`, and `null`
- **WHEN** each is evaluated
- **THEN** the results SHALL be the strings `true`, `42`, and `null`
  respectively — not `Value::Bool`, `Value::Number`, or `Value::Null`

#### Scenario: Escapes are raw text

- **GIVEN** an expression created from `"a\n"`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the five-character source text `"a\n"`, not a
  string containing a newline

### Requirement: Whole-slot expressions return the slot value

When the outer-trimmed source is exactly one `${...}` slot and nothing else,
the expression SHALL evaluate to the slot's value using the EXISTING Simple
evaluator semantics for that atom, with no added stringification. In
particular, `${body}` on a `Body::Json` body SHALL return the serialized JSON
string (existing `evaluator.rs` behavior for `Expr::Body`), while a body-field
slot `${body.items}` SHALL return the native JSON array (existing
`Expr::BodyField` behavior). A missing/absent slot value SHALL render as
`Value::Null`. In a mixed template (literal text and/or multiple slots), a
missing slot SHALL render as the empty string instead.

#### Scenario: Whole header slot returns its native value

- **GIVEN** an expression `${header.n}` and header `n` holding the JSON number
  `7`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `Value::Number` `7`, not the string `7`

#### Scenario: Whole body slot keeps existing stringification

- **GIVEN** an expression `${body}` and a `Body::Json` body `{"a":1}`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the serialized JSON string `{"a":1}` (existing
  `Expr::Body` semantics), not a native object

#### Scenario: Whole body-field slot keeps an array

- **GIVEN** an expression `${body.items}` and a JSON body `{"items":[1,2]}`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the JSON array `[1,2]`, not a string

#### Scenario: Whole missing slot returns Null

- **GIVEN** an expression `${header.missing}` with no such header
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `Value::Null`

#### Scenario: Mixed missing slot renders empty

- **GIVEN** an expression `prefix-${header.missing}-suffix`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the string `prefix--suffix`

### Requirement: Bare top-level string value sources interpolate consistently

In a declarative route, a bare **top-level** string value source that contains a
`${...}` slot SHALL be compiled as a Simple template expression, whichever verb
carries it (`log:`, `set_body:`, `set_header.value`,
`set_header_if_absent.value`, `set_property.value`, `transform:`). A `${...}`
in such a bare string SHALL NOT reach the Exchange as
literal text. `set_header_if_absent` shares `SetHeaderStep` and
`parse_value_source` but is produced by REST lowering, not authored directly
(`#[serde(skip_deserializing)]`); its `value` is gated through the same helper. This covers both the bare scalar form
(`set_body: "Got ${header.h}"`) and the object `value:` form
(`set_body: {value: "Got ${header.h}"}`); both SHALL compile through one shared
helper, used by the bare body and transform arms and by `parse_value_source`
for the header/property/log value forms. A top-level bare string without a
`${...}` slot SHALL remain a literal value. A string **nested inside an object
or array literal** SHALL remain literal, gate or no gate. The same helper SHALL
serve both the YAML and JSON route compilers, and `log:` SHALL keep compiling
its bare string to a Simple expression.

#### Scenario: bare log interpolates

- **GIVEN** a step `log: "Got ${header.h}"` with header `h` equal to `B`
- **WHEN** the route is compiled and the step runs
- **THEN** the message SHALL be `Got B`

#### Scenario: bare set_body interpolates

- **GIVEN** a step `set_body: "Got ${header.h}"` with header `h` equal to `B`
- **WHEN** the route is compiled and the step runs
- **THEN** the body SHALL be the text `Got B`

#### Scenario: object value form interpolates

- **GIVEN** a step `set_body: {value: "Got ${header.h}"}` with header `h`
  equal to `B`
- **WHEN** the route is compiled and the step runs
- **THEN** the body SHALL be the text `Got B`, compiled through the same
  shared helper as the bare scalar form

#### Scenario: bare set_header value interpolates

- **GIVEN** `set_header: {key: x, value: "Got ${header.h}"}` with header `h`
  equal to `B` and no explicit `simple:`
- **WHEN** the route is compiled and the step runs
- **THEN** header `x` SHALL be the string `Got B`

#### Scenario: plain literal is unchanged

- **GIVEN** a step `set_body: "order"` with no `${...}`
- **WHEN** the route is compiled
- **THEN** the value source SHALL remain a literal string `order`

#### Scenario: a formerly-literal template-looking string now fails to compile

- **GIVEN** a step `set_body: "echo ${HOME}"` (or a JS template payload such as
  `` set_body: "`${name}`" ``) intended as a literal string
- **WHEN** the route is compiled
- **THEN** compilation SHALL fail with an `unrecognized token` parse error
  (the top-level string is now gated and `${HOME}` is not a valid Simple atom).
  This is a BREAKING change; the commit carries `!:` or `BREAKING CHANGE:`

#### Scenario: nested literal is not gated

- **GIVEN** a step `set_body: {value: {nested: "${header.h}"}}` (or an
  equivalent JSON object literal)
- **WHEN** the route is compiled
- **THEN** the nested string SHALL remain the literal `"${header.h}"`; only a
  top-level string SHALL be gated

#### Scenario: JSON and YAML agree

- **GIVEN** the same route authored once in YAML and once in JSON, each with a
  bare top-level `set_body: "Got ${header.h}"`
- **WHEN** both are compiled to declarative routes
- **THEN** both SHALL produce a Simple template expression with the same source

### Requirement: Environment and job-argument substitutions are trusted

Route-source substitution SHALL handle `${env:NAME}` ONLY and SHALL fail
closed for an unescaped `${arg:NAME}`. `${arg:NAME}` is NOT substituted on the
route path: the tree walk passes an empty argument lookup and fails closed
(`camel-dsl/src/env_interpolation.rs`,
`interpolate_env_tree_with_provenance` calls `interpolate_value(..., &|_| None,
...)`), so an unescaped `${arg:NAME}` in a route source SHALL fail to load. Job
arguments SHALL be substituted only on the `camel-cli` job execution/send path
(`crates/camel-cli/src/commands/job/document.rs`, `interpolate_with_args`).
Substituted text is trusted operator configuration (ADR-0032); a `${...}` token
it introduces MAY be interpreted as a template slot. The `$${...}` escape form
(`$${env:X}` / `$${arg:X}`) emits the literal text `${...}`, which the template
gate then treats as a `lang:` slot; preserving a substituted literal `${...}`
is deferred to rc-luew6.

#### Scenario: env value equal to a slot becomes a slot

- **GIVEN** an environment variable `NAME` whose value is `${header.h}`, and a
  step `set_body: "Hello ${env:NAME}"` with header `h` equal to `B`
- **WHEN** the route is compiled and the step runs
- **THEN** the compiled value source SHALL be a Simple template
  `Hello ${header.h}` and the body SHALL be `Hello B`

#### Scenario: plain env value stays literal

- **GIVEN** an environment variable `NAME` whose value is `B`, and a step
  `set_body: "Hello ${env:NAME}"`
- **WHEN** the route is compiled
- **THEN** the compiled value source SHALL be the literal string `Hello B`
  (no `${` remains, so no template)

#### Scenario: unescaped route arg fails to load

- **GIVEN** a route source containing `set_body: "Hello ${arg:NAME}"`
- **WHEN** the route is loaded
- **THEN** loading SHALL fail with an unresolved `NAME` error; `${arg:NAME}` is
  substituted only on the `camel-cli` job path

#### Scenario: escaped env literal becomes a slot and fails at runtime

- **GIVEN** a route source containing `set_body: "$${env:NAME}"` with `NAME`
  set to `B`
- **WHEN** the route is compiled and the step runs
- **THEN** the escape SHALL yield the literal text `${env:NAME}`, the template
  gate SHALL treat it as a `lang:expr` slot, the route SHALL compile, and at
  runtime the step SHALL fail with `Language 'env' not found` (no literal
  `${...}` path until rc-luew6)

### Requirement: Boolean language conditions use predicate evaluation

A language completion condition in the aggregator SHALL compile through the
language `Predicate` contract and evaluate with `Predicate::matches`, so a
non-boolean script result SHALL be a type error and every evaluation error
SHALL propagate. An `Expression` result SHALL NOT be coerced with `as_bool`.

#### Scenario: completion predicate is strict

- **GIVEN** an aggregator `PredicateExpr` whose Simple source is
  `${body} == 'DONE'`
- **WHEN** the predicate evaluates against an exchange
- **THEN** it SHALL compile as a predicate and match only when the comparison
  is true

#### Scenario: non-boolean predicate result errors

- **GIVEN** an aggregator `PredicateExpr` whose source evaluates to a string
- **WHEN** the predicate evaluates
- **THEN** the step SHALL return `Err(CamelError::ProcessorError)` carrying the
  language's type-mismatch text, NOT `Ok(false)`. The aggregator has no
  `EvalMeta`, so the error is a `ProcessorError`, not an
  `ExpressionFailed`/`ExpressionErrorClass` variant (pre-existing variant gap,
  filed as a follow-up)

### Requirement: Rhai string methods have explicit purity semantics

The Rhai sandbox SHALL register a pure `trim()` that returns a string,
shadowing the `StandardPackage` in-place method that mutates its subject and
returns unit. No `_inplace` alias SHALL be added. The remaining
`StandardPackage` string methods that mutate in place and return unit SHALL be
documented with their pure alternatives. The shadow SHALL apply to the
read-only and mutating engines alike.

#### Scenario: trim returns a string

- **GIVEN** a rhai expression `let x = "  a  "; x.trim() == "a"`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `true`

#### Scenario: blank check is correct

- **GIVEN** a rhai expression `let x = "   "; x.trim() == ""`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `true`, and `let x = "   "; x.trim() != ""`
  SHALL be `false`

#### Scenario: shadow applies in a script step

- **GIVEN** a rhai `script:` step whose source is
  `let x = "  a  "; x.trim() == "a"`
- **WHEN** the step evaluates
- **THEN** the result SHALL be boolean `true`

### Requirement: Rhai JSON helpers shadow the stock built-ins from a shared module

The Rhai sandbox SHALL build one process-wide host module containing the JSON
wrapper type, functions, and indexers, and SHALL register it globally on every
engine it builds after the standard package, so its functions take precedence
over the engine's stock `parse_json` and `to_json`. The host functions SHALL
read `max-string-size`, `max-array-size`, and `max-map-size` from the calling
engine, not from captured state.

#### Scenario: Host helpers win on every engine

- **GIVEN** an engine built by the rhai sandbox
- **WHEN** a script calls `parse_json` or `to_json`
- **THEN** the host implementation SHALL run, not the stock built-in, on the
  read-only, mutating, and expression engines alike

### Requirement: Rhai JSON parsing is strict RFC 8259

`parse_json` SHALL accept the full RFC 8259 grammar for any JSON value (object,
array, or scalar), including the `\/` escape, EXCEPT that an unpaired surrogate
escape SHALL be refused. Malformed input, including unpaired surrogates, SHALL
fail with a typed `parse` error that contains no parsed input text and no
parser line or column; the error SHALL carry the script call position instead.

#### Scenario: Escaped solidus is accepted

- **GIVEN** the JSON text `{"path":"a\/b"}`
- **WHEN** a script evaluates `parse_json` on it
- **THEN** parsing SHALL succeed and the `path` value SHALL be `a/b`

#### Scenario: Unpaired surrogate is refused

- **GIVEN** JSON text containing the escape `"\uD800"` with no low surrogate
- **WHEN** a script calls `parse_json`
- **THEN** the step SHALL fail with a `parse` error naming the script call
  position, with no input text and no parser line or column

### Requirement: Rhai JSON number projection is integer-form only

`parse_json` SHALL store each number as its exact source token and SHALL NOT
convert any number to `f64`. A token SHALL project to a native Rhai `INT` only
when it is an integer form with no fraction and no exponent and its
`parse::<i64>()` fits; `-0` MAY normalize. Every other token, including any
decimal or exponent form and any integer outside the `i64` range, SHALL be a
`JsonNumber` holding the raw token. The only conversion SHALL be an explicit
`JsonNumber.to_float()`, which returns a finite `f64` or an `arithmetic` error.

#### Scenario: Integer forms project, decimals and exponents wrap

- **GIVEN** the JSON numbers `9223372036854775807`, `-0`, `1.0`, `1e2`, and
  `18446744073709551615`
- **WHEN** a script reads them after `parse_json`
- **THEN** `9223372036854775807` (and the normalized `-0`) SHALL be native
  integers, while `1.0`, `1e2`, and `18446744073709551615` SHALL each be a
  `JsonNumber`

#### Scenario: Large magnitude round-trips exactly

- **GIVEN** the JSON numbers `18446744073709551615`,
  `123456789012345678901234567890`, and `1e400`
- **WHEN** a script converts `parse_json` output back with `to_json`
- **THEN** each number SHALL re-emit with its exact source token and no `f64`
  rounding

#### Scenario: to_float is explicit and checked

- **GIVEN** the `JsonNumber` for `1e400`
- **WHEN** a script calls `to_float()` on it
- **THEN** the step SHALL fail with an `arithmetic` error because the result is
  not finite

### Requirement: Rhai JSON objects preserve insertion order and duplicate semantics

`parse_json` SHALL retain object keys in authored insertion order in an
`IndexMap`. A JSON duplicate key SHALL take the last value while keeping the
position of its first occurrence. Replacing the value at an existing key SHALL
retain that key's position. `remove` SHALL use shift semantics (remaining keys
keep their relative order) and re-inserting a removed key SHALL place it at the
end. `to_json` SHALL emit keys in stored order.

#### Scenario: Authored order is retained

- **GIVEN** the JSON text `{"z":1,"a":2,"m":3}`
- **WHEN** a script re-emits it with `to_json`
- **THEN** the keys SHALL appear in the order `z`, `a`, `m`

#### Scenario: Duplicate key keeps first position, last value

- **GIVEN** the JSON text `{"b":1,"a":2,"b":3}`
- **WHEN** a script re-emits it with `to_json`
- **THEN** the output SHALL be `{"b":3,"a":2}` in that key order

#### Scenario: Removing a keeps the order of the rest

- **GIVEN** a parsed object `{"a":1,"b":2,"c":3}`
- **WHEN** a script removes `a` and re-emits the object
- **THEN** the output SHALL contain `b` then `c` in that order

### Requirement: Rhai parsed JSON uses an Arc copy-on-write wrapper with a minimal surface

`parse_json` SHALL use the same projection at the root and on indexed reads:
objects/arrays become `JsonValue`, strings/bools/integer-form i64 become native,
other numbers become `JsonNumber`, and null becomes unit. Containers are backed
by an `Arc` tree. Reading a wrapper SHALL clone the `Arc`
(O(1)); mutating a value reached by a bare read SHALL NOT affect its source.
The wrapper SHALL expose only: string-key and integer-index get and set
(including chained assignment), dot-property fallback, `len`, `contains`,
`keys`, `remove`, and `push`. Public `type_of` names SHALL be exactly
`json value` and `json number`; outbound errors SHALL use these stable labels,
never Rust type paths.

#### Scenario: Bare read is isolated

- **GIVEN** a parsed object `{"a":{"b":1}}`
- **WHEN** a script evaluates `let x = j["a"]; x["b"] = 9;` and then
  `to_json(j)`
- **THEN** the output SHALL still contain `"b":1`

#### Scenario: type_of uses stable labels

- **GIVEN** a parsed object and a parsed `JsonNumber`
- **WHEN** a script evaluates `type_of` on each
- **THEN** the results SHALL be `json value` and `json number` respectively

### Requirement: Rhai JSON wrapper indexing, assignment, and mutation semantics

The wrapper SHALL use one projection everywhere: object or array SHALL read as a
`JsonValue`; string, bool, and unit SHALL read natively; an `i64` number SHALL
read as native `INT`; any other number SHALL read as `JsonNumber`. A missing key
and a JSON `null` SHALL both read as unit; `contains` and the `in` operator
SHALL distinguish a present key from an absent one. `contains` SHALL accept a
string key on an object; calling it on an array SHALL fail with `type-mismatch`.
Dot access on an object SHALL fall back to string-key indexing. A negative array
index SHALL count from the end; an out-of-range index SHALL raise
`ErrorArrayBounds`. `push` SHALL only append and SHALL recheck the array cap.
`remove` SHALL return the removed value, or unit when the key or index is
absent. Assigning unit SHALL store JSON `null`.

Accepting setters SHALL be: unit, bool, `i64`, finite `f64`, `String`,
`JsonNumber`, `JsonValue`, native `Map`, and native `Array`. Index and helper
setters SHALL be registered as single `Dynamic`-accepting functions that
validate the value type internally, so Rhai never fails to resolve an overload
and raises a discarded `ErrorIndexingType`. A setter with a wrong-container
target SHALL fail with a `type-mismatch` error.

#### Scenario: Chained, dot, and negative-index access

- **GIVEN** a parsed object `{"a":[{"b":1}]}`
- **WHEN** a script evaluates `j["a"][0]["b"] = 2`, then `j.a[-1].b = 3`
- **THEN** `to_json(j)` SHALL contain the updated nested value, and an
  out-of-range index SHALL fail with `ErrorArrayBounds`

#### Scenario: Missing key versus null

- **GIVEN** a parsed object `{"n":null}` with no key `m`
- **WHEN** a script evaluates `j["n"]`, `j["m"]`, `contains("n")`,
  `contains("m")`, and `"m" in j`
- **THEN** both reads SHALL return unit, `contains("n")` SHALL be `true`,
  `contains("m")` SHALL be `false`, and `"m" in j` SHALL be `false`

#### Scenario: Contains on an array fails typed

- **GIVEN** a parsed array
- **WHEN** a script calls `contains("k")` on it
- **THEN** the step SHALL fail with a `type-mismatch` error

#### Scenario: Push grows and remove returns

- **GIVEN** a parsed array `[1,2]`
- **WHEN** a script calls `push(3)`, then `remove(0)`
- **THEN** the array SHALL grow to `[1,2,3]`, and `remove(0)` SHALL return `1`

#### Scenario: Unit assignment stores null

- **GIVEN** a parsed object
- **WHEN** a script assigns `j["k"] = ()` and re-emits it
- **THEN** the output SHALL contain `"k":null`

#### Scenario: Nested setter failure rolls back

- **GIVEN** a parsed object
- **WHEN** a chained assignment fails on a nested wrong-typed setter
- **THEN** no part of the wrapper SHALL be mutated

### Requirement: Rhai JSON wrapper comparisons refuse implicit coercion

`==`, `!=`, `<`, `>`, `<=`, and `>=` SHALL be registered between
`JsonNumber`/`JsonValue` and native `i64`, `f64`, `String`, and `bool`, and
between wrappers, in both operand orders. All these comparisons SHALL fail with
`type-mismatch`; no exact-number comparison engine SHALL be added. Scripts MAY
explicitly call `to_float()` for lossy comparisons. Pairs outside the registered
set retain Rhai builtin behavior, including comparisons with unit. JSON null
projects to unit, so its equality with unit is true.

#### Scenario: Mixed numeric comparison refuses implicit coercion

- **GIVEN** a `JsonNumber` holding `18446744073709551615`
- **WHEN** a script evaluates `j == 2` or `j < 3`
- **THEN** each SHALL fail with `type-mismatch`

#### Scenario: Null compares with unit

- **GIVEN** a parsed JSON `null`
- **WHEN** a script evaluates the parsed value `== ()`
- **THEN** the comparison SHALL be `true`

### Requirement: Rhai JSON serialization is faithful and compatible

`to_json` SHALL be registered for `Map`, `Array`, `JsonValue`, `JsonNumber`,
and the scalar types `String`, `bool`, `i64`, `f64`, and unit, in both function
and method form. `to_string` and `to_debug` on a wrapper SHALL return the same
compact JSON (for `JsonNumber`, the number token). Native `Map` keys SHALL be
emitted sorted; a wrapper nested inside a native `Map` or `Array` SHALL be
inlined. Output SHALL be raw UTF-8 (non-ASCII emitted directly), SHALL NOT
escape `/`, and SHALL escape control characters. Unsupported values (function
pointers, closures, timestamps, custom types) and non-finite floats SHALL fail
with a `type-mismatch` error and SHALL NOT use a `Debug`/`Display` fallback or
include the value in the error. Serializing a native container SHALL apply the
same depth invariant as parsing.

#### Scenario: Native containers serialize sorted with nested wrappers

- **GIVEN** a native Rhai `Map` `#{"b":1,"a":<parsed value>}`
- **WHEN** a script calls `to_json` on it
- **THEN** the output SHALL order keys `a` then `b` and inline the parsed value

#### Scenario: Function and method forms

- **GIVEN** a parsed value
- **WHEN** a script evaluates `to_json(j)` and `j.to_json()`
- **THEN** both SHALL return the same compact JSON

#### Scenario: Raw UTF-8 and no slash escaping

- **GIVEN** a parsed value containing `café` and `a/b`
- **WHEN** a script calls `to_json`
- **THEN** the output SHALL contain the literal UTF-8 text and the literal `/`

#### Scenario: Non-finite float is refused

- **GIVEN** a Rhai `Map` whose value is `0.0/0.0`
- **WHEN** a script calls `to_json` on it
- **THEN** the step SHALL fail with a `type-mismatch` error and SHALL NOT emit
  `NaN` or a debug string

### Requirement: Rhai JSON wrappers are bounded by the sandbox limits

Parsing SHALL descend the input level at a time using
`serde_json::value::RawValue` and validate iteratively with an owned depth
counter. A top-level scalar SHALL have depth 0 and a top-level container depth
1; depth 128 SHALL be accepted and depth 129 SHALL fail with `limit`. Input
`max-string-size` SHALL be enforced before descent, and total parse cost SHALL
be bounded by 128 × the input byte length. The implementation SHALL NOT enable
`arbitrary_precision` or `unbounded_depth`. Parsing SHALL enforce
`max-array-size` and `max-map-size` per container and fail with `limit` when
either cap is exceeded.

The depth cap SHALL be invariant across parsing and mutation. Every set, push,
and conversion, and every `to_json` of a native container, SHALL check the
depth of the target path plus the depth of the inserted value and SHALL fail
with `limit` before applying when the total would exceed 128; a self-assignment
such as `j["a"] = j` that would exceed the cap SHALL be refused to avoid a stack
crash. Array and map caps and a total serialized-size estimate SHALL be
rechecked on every mutation, and a mutation that would exceed `max-string-size`
SHALL fail with `limit` before being applied. The serializer SHALL be bounded
and SHALL stop at the limit with a `limit` error rather than truncating. These
limits SHALL be read from the calling engine. A size limit configured as `0`
SHALL keep Rhai's unlimited meaning for that cap; the owned depth cap of 128
and the 128 × input-byte parse cost bound SHALL still apply. Exceeding any bound SHALL NOT
commit any Exchange change.

#### Scenario: Depth boundary

- **GIVEN** JSON text whose top-level container has depth 128
- **WHEN** a script calls `parse_json`
- **THEN** parsing SHALL succeed; one more level SHALL fail with a `limit` error

#### Scenario: Mutation depth and cap are rechecked

- **GIVEN** a wrapper at depth 128
- **WHEN** a script assigns a nested value that would deepen it, or pushes
  beyond `max-array-size`
- **THEN** the step SHALL fail with a `limit` error and the wrapper SHALL be
  unchanged

#### Scenario: Self-assignment at the cap is refused

- **GIVEN** a wrapper at depth 128
- **WHEN** a script evaluates `j["a"] = j`
- **THEN** the step SHALL fail with a `limit` error before a stack crash

### Requirement: Rhai JSON errors follow the existing classifier

An unavailable operation SHALL be classified by the engine's existing
classifier. `JsonNumber` arithmetic (`+`, `+=`) SHALL fail as
`function-not-found`; `values` and `merge` SHALL fail as `function-not-found`;
an unsupported `for` iteration SHALL fail as `runtime` (ErrorFor). Every
registered wrapper comparison SHALL fail as `type-mismatch`.

A `parse_json` call whose argument is not a string SHALL resolve and SHALL fail
as `type-mismatch`, naming `string` as the expected type and the argument's
static Rhai type name (`blob`, `map`, `array`, `json value`, `json number`,
`i64`, `f64`, `bool`, `()`, `char`, `FnPtr`, `Instant`) as the actual type, at
the script call position. Such a call SHALL NEVER be classified as
`function-not-found`. The `parse_json` exact-type guards and the wrapper
comparisons are the only failing overloads added.

A size or depth violation SHALL be `limit`. A malformed parse
SHALL be `parse` with no input text and no parser line or column. A setter type
failure SHALL be `type-mismatch` and SHALL NEVER be `ErrorIndexingType`, because
Rhai silently discards indexing errors during index-chain write-back. No error
SHALL contain exchange data.

#### Scenario: Arithmetic and container helpers are function-not-found

- **GIVEN** a `JsonNumber` and a parsed object
- **WHEN** a script evaluates `j + 1`, `j += 1`, `obj.values()`, or
  `obj.merge(x)`
- **THEN** each SHALL fail with a `function-not-found` error

#### Scenario: Non-string parse_json input is a typed mismatch

- **GIVEN** a script step `let data = parse_json(body)` where `body` binds to a
  `Body::Bytes` (Rhai `Blob`) or a `Body::Json` object (native map)
- **WHEN** the step evaluates
- **THEN** it SHALL fail with `expected string, actual blob` or
  `expected string, actual map` at the position of the `parse_json` call
- **AND** the error SHALL NEVER be `function-not-found`
- **AND** the error text SHALL contain no body bytes, no parsed values, and no
  blob hex

#### Scenario: Iteration is a runtime error

- **GIVEN** a parsed object
- **WHEN** a script iterates it with `for`
- **THEN** the step SHALL fail with a `runtime` error (ErrorFor)

#### Scenario: Setter failure propagates

- **GIVEN** a parsed object
- **WHEN** a script assigns a function pointer to a key
- **THEN** the step SHALL fail with a `type-mismatch` error; the failure SHALL
  NOT be an `ErrorIndexingType` and SHALL NOT be discarded

#### Scenario: Parse error is redacted

- **GIVEN** a script that calls `parse_json` on malformed text holding secret
  data
- **WHEN** the script runs
- **THEN** the error SHALL NOT contain the input text or a parser line/column;
  it SHALL carry only the class and the script call position

### Requirement: Rhai host JSON helpers leave the exchange converter unchanged

`parse_json` and `to_json` SHALL be script-local host helpers. They SHALL NOT
change the exchange inbound conversion (`body`, `headers`, and `properties`
binding) or scope preparation, and a JSON integer greater than `i64::MAX` on
that path SHALL still be refused. The outbound `dynamic_to_value` converter
SHALL gain explicit refusal arms for `JsonValue` and `JsonNumber` that use only
the stable labels `json value` and `json number`; no other converter behavior
SHALL change. Direct outbound conversion of a wrapper SHALL remain deferred, so
persisting parsed JSON SHALL require `to_json` or a native leaf. Assigning
`body = to_json(j)` SHALL yield a text body holding valid JSON. Assigning a
wrapper directly to a body SHALL fail as an explicit compatibility break. The
`rc-141om` inbound behavior SHALL remain untouched.

#### Scenario: Exchange inbound behavior is unchanged

- **GIVEN** an exchange property holding a JSON integer greater than `i64::MAX`
- **WHEN** a Rhai script reads that property
- **THEN** the evaluation SHALL still fail with the existing typed conversion
  error

#### Scenario: Outbound refusal uses stable labels

- **GIVEN** a Rhai script whose expression result is a `JsonValue`
- **WHEN** the outbound conversion runs
- **THEN** it SHALL refuse with a conversion error whose source label is
  `json value` (or `json number`) and no Rust type path

#### Scenario: Explicit serialization yields a text body

- **GIVEN** a script that parses JSON and assigns `body = to_json(j)`
- **WHEN** the script runs
- **THEN** the body SHALL be a text body holding valid JSON

### Requirement: Rhai JSON round-trip is semantic, not textual

`to_json(parse_json(s))` SHALL re-emit JSON with the same value, numeric
magnitude, and key order as `s`. Wrapped number tokens SHALL remain exact;
native i64 spelling MAY normalize, including `-0` to `0`.
Whitespace, indentation, and escape spelling
SHALL NOT be preserved and no byte-for-byte guarantee SHALL be made.

#### Scenario: Equivalent, not byte-identical

- **GIVEN** the JSON text `{ "a" : 1 , "b" : "x\/y" }`
- **WHEN** a script re-emits it with `to_json`
- **THEN** the output SHALL be valid JSON with the same value and key order,
  with different whitespace permitted

### Requirement: Simple predicates reject malformed interpolation

`SimpleLanguage::create_predicate` SHALL apply the same unclosed-`${` rule as
template mode: a `${` with no closing `}` in predicate source SHALL be a
compile-time parse error naming `unclosed interpolation` and the byte offset
of the `${` in the trimmed source — never literal text. Single- and
double-quoted strings in predicate source SHALL continue to protect `${` from
slot parsing. Literal `$`, `{`, and `}` outside a `${` SHALL remain literal.

#### Scenario: Unclosed slot after literal text is a parse error

- **GIVEN** a predicate source `Got ${name`
- **WHEN** the predicate is created
- **THEN** creation SHALL fail with a parse error whose message names
  `unclosed interpolation` and `byte offset 4`

#### Scenario: Quoted strings still protect a dollar-brace

- **GIVEN** a predicate source `'Got ${name'`
- **WHEN** the predicate is created
- **THEN** creation SHALL succeed and the parsed string SHALL include
  `${name` verbatim

#### Scenario: Literal dollar and braces stay literal

- **GIVEN** a predicate source `Got $100 from ACME`
- **WHEN** the predicate is created
- **THEN** creation SHALL succeed

### Requirement: Simple bounded scalar expression grammar

The Simple predicate grammar SHALL parse bounded scalar expressions with the
precedence chain `||` < `&&` < comparison < sum < product < unary minus <
atom. Comparison operators are `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`;
a comparison SHALL NOT chain (at most one comparison per level). Sum
operators are `+` and `-` (left-associative); the product operator is `*`;
unary minus negates its operand; parentheses group sub-expressions. Atoms
are the existing literals, lookups (`header.x`, `property.x`,
`exchangeProperty.x`, `body`, `body.<path>`, `exception`,
`exception.message`),
`${language:expr}` delegation, and calls to exactly `int`, `string`,
`default`, `min`, `max`. A bare identifier that names no lookup and no
accepted call SHALL be a parse error; in particular `now()` SHALL be a parse
error — callers inject timestamps through the Exchange
(`CamelMessageTimestamp` -> `nowMs`). Integer literals SHALL be accepted in
u64 range; a unary minus whose operand is the literal `9223372036854775808`
SHALL produce i64::MIN, and any other literal outside i64 range SHALL be a
parse error. Float literals, `/`, `%`, string concatenation with `+`,
ternary conditionals, `in`, `??`, exponentiation, shifts, and JSON or map
construction SHALL be parse errors.

#### Scenario: Precedence and parentheses

- **GIVEN** the predicate
  `int(header.a) + 2 * 3 >= int(header.b) - 1 && property.c || property.d`
- **WHEN** the predicate is created
- **THEN** creation SHALL succeed and the AST SHALL group `2 * 3` before the
  sum, the sums before the comparison, the comparison before `&&`, and `&&`
  before `||`
- **AND** `(int(header.a) + 2) * 3` SHALL change the grouping accordingly

#### Scenario: Comparison does not chain

- **GIVEN** the predicate `1 < 2 < 3`
- **WHEN** the predicate is created
- **THEN** creation SHALL fail with a parse error

#### Scenario: Deferred constructs are parse errors

- **GIVEN** predicates using `1.5`, `7 / 2`, `10 % 3`, `'a' + 'b'`,
  `x ? y : z`, `n in (1, 2)`, `a ?? b`, or `2 ** 3`
- **WHEN** each predicate is created
- **THEN** each creation SHALL fail with a parse error naming the deferred
  construct

#### Scenario: Bare identifier and clock call are rejected

- **GIVEN** the predicates `x == 1` and `now() > 1000`
- **WHEN** each predicate is created
- **THEN** each creation SHALL fail with a parse error
- **AND** `property.x == 1` SHALL still parse

#### Scenario: Accepted calls parse

- **GIVEN** the predicate
  `min(int(header.base), int(header.cap)) <= int(property.nowMs) && default(property.giveUpAt, '0') != 'null'`
- **WHEN** the predicate is created
- **THEN** creation SHALL succeed

#### Scenario: Integer literal range boundaries

- **GIVEN** the literals `9223372036854775807`, `-9223372036854775808`,
  `9223372036854775808`, and `99999999999999999999`
- **WHEN** predicates using each literal are created
- **THEN** `9223372036854775807` SHALL parse to i64::MAX
- **AND** `-9223372036854775808` SHALL parse to i64::MIN via unary minus
- **AND** `9223372036854775808` outside a unary minus SHALL be a parse error
- **AND** `99999999999999999999` SHALL be a parse error
- **WHEN** the predicates `9223372036854775807 + 1` and
  `-9223372036854775808 - 1` evaluate
- **THEN** both SHALL fail with `EvalFailure(Arithmetic)`

#### Scenario: Exception atom renders kind-prefixed message

- **GIVEN** an expression `${exception}` and the exchange carries an
  `HttpOperationFailed` error whose Display text is
  `HTTP GET http://svc/api failed: 500 Internal Server Error`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the string
  `HttpOperationFailed: HTTP GET http://svc/api failed: 500 Internal Server Error`
- **AND** the result SHALL NOT contain the error's response body payload or any
  Debug formatting

### Requirement: Simple integer coercion and arithmetic semantics

The `int(E)` call SHALL return the operand when it is an i64, a u64 that
fits i64, or a string matching `[+-]?[0-9]+` with no whitespace; it SHALL
fail with `ConversionError` for floats, booleans, containers, malformed
strings, and integers outside i64 range (including u64 values above
i64::MAX), and SHALL NEVER substitute a silent 0. The `string(E)` call
SHALL return strings unchanged, SHALL serialize numbers and booleans to
their decimal and `true`/`false` forms, SHALL map null to `""`, and SHALL
fail with `ConversionError` for containers. The `default(a, b)` call SHALL
evaluate `b` lazily — only when `a` yields null, `""`, or `"null"` — SHALL
propagate errors from `a`, and SHALL treat `false` and `0` as present
values. The `min(E, E)` and `max(E, E)` calls and the `+`, `-`, `*`, and
unary `-` operators SHALL operate on i64 with checked arithmetic; overflow
or underflow SHALL fail with `EvalFailure(Arithmetic)`. Arithmetic,
unary minus, `min`, and `max` operands SHALL accept exactly the `int()`
representations (i64, fitting u64, strict integer strings); every other
operand SHALL fail with `TypeMismatch`. String concatenation with `+` is
deferred: when both `+` operands are string literals this SHALL be a parse
error; dynamic operands are validated at evaluation by the operand rule
above, which still admits strict integer strings (numeric-string
arithmetic).

#### Scenario: Integer conversion from numeric strings

- **GIVEN** header `count` with value `"42"` and property `nowMs` with the
  epoch-ms string `"1750000000000"`
- **WHEN** `int(header.count)` and `int(property.nowMs)` evaluate
- **THEN** both SHALL return the exact i64 values 42 and 1750000000000

#### Scenario: Malformed and out-of-range conversion is a catchable error

- **GIVEN** header `v` with value `"abc"`, `" 7"`, or `"1.5"`, header `u`
  with value `"18446744073709551615"`, property `b` with boolean value
  `true`, and property `u64val` holding a native u64 number value above
  i64::MAX
- **WHEN** `int(...)` evaluates each operand
- **THEN** each evaluation SHALL fail with `ConversionError`
- **AND** no evaluation SHALL return 0

#### Scenario: Checked arithmetic catches overflow

- **GIVEN** property `n` with value `"9223372036854775807"`
- **WHEN** `int(property.n) + 1` evaluates
- **THEN** the evaluation SHALL fail with `EvalFailure(Arithmetic)`

#### Scenario: Arithmetic rejects non-integer operands

- **GIVEN** header `f` with value `"1.5"` and property `flag` with boolean
  value `true`
- **WHEN** `header.f * 2` and `property.flag + 1` evaluate
- **THEN** both SHALL fail with `TypeMismatch`

#### Scenario: String conversion rules

- **GIVEN** header `h` with string value `"x"`, header `n` with numeric
  value `42`, property `flag` with boolean value `true`, an unset header,
  and property `arr` holding a JSON array value
- **WHEN** `string(...)` evaluates each
- **THEN** the results SHALL be `"x"`, `"42"`, `"true"`, `""`, and a
  `ConversionError` respectively

#### Scenario: Default is lazy and treats null-like values as missing

- **GIVEN** property `giveUpAt` unset, and header `flag` with value `""`,
  `"null"`, `"0"`, and `false` in turn, and header `bad` = `"abc"`
- **WHEN** `default(property.giveUpAt, '100')` evaluates, the unset case
  SHALL evaluate to `"100"`
- **AND** `default(header.flag, 'x')` SHALL select `'x'` for `""` and
  `"null"`, and SHALL select the operand for `"0"` and `false`
- **AND** `default(header.flag, int(header.bad))` SHALL succeed while `flag`
  is present (the fallback is not evaluated) and SHALL fail with
  `ConversionError` when `flag` is unset
- **AND** an erroring first operand SHALL propagate its error

#### Scenario: Min and max select on i64

- **GIVEN** headers `base` = `"500"` and `cap` = `"3000"`
- **WHEN** `min(int(header.base) * 2, int(header.cap))` evaluates
- **THEN** the result SHALL be the i64 value 1000
- **WHEN** `max(int(header.base), int(header.cap))` evaluates
- **THEN** the result SHALL be the i64 value 3000

### Requirement: Simple comparison and ordering semantics

Equality SHALL keep the existing rules: null equals null; values of
incompatible types compare unequal without an error; a numeric string
coerces to a number for comparison with exact integer equality; `!=` SHALL
be the exact complement of `==`. Unset (null) SHALL NOT equal the empty
string. Ordering comparisons (`>`, `<`, `>=`, `<=`) with a null operand
SHALL return false. Two strings that both parse as strict integers SHALL
compare numerically; any other two strings SHALL compare lexicographically.
A numeric string and a number SHALL compare as exact integers; incompatible
operand types or non-integer numeric strings in mixed comparisons SHALL
fail with `TypeMismatch`. Strict boolean predicate results and
short-circuit truthiness for `&&` and `||` SHALL stay unchanged.

#### Scenario: Equality never throws on type mismatch

- **GIVEN** property `flag` with boolean value `true` and header `ok` with
  string value `"true"`
- **WHEN** `property.flag == header.ok` evaluates
- **THEN** the result SHALL be false with no error
- **AND** `property.flag != header.ok` SHALL be true

#### Scenario: Null equality and the unset-vs-empty distinction

- **GIVEN** header `a` unset and header `b` with value `""`
- **WHEN** `header.a == header.a`, `header.a == header.b`, and
  `header.a != header.b` evaluate
- **THEN** the results SHALL be true, false, and true respectively

#### Scenario: Numeric string coercion is exact beyond float precision

- **GIVEN** header `big` with value `"9007199254740993"` and header `n` with
  value `"42"`
- **WHEN** `header.big == 9007199254740993` and `header.n == 43` evaluate
- **THEN** the results SHALL be true and false respectively
- **AND** `header.big == 9007199254740992` SHALL be false — the comparison
  SHALL NOT round through f64

#### Scenario: Ordering rules

- **GIVEN** header `n` unset, headers `a` = `"10"` and `b` = `"9"`, headers
  `w` = `"apple"` and `v` = `"9"`, header `dec` = `"1.5"`, and header
  `flag` = `true`
- **WHEN** `header.n > 1`, `header.a > header.b`, `header.w > header.v`,
  `header.dec > 1`, and `header.flag > 0` evaluate
- **THEN** the results SHALL be false (null), true (numeric strings compare
  numerically), true (non-numeric strings compare lexicographically), a
  `TypeMismatch` error (non-integer string against a number), and a
  `TypeMismatch` error (boolean against a number) respectively

#### Scenario: Strict boolean results stay

- **GIVEN** the predicate `int(header.n) + 1` with header `n` = `"41"`
- **WHEN** the predicate is evaluated
- **THEN** evaluation SHALL fail with a type-mismatch error because the
  result is a number, not a boolean
- **AND** the predicate `false && header.missing > 1` SHALL return false
  without evaluating the right side

### Requirement: Simple evaluation resource limits

Simple SHALL enforce parse-time limits: source at most 16 KiB, at most 512
AST nodes, and parse-time nesting depth at most 64 with parentheses counted
toward the nesting bound; violations SHALL be parse errors. Simple SHALL
enforce evaluation-time limits with one shared budget per source
evaluation: charged work of one unit per evaluated AST node, one unit per
template slot, and one unit per language delegation, capped at 4096 units;
and produced output capped at 1 MiB. Because the grammar has no repetition
construct, the work ceiling is defense in depth; a source at the 512-node
limit SHALL evaluate within the budget. Limit violations SHALL fail with
`EvalFailure(Limit)`. All Simple diagnostics — parse, conversion,
arithmetic, type-mismatch, limit, and delegation failures — SHALL be
payload-free: they SHALL carry counts, kinds, and positions, never exchange
values.

#### Scenario: Parse-time limits

- **GIVEN** a predicate source larger than 16 KiB, an AST larger than 512
  nodes, and an expression with 65 nested parentheses in turn
- **WHEN** each is created
- **THEN** each creation SHALL fail with a parse error naming the limit
- **AND** an expression with 64 nested parentheses SHALL parse successfully

#### Scenario: Output limit at evaluation

- **GIVEN** a header `big` whose value is larger than 1 MiB
- **WHEN** the template `x${header.big}` evaluates
- **THEN** the evaluation SHALL fail with `EvalFailure(Limit)`
- **AND** the error detail SHALL contain no exchange data

#### Scenario: Node-limited source evaluates within the work budget

- **GIVEN** a valid template whose AST has close to 512 nodes (including
  slot and delegation units in the charge)
- **WHEN** the template evaluates
- **THEN** evaluation SHALL succeed with charged work under the 4096-unit
  ceiling

### Requirement: Template slots parse bounded expressions and unknown languages fail at evaluation

A `${...}` slot SHALL parse with the atom grammar (`body`, `header.`,
`body.<path>`, `exchangeProperty.`, `exception`, `exception.message`, `${language:expr}`
delegation) AND with the bounded scalar expression grammar (`int`,
`string`, `default`, `min`, `max`, arithmetic, comparisons, logical
operators, parentheses). Slots SHALL NOT nest. A slot that names nothing
SHALL be a compile-time parse error. A slot with a valid lowercase language
prefix whose language is not registered SHALL be created successfully and
SHALL fail at evaluation time with an evaluation error, not at compile
time. Existing slot escaping and whole-slot native-value semantics SHALL
stay unchanged.

#### Scenario: Unrecognized slot is a compile error

- **GIVEN** an expression `${unknown}`
- **WHEN** the expression is created
- **THEN** creation SHALL fail with a parse error

#### Scenario: Unknown language is an evaluation failure

- **GIVEN** an expression `${nope:x}` where `nope` is not a registered
  language
- **WHEN** the expression is created
- **THEN** creation SHALL succeed
- **WHEN** the expression is evaluated
- **THEN** evaluation SHALL fail with an evaluation error

#### Scenario: Slot expression evaluates

- **GIVEN** property `nowMs` = `"1750000000000"` and header `delay` = `"500"`
- **WHEN** the template
  `retry-at-${int(property.nowMs) + int(header.delay)}` evaluates
- **THEN** the result SHALL be the string `retry-at-1750000000500`

#### Scenario: Nested slots are a parse error

- **GIVEN** the template `${default(header.x, '${header.y}')}`
- **WHEN** the template is created
- **THEN** creation SHALL fail with a parse error

#### Scenario: Whole-slot expressions keep native results

- **GIVEN** the source `${int(header.n) * 2}` with header `n` = `"21"`
- **WHEN** the source is evaluated as a whole-slot expression
- **THEN** the result SHALL be the number 42, not the string `"42"`

#### Scenario: Exception slot without an error is null

- **GIVEN** an expression `${exception}` and the exchange carries no error
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `Value::Null`

