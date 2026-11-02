## MODIFIED Requirements

### Requirement: Absent keys and null values are distinguishable

Reading a property or header SHALL return the Language's unit/null value for
both an absent key and a stored `null`. A `has_property(name)` and a
`has_header(name)` function SHALL exist wherever the language exposes header or
property data, so scripts can tell the two states apart. Presence SHALL be
tested by key containment, not by comparing the read value to unit/null; a
stored `Value::Null` is therefore present. Each language tests presence on the
same view its value readers use: Rhai is snapshot-backed (the pre-evaluation
snapshot maps also returned by `header()`/`property()`), JavaScript is live
(the current `camel.headers`/`camel.properties` maps, matching
`camel.headers.has`), and Simple and MiniJinja read the per-evaluation exchange
view they already expose.

The surface is per language:

- Simple: `has_header("k")` and `has_property("k")`.
- Rhai: `has_header("k")` and `has_property("k")` on read-only and script-step
  engines, backed by the pre-evaluation snapshot maps.
- JavaScript: `camel.has_header("k")` and `camel.has_property("k")` (the
  existing `camel.headers.has`/`camel.properties.has` map methods remain).
- MiniJinja: `has_header("k")` and `has_property("k")` callable values.
- JSONPath and XPath: not applicable; both are body-only languages (see the
  body-only requirement).

#### Scenario: has_property separates absent from null

- **GIVEN** no property `p` set, and a property `q` explicitly set to `null`
- **WHEN** a script evaluates `has_property("p")` and `has_property("q")`
- **THEN** both `property("p")` and `property("q")` SHALL return the unit
  value, while `has_property("p")` SHALL return `false` and
  `has_property("q")` SHALL return `true`

#### Scenario: has_header separates absent from null in every key language

- **GIVEN** a Simple expression, a Rhai expression, a JavaScript expression,
  and a MiniJinja template, each reading the key `flag`, where header `flag`
  is absent
- **WHEN** each evaluates its `has_header("flag")`
- **THEN** each SHALL return `false`
- **AND GIVEN** header `flag` is then set to `null`
- **THEN** each `has_header("flag")` SHALL return `true` while the value read
  remains unit/null

### Requirement: Body exposure is native and non-destructive

For script evaluation surfaces (Simple, Rhai, JavaScript) the `body` value
exposed to scripts SHALL be native: `Text` and `Xml` as a string, `Json` as the
native structured value, and `Empty` as the unit/null value. `Bytes` SHALL be a
byte-sequence value; in the JSON-valued surfaces (Simple, JavaScript) that is
the JSON array of integers 0-255, and in Rhai it is a Blob. MiniJinja is a
template surface, not a script surface: its context keeps the recorded template
projections (`Empty` as the empty string, `Text`/`Xml` as a string, `Json`
structured), for the inline MiniJinja language, `Bytes` is presented as UTF-8 text and SHALL
be refused with a conversion error when the bytes are not valid UTF-8 (never
lossy decoding in the language evaluation), and `Stream` is refused lazily per
the access-aware requirement. The external template component (`camel-template`,
ADR-0047) keeps its documented lossy byte projection and is not bound by this
language requirement.

For every surface, `Stream` SHALL NOT be materialized implicitly: reading the
stream body's VALUE through `body` SHALL fail, and the refusal SHALL happen
only when the expression actually reads the body (see the access-aware refusal
requirement).
An access that materializes the value before failing (for example an
unsupported operator with a marker operand) is a boundary refusal and SHALL
fail the step even when handled in-script; a failure the script handles with
in-script error handling before materializing a value SHALL be suppressed per
the in-script error-handling requirement. Static type introspection and no-op
accesses that materialize no value MAY be exempt (rhai `type_of(body)` returns
`"StreamBodyRef"`; a bare discarded `body;` is optimized away).

#### Scenario: JSON body reads as a map

- **GIVEN** an exchange whose body is `Body::Json` holding `{"n": 1}`
- **WHEN** a rhai script reads `body`
- **THEN** it SHALL see a map, not an empty string, and `type_of(body)` SHALL
  be `map`

#### Scenario: Simple JSON body reads as a map

- **GIVEN** an exchange whose body is `Body::Json` holding `{"n": 1}` and a
  Simple expression `${body}`
- **WHEN** the expression evaluates
- **THEN** the result SHALL be the native JSON object `{"n": 1}`, not the
  string `{"n":1}`

#### Scenario: JavaScript JSON body reads as an object

- **GIVEN** an exchange whose body is `Body::Json` holding `{"n": 1}` and a
  JavaScript expression `camel.body.n`
