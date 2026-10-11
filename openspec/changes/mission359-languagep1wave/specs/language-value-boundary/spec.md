## ADDED Requirements

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
as a raw literal. "Simple
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

#### Scenario: Unclosed slot after a valid slot is a parse error

- **GIVEN** an expression source `"${header.h} ${"` with header `h` equal to
  `B`
- **WHEN** the expression is created
- **THEN** creation SHALL fail with a parse error; an unclosed `${` in a
  source with no completed slot and preceding literal text (`Got ${body`)
  SHALL remain literal text. A source that begins with an unclosed `${`
  (`${body` or `${`) SHALL fail with a parse error.

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

### Requirement: Template slots parse atoms and unknown languages fail at evaluation

A `${...}` slot SHALL parse with the existing atom grammar (`body`, `header.`,
`body.<path>`, `exchangeProperty.`, `exception.message`, `lang:expr`). A slot
that names nothing SHALL be a compile-time parse error. A slot with a valid
lowercase `lang:` prefix whose language is not registered SHALL fail at
evaluation time with an evaluation error, not at compile time.

#### Scenario: Unrecognized slot is a compile error

- **GIVEN** an expression `${unknown}`
- **WHEN** the expression is created
- **THEN** creation SHALL fail with a parse error

#### Scenario: Unknown language is an evaluation failure

- **GIVEN** an expression `${nosuchlang:x}` with no resolver entry for
  `nosuchlang`
- **WHEN** the expression is evaluated
- **THEN** evaluation SHALL fail with an evaluation error; creation SHALL
  succeed

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

## MODIFIED Requirements

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
