# Design: rhai-json-fidelity

## Approach

Rhai's stock JSON helpers reject `\/`, lose numeric magnitude and key order,
and emit invalid escapes. A shared host module shadows them.

**Data model.** `src/json/mod.rs` defines `JsonValue(Arc<JsonNode>)` and
`JsonNumber(Arc<str>)`, with `JsonNode = Null | Bool | Number(raw token) |
String | Array(Vec<JsonValue>) | Object(IndexMap<String, JsonValue>)`; numbers
hold their exact source token via `serde_json::value::RawValue`.
`arbitrary_precision` and `unbounded_depth` are NOT enabled. The crate declares
`serde_json` (`raw_value` + `preserve_order`, workspace-unified) and `indexmap`
per crate; no workspace feature is added.

**Registration.** Once, a `OnceLock<rhai::Module>` builds the wrapper,
functions, and indexers; `create_base_engine` registers it globally after
`StandardPackage` (Rhai inserts at index 1, taking precedence). Functions read
limits via `ctx.engine().max_string_size()/max_array_size()/max_map_size()`
(`NativeCallContext`).

**Parse.** Parsing descends level at a time with `RawValue` and validates
iteratively: top-level scalar depth 0, container depth 1; 128 accepted, 129 is
`limit`. Input `max-string-size` is enforced before descent and cost is bounded
by 128 × input bytes. Strict RFC 8259 including `\/`; unpaired surrogates and
malformed input are `parse` with no input text or parser line/column, at the
script call position.

**Numbers.** A token is native INT only when integer-form (no fraction or
exponent) and `parse::<i64>()` fits. Decimals, exponents, and out-of-range
integers are `JsonNumber`. `to_float()` is the only conversion and is
`arithmetic` on a non-finite result; there is no implicit `as_f64`.

**Copy-on-write, depth, size.** Reads clone the `Arc` (O(1)); mutation uses
`Arc::make_mut`, so a mutated bare read never aliases its source and a failed
nested setter rolls back. The depth-128 invariant is checked on every set, push,
conversion, and native-container `to_json`, using target-path depth plus value
depth; a self-assignment that would exceed it is refused. Caps and a total size
estimate are checked at parse and rechecked on every mutation; the serializer is bounded.

**Serialization.** Round-trip preserves magnitude and key order; wrapped tokens
remain exact, native i64 spelling may normalize (`-0` to `0`). `to_json` covers `Map`, `Array`, `JsonValue`,
`JsonNumber`, and scalar `String`/`bool`/`i64`/`f64`/unit, function and method;
`to_string`/`to_debug` return the same compact JSON. Native `Map` keys emit
sorted, nested wrappers inline. Output is raw UTF-8, `/` unescaped, controls
escaped. Unsupported/non-finite values are refused `type-mismatch`, never
`Debug`/`Display`.

**Read/write semantics.** One projection: object/array → `JsonValue`;
string/bool/unit and `i64` native; other numbers `JsonNumber`. Missing key and
`null` read as unit; `contains` (objects only) and `in` distinguish presence,
and array `contains` is `type-mismatch`. Dot falls back to string indexing;
negative indexes count from the end, out-of-range `ErrorArrayBounds`; `push`
only appends and rechecks the cap; `remove` shifts and returns the value; unit
assignment stores `null`. Accepted setters: unit, bool, i64, finite f64,
String, `JsonNumber`, `JsonValue`, native `Map`/`Array`. Setters are single
`Dynamic` functions validated internally, so Rhai never raises a discarded
`ErrorIndexingType`; wrong containers are `type-mismatch`.

**Comparisons.** `==`, `!=`, `<`, `>`, `<=`, `>=` are registered between
wrappers, `i64`/`f64`/`String`/`bool`, both operand orders, in Rust, no
separate engine. All registered wrapper comparisons fail `type-mismatch`;
explicit `to_float()` permits lossy comparison. Unregistered pairs retain
builtin behavior; null projects to unit. No numeric comparison engine is added.

**Errors.** The classifier is unchanged; no failing overload is added for
unavailable operations: `+`/`+=` and `values`/`merge` are `function-not-found`,
`for` is `runtime`, every registered wrapper comparison and setter type failure is `type-mismatch` (never
`ErrorIndexingType`), size/depth are `limit`. All errors are redacted.

**Converter, boundary, docs.** `converter.rs` adds outbound refusal arms in
`dynamic_to_value` using only the stable labels `json value`/`json number`;
inbound `json_to_dynamic` and `make_scope` are unchanged. Outbound wrapper
conversion stays deferred, so `set_body(parse_json(...))` fails explicitly and
persistence uses `to_json` or a native leaf. `prepare_scope` is not fixed here
(rc-m01r9 out of scope), but the shared module alters per-eval cost, so a
before/after benchmark is required; rc-141om untouched.
`docs/src/languages/rhai.md` and the crate `CONTEXT.md` document the surface,
break, guarantees, and round-trip caveat.

## Affected crates

- `camel-language-rhai`: json module, shared registration, `converter.rs`
  outbound arms, per-crate deps, docs, `CONTEXT.md`.

## Architecture boundaries

Inside the Languages boundary; the SPI and inbound converter contract are
untouched. Untrusted data keeps redaction (ADR-0032, ADR-0051); bounding reuses
the ADR-0011 / ADR-0033 limit channel; no log-level change (ADR-0012).

## Alternatives considered

- **`arbitrary_precision` / `unbounded_depth`.** Rejected: global serializer
  change, unbounded parsing.

## Tests

- Strict JSON: `\/`, surrogate pairs, duplicate last-wins; malformed and
  unpaired surrogates refused without leak.
- Unicode raw UTF-8, `/` unescaped; numbers: native `i64` extrema;
  `1.0`/`1e2`/`u64::MAX`/30-digit/`1e400` are `JsonNumber`; `to_float` finite.
- Order: authored, duplicate last-wins, shift-remove keeps `b`,`c`.
- Mutation: chained/dot/`+=` on native i64, bare-read isolation, nested
  rollback, caps rechecked.
- Depth: 128 accepted, 129 `limit`; mutation depth and self-assignment refused.
- Semantics: missing vs null, array `contains` refusal, negative indexes,
  `ErrorArrayBounds`, `remove` return, unit to null.
- Errors: `function-not-found` for `+`/`+=`/`values`/`merge`, `runtime` for
  `for`; `type-mismatch` for comparisons/setters; outbound labels
  `json value`/`json number` with sorted native containers and nested wrappers.
- Benchmark: per-eval registration cost.

## Risks

Chained write-back, overload precedence, and depth/size accounting require
regression coverage.