- **WHEN** the expression evaluates
- **THEN** the result SHALL be the native number `1`

#### Scenario: Bytes read as a byte-sequence array

- **GIVEN** an exchange whose body is `Body::Bytes` holding `[0, 128, 255]`
  and a Simple `${body}` expression and a JavaScript `camel.body` expression
- **WHEN** each evaluates
- **THEN** each SHALL produce the JSON array `[0, 128, 255]`, with no lossy
  UTF-8 decoding

#### Scenario: MiniJinja refuses non-UTF-8 Bytes instead of lossy text

- **GIVEN** an exchange whose body is `Body::Bytes` holding invalid UTF-8 and
  a MiniJinja template that renders `{{ body }}`
- **WHEN** the template renders
- **THEN** rendering SHALL fail with a `conversion` error; the bytes SHALL NOT
  be silently replaced with U+FFFD

#### Scenario: Stream body fails loudly

- **GIVEN** an exchange whose body is `Body::Stream`
- **WHEN** an expression consumes the stream body's value through `body`
  (method call, indexing, materializing capture into a variable in Simple or
  Rhai, function argument, result position, or a MiniJinja consuming operation)
- **THEN** the evaluation SHALL fail with a `conversion` error for Simple and
  Rhai, and with the language's existing payload-free refusal for JavaScript
  and MiniJinja; the stream SHALL NOT be silently read as null, an empty
  string, or lossy text

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
  value) in in-script error handling and returns a fallback value
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
(`Json`, `Xml`, `Bytes`, `Stream`, `Empty`). A JavaScript `Body::Stream` that
is neither read nor replaced SHALL remain the original `Body::Stream`; a
replacement of the `camel.body` property SHALL write the replacement value.

For JavaScript, assigning `null` to the body SHALL make it `Body::Empty`. A
`null` assignment on a JavaScript `Body::Empty` body SHALL be a no-op (the
normalized snapshot is also null). A JavaScript `Body::Bytes` body is exposed
as an integer array, so a `null` assignment on it SHALL be a real change and
SHALL make it `Body::Empty`. For Rhai, `Bytes` is exposed as a Blob, so
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
  type.

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

#### Scenario: JS null on Bytes is a change

- **GIVEN** an exchange with a `Body::Bytes` body and a `js` `script:` step
  that assigns `camel.body = null`
- **WHEN** the step succeeds
- **THEN** the body SHALL be `Body::Empty` (JavaScript Bytes are exposed as an
  integer array, so null is a real change); a null assignment on a JavaScript
  `Body::Empty` body SHALL remain `Body::Empty`

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

For JavaScript, the comparison SHALL use the same value conversion on both
sides, so an unchanged value whose representation changed through the
JavaScript round-trip (an integral `f64` such as `10.0` becoming the integer
`10`, recursively inside arrays and objects) compares equal and SHALL NOT be
written back. Untouched entries SHALL keep their original JSON number form.

#### Scenario: Untouched map property stays an object

- **GIVEN** a property holding `{"a": [1,2]}` and a `script:` step that
  modifies a different property
- **WHEN** the step succeeds
- **THEN** the untouched property SHALL still hold the JSON object `{"a": [1,2]}`

#### Scenario: Untouched JS float header keeps its representation

- **GIVEN** a header `n` holding the JSON number `10.0` and a `js` `script:`
  step that modifies a different header
- **WHEN** the step succeeds
- **THEN** header `n` SHALL still hold `10.0` (not the integer `10`)

#### Scenario: Conversion failure commits nothing

- **GIVEN** a `script:` step that changes one property to a valid value and
  another to a value the engine cannot represent
- **WHEN** the write-back conversion fails
- **THEN** neither change SHALL be applied; the Exchange SHALL keep its
  pre-step state

### Requirement: Read surface parity across evaluation modes

Read-only and mutating evaluation SHALL expose the same read surface within
each language's native data model: a body view, header and property views, and
the reader and key-presence functions the language supports. The Rhai
read-only scope SHALL include the `properties` variable, and the Rhai script-step (mutating) engine
SHALL register `header()`, `property()`, `has_header()`, and `has_property()`
backed by the pre-evaluation snapshot maps. JavaScript SHALL expose
`camel.header(name)`, `camel.property(name)`, `camel.has_header(name)`, and
`camel.has_property(name)`. MiniJinja SHALL expose `header(name)`,
`property(name)`, `has_header(name)`, and `has_property(name)` beside its
`body`, `headers`, and `exchangeProperty` context entries. Simple SHALL expose
`has_header(name)` and `has_property(name)` beside its `header.<name>` and
`exchangeProperty.<name>` accessors.

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

