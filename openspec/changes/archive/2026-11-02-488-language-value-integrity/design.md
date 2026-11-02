# Design: 488-language-value-integrity

## Approach

Three sequenced slices, all inside `crates/languages/`. The spec is the
authority (ADR-0032 trust boundary, ADR-0047 MiniJinja, ADR-0092 Simple).

**Read surfaces + key presence.** `has_header(name)`/`has_property(name)`
return `bool` from key containment on the same view the language's value
readers use, never by comparing the read value (so a stored `Value::Null` is
present). Rhai is snapshot-backed; JavaScript is live, matching
`camel.headers.has`. Implementations:
Simple adds `CallKind::HasHeader`/`HasProperty` (parsed as
`has_header("k")`, evaluated in both the sync and async walks); Rhai registers
the functions on both `create_eval_engine` and the mutating engine's
snapshot-reader setter; JS adds `camel.has_header`/`camel.has_property` and a
`camel.header(name)` reader beside `camel.property`; MiniJinja adds
`header(name)`, `property(name)`, `has_header(name)`, and `has_property(name)`
as callable context values (per-evaluation `Object`s) because its `Environment`
is compiled once and shared. JSONPath and XPath stay body-only: the spec records this as a
deliberate boundary (they have no key-addressed input surface), closing the
audit-matrix section 4 decision. The spec's `rc-w1mds` deferral sentence is
removed in the same change that adds the implementations.

**Value integrity.** Simple's body reader becomes a single fallible helper used
by both walks (script surfaces: Simple, Rhai, JavaScript): `Json` clones natively, `Xml`/`Text` stay strings, `Bytes`
becomes a JSON array of integers 0-255 (the documented Blob projection),
`Empty` is null, `Stream` is refused with a `conversion` error. Body-field reads remain `Json`-only; a `Stream` body is refused, other variants
read as absent. MiniJinja is a template surface, not a script surface: its
context keeps the recorded projections (`Empty` as empty string, `Text`/`Xml`
as string, `Json` structured, `Bytes` as UTF-8 text). The INLINE language
evaluation refuses a `Bytes` body that is not valid UTF-8 with a conversion
error instead of lossy decoding. The shared `build_context_bounded` helper and
the external `camel-template` component (ADR-0047) keep their documented lossy
byte projection, so the refusal check lives in `MinijinjaExpression::evaluate`
before it builds the context, not in the shared helper. JS `snapshot_exchange` maps `Bytes` to the same integer array; because
the pre-run snapshot is captured through the identical conversion, an untouched
`Bytes` body is never written back. JS map write-back becomes changed-only: the
expression layer compares post-evaluation maps against the actual installed
pre-run snapshot after the same JS normalization (integral `f64` within the safe
range becomes an integer, recursively), applies only changed/removed keys, and
leaves untouched entries' original JSON value and type intact. Conversions all
complete inside the worker before any mutation, so a conversion failure leaves
the Exchange unchanged (the existing rollback is then redundant, not load
bearing).

**Access-aware streaming refusal.** Simple refuses only in the `Body`/
`BodyField` arms. Rhai keeps its lazy `StreamBodyRef` marker (already
conformant; add the header-only regression test). MiniJinja installs a
runtime-lazy invalid value carrying a static stream detail for the `body`
context entry, plus a formatter that errors only when an invalid value's
rendered text carries that static detail (and otherwise delegates to
`escape_formatter`, so unrelated invalid values keep their existing behavior).
Filters, comparisons, access, and rendering therefore fail at use, while a
header-only template never touches the value; an unexecuted branch does not
read it. `map_render_error` recognizes the static detail by exact string (never
user data) and keeps the existing payload-free refusal. The measurement pass
serializes a stream as an empty string (the context value, not the
measurement, carries the refusal).

