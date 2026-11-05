## MODIFIED Requirements

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

In Simple template interpolation — any template whose result is a concatenation
of literal text and slot renderings rather than a single whole-slot source — a
Bytes-backed body slot SHALL render as UTF-8 text when the bytes are valid
UTF-8, and SHALL fail with a `conversion` error when they are not. The
interpolation rendering SHALL NEVER be lossy (no U+FFFD replacement) and SHALL
NEVER render the byte-sequence array. The bare-access projection is unchanged:
a whole-slot `${body}` source and every non-template read keep the byte-sequence
array, and nested field access on a Bytes body (`${body.<path>}`) keeps its
existing behavior. This rule applies to the Simple string surface only; Rhai,
JavaScript, and MiniJinja bindings are unchanged.

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

#### Scenario: Bytes embedded in an interpolated template render as UTF-8 text

- **GIVEN** an exchange whose body is `Body::Bytes` holding `hello` (valid
  UTF-8) and a Simple template `pre${body}post`
- **WHEN** the template evaluates
- **THEN** the result SHALL be the string `prehellopost`, with no brackets,
  no byte list, and exactly one concatenation of the slot rendering
- **AND GIVEN** a template `status=${header.code} body=${body}` with header
  `code` = `200` and a body holding `ok`
- **WHEN** the template evaluates
- **THEN** the result SHALL be the string `status=200 body=ok`

#### Scenario: MiniJinja refuses non-UTF-8 Bytes instead of lossy text

- **GIVEN** an exchange whose body is `Body::Bytes` holding invalid UTF-8 and
  a MiniJinja template that renders `{{ body }}`
- **WHEN** the template renders
- **THEN** rendering SHALL fail with a `conversion` error; the bytes SHALL NOT
  be silently replaced with U+FFFD

#### Scenario: Simple interpolation refuses non-UTF-8 Bytes with a typed conversion error

- **GIVEN** an exchange whose body is `Body::Bytes` holding bytes that are not
  valid UTF-8 and a Simple template `body=${body}`
- **WHEN** the template evaluates
- **THEN** the evaluation SHALL fail with a `conversion` error; the result
  SHALL NOT contain U+FFFD replacement characters and SHALL NOT be a byte list

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