#### Scenario: key-presence readers work in read-only and mutating modes

- **GIVEN** an exchange with property `level` set and no header `level`
- **WHEN** a read-only predicate and a mutating script each evaluate
  `has_property("level")` and `has_header("level")`
- **THEN** both SHALL return `true` for the property and `false` for the
  header in both modes

#### Scenario: JS readers see live mutations

- **GIVEN** a js `script:` step with source
  `camel.headers.set("k", "new"); camel.headers.get("k")` and an exchange whose
  header `k` is `"old"`
- **WHEN** the step evaluates
- **THEN** `camel.headers.get("k")` SHALL return `"new"` (live behavior,
  unchanged)

## ADDED Requirements

### Requirement: Streaming body refusal is access-aware

Refusing a `Body::Stream` SHALL depend on whether the expression actually reads
the body, not on the body variant alone. An expression that never reads the
body (for example a header-only filter) SHALL evaluate successfully against a
`Body::Stream` in Simple, Rhai, JavaScript, and MiniJinja, and SHALL leave the
stream unconsumed. An expression that reads the body SHALL refuse per the body
exposure requirement. Static reference to `body` in an unexecuted branch is not
a read in any of the four languages.

What counts as a read is per engine. In Simple and Rhai it is any
materialization of the value. In JavaScript it is any access to the `camel.body`
property (a getter); an assignment replaces the body without reading it, and a
caught getter exception is a handled read. In MiniJinja it is any variable
lookup of the `body` name or any consuming operation on the value: rendering,
filter or function coercion, comparison, iteration, member access, a `{% set %}`
capture, or a truth test. MiniJinja validates an invalid value at lookup, so a
capture and a truth test refuse immediately; a reference inside an unexecuted
branch is never looked up and does not refuse.

#### Scenario: Header-only Simple filter passes a stream body

- **GIVEN** an exchange with a `Body::Stream` body and header `type` set to
  `order`, and a Simple filter `${header.type} == 'order'`
- **WHEN** the predicate evaluates
- **THEN** it SHALL return `true`; the stream SHALL be unconsumed

#### Scenario: Header-only Rhai filter passes a stream body

- **GIVEN** the same exchange and a rhai filter `header("type") == "order"`
- **WHEN** the predicate evaluates
- **THEN** it SHALL return `true`; the stream SHALL be unconsumed

#### Scenario: Header-only JavaScript filter passes a stream body

- **GIVEN** the same exchange and a JavaScript filter
  `camel.header("type") === "order"`
- **WHEN** the predicate evaluates
- **THEN** it SHALL return `true`; the stream SHALL be unconsumed

#### Scenario: Header-only MiniJinja render passes a stream body

- **GIVEN** the same exchange and a MiniJinja template that reads only a
  header
- **WHEN** the template renders
- **THEN** rendering SHALL succeed; the stream SHALL be unconsumed

#### Scenario: MiniJinja unexecuted body branch does not read the stream

- **GIVEN** an exchange with a `Body::Stream` body and a template whose
  `body` reference is inside a branch that does not execute
- **WHEN** the template renders
- **THEN** rendering SHALL succeed; the stream SHALL be unconsumed

#### Scenario: MiniJinja capture and truth test refuse a stream body

- **GIVEN** an exchange with a `Body::Stream` body and a template
  `{% set captured = body %}ok`
- **WHEN** the template renders
- **THEN** rendering SHALL fail with the stream refusal (lookup validates the
  value), not render `"ok"`
- **AND GIVEN** a template `{% if body %}x{% endif %}`
- **THEN** rendering SHALL fail with the stream refusal

#### Scenario: JavaScript replaced stream body needs no read

- **GIVEN** an exchange with a `Body::Stream` body and a JavaScript mutating
  step that assigns `camel.body = "done"` without reading it
- **WHEN** the step succeeds
- **THEN** the body SHALL be `Body::Text("done")`

### Requirement: JSONPath and XPath are body-only languages

JSONPath and XPath SHALL expose only the body. They SHALL NOT expose header or
property variables, nor `header()`, `property()`, `has_header()`, or
`has_property()`. This is a deliberate boundary decision (audit matrix section
4): both languages address a document body and have no key-addressed input
surface. Their key-presence behavior is therefore not applicable and the
absent-vs-null requirement does not bind them.

#### Scenario: JSONPath and XPath do not expose key readers

- **GIVEN** a JSONPath expression or predicate and an XPath expression or
  predicate
- **WHEN** the expression is created and evaluated
- **THEN** neither SHALL resolve a header or property reader; the only input
  SHALL be the body, and a header-only filter is not a supported form for these
  languages