JS installs `camel.body` as an accessor for a stream input: the getter throws a
per-evaluation opaque stream sentinel, classified as the existing runtime-class
refusal; the setter replaces the accessor with a sibling data property. The
engine getter is retained in `InstalledGlobals`, and extraction reads the
property descriptor, never the getter: a descriptor whose getter is
identity-equal to the engine getter means the stream was neither read nor
replaced and the inbound `Body::Stream` is preserved; a data property is the
replacement value; a deleted property yields `Body::Empty`; a DIFFERENT
accessor (a script-installed replacement) is an unsupported body replacement,
refused with the runtime class so the step fails and rolls back rather than
executing script code during extraction.
`JsExchange` gains `body_is_stream`; `JsEvalResult` gains
`body_unread_stream`/`body_stream_replaced` so the expression layer never
writes a preserved stream. These are crate-public struct fields: additive to
the `JsEngine` trait (no method change), but SOURCE-BREAKING for downstream
struct literals and external `JsEngine` implementors. In-tree construction
sites are updated; `Default` keeps the new fields at `false`, and an external
implementor that leaves them false gets the pre-change body semantics (never a
preserved-stream write). Read-only evaluation is unchanged apart from
tolerating the accessor. A caught getter exception is a handled read and does
not fail the step, mirroring Rhai. What counts as a read is per engine:
materialization for Simple/Rhai, property access for JS, and for MiniJinja any
variable lookup of `body` or consuming operation (render, filter/coercion,
comparison, iteration, member access, `{% set %}` capture, truth test).
MiniJinja validates an invalid value at lookup, so capture and truth tests
refuse immediately; an unexecuted branch is never looked up and does not
refuse.

## Affected crates

- `camel-language-simple`: body helper, `has_*`, stream refusal, R3 flip test.
- `camel-language-rhai`: `has_*` registrations, header-only stream test,
  write-back pin tests (Phase 3).
- `camel-language-js`: accessor stream body, enum state, changed-only maps,
  `Bytes`, `has_*`/`header`, regression tests.
- `camel-language-minijinja`: invalid-value stream body, formatter, `has_*`
  callable context values, non-UTF-8 `Bytes` refusal (value-integrity ride-along
  in Phase 2), tests.
- `camel-language-jsonpath`, `camel-language-xpath`: no code change; body-only
  decision is spec-only (existing tests already pin body-only behavior).
- `camel-language-api`: no trait change.
- `docs/src/languages/*`: user-visible behavior.

## Architecture boundaries

Only the Languages bounded context and its user docs change. The Runtime sees
the same `Expression`/`MutatingExpression`/`Predicate` traits; no DSL, EIP,
component, or registry change. Redaction is preserved: stream refusals stay
payload-free, and JS error text is still never forwarded.

## Phases

### Phase 1: Read surfaces and key presence (rc-u8t5c)
- **Goal:** `has_header`/`has_property` in Simple/Rhai/JS/MiniJinja; JS
  `header()` and MiniJinja `header()`/`property()` readers; JSONPath/XPath
  body-only decision recorded.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** new reader functions only.
- **Deliverable:** code + tests + spec delta for absent-vs-null.
- **Exit-criteria:** absent-vs-null and reader tests pass per language; spec
  has no `rc-w1mds` deferral; JSONPath/XPath scenario present.

### Phase 2: Access-aware streaming refusal (rc-xnpg7)
- **Goal:** refuse a stream body only when read; header-only works in all four;
  MiniJinja's lossy `Bytes` decode is replaced by a conversion refusal.
- **Dependencies:** Phase 1 (JS reader surface present before JS body work).
- **Externally-visible types/interfaces:** `JsExchange.body_is_stream`,
  `JsEvalResult.body_unread_stream`/`body_stream_replaced` (crate public;
  additive to the trait, source-breaking for struct literals).
- **Deliverable:** code + four header-only stream tests + spec delta.
- **Exit-criteria:** header-only-on-stream passes for Simple, Rhai, JS,
  MiniJinja; reading `body` refuses.

### Phase 3: Value integrity (rc-zwy7e)
- **Goal:** Simple native JSON/Bytes; JS changed-only transactional maps,
  `Bytes`, normalization; absorb audit R3 flip test.
- **Dependencies:** Phase 2 (JS body state already lazy, so the value work
  does not introduce a second eager snapshot).
- **Externally-visible types/interfaces:** Simple `Bytes`/`Json` exposure;
  JS map write-back semantics.
- **Deliverable:** code + R3 flip test + spec/docs.
- **Exit-criteria:** R3 test passes as a contract test; untouched `10.0` and
  untouched maps survive a JS mutating step; conversion failure commits
  nothing.

## Alternatives considered

- Adding header/property readers to JSONPath/XPath: rejected. Both are
  body-only by design (no key-addressed input), and the audit matrix asks for
  an explicit decision, not new surfaces.
- Static MiniJinja `undeclared_variables` gate: rejected. A template can
  mention `body` in an unexecuted branch; a syntactic reference is not a read.
  Runtime invalid-value access is the only faithful mechanism.
- Eager JS accessor flag only (boolean "read"): rejected. Scripts can delete or
  redefine the property; the property descriptor plus host-owned result state
  is the only sound extraction rule.
- Reclassifying every stream refusal to `conversion` now: out of scope under
  "preserve error classes"; `rc-fukc6` tracks class unification.
