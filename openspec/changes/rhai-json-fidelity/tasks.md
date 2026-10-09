# Tasks: rhai-json-fidelity

Single delivery phase. The implementation adds a private `json` module, a
process-wide host `rhai::Module`, and two outbound refusal arms in
`converter.rs`. No `## Phase N` headings: the flat task loop applies.

## Preconditions (read before the first task)

- Worktree root (`$WT`) is this worktree, on `feature/355-rhaijson`. Every
  cargo command runs with `CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4
  TMPDIR=/home/shared/tmp` from `$WT`. Never build in the main checkout.
- Before any `--release` compile or the final gate block, run
  `df -h "$PWD" | awk 'NR==2{print $5}'`; if the use percentage is `>= 78%`,
  STOP and report `park: disk usage` instead of proceeding.
- Comparison contract (approved by the spec blessing, do NOT alter): the six
  registered operators `==`, `!=`, `<`, `>`, `<=`, `>=` between the two wrapper
  types and native `i64`/`f64`/`String`/`bool`, both operand orders, and between
  the two wrapper types, ALL fail with `type-mismatch`. There is NO exact
  numeric comparison or equality engine.
- `rc-m01r9` (`make_scope`/`prepare_scope` eager property conversion) and
  `rc-141om` (u64 inbound refusal) stay UNCHANGED. Task 1.1 measures the former
  as a baseline only. The inbound converter is unchanged; the only converter
  change is two outbound refusal arms.
- RED/GREEN meaning in test lines: `red:` is the observed result before the
  task's implementation exists (compile error or assertion failure); `green:`
  is the observed result after. A test that is already green before its task is
  a plan defect — report it.

## camel-language-rhai

### Task 1.1: Baseline the per-eval registration and scope-prep cost

Record the pre-change per-eval cost BEFORE any move of `create_base_engine`, so
the shared-module cost can be compared after Task 1.5.

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)

**Steps:**
1. In the `#[cfg(test)] mod tests` block, add helper
   `fn bench_exchange(property_entries: usize) -> Exchange`: build
   `Exchange::new(Message::new(""))` and insert property `"buf"` as
   `Value::Object((0..property_entries).map(|i| (i.to_string(), Value::from(i as i64))).collect())`.
   This reproduces the `rc-m01r9` eager `make_scope`/`prepare_scope` deep
   conversion: every property is converted with `json_to_dynamic` on every eval.
2. Add `#[test] #[ignore = "slow test: perf baseline; run explicitly with --release"] fn
   bench_json_host_per_eval_cost()`. Build
   `lang = RhaiLanguage::with_limits(RhaiLimitsConfig { execution_timeout_ms: Some(60_000), ..Default::default() })`
   and `expr = lang.create_expression("1 + 1").unwrap()`.
3. For phase A (`bench_exchange(0)`) and phase B (`bench_exchange(4000)`):
   run 5 warmup `evaluate` calls discarding results, then 20 timed calls, and
   compute the median elapsed duration in nanoseconds.
4. Print exactly one machine-readable line:
   `BENCH json_host_per_eval small_ns=<a> big_ns=<b>`.
5. Assert `a > 0 && b > 0` and `b >= a` (the known eager-scope cost is
   reproduced). No production code path changes in this task.

**Tests:**
- `bench_json_host_per_eval_cost`: setup small + 4000-entry property exchanges
  → 5 warmup + 20 timed evals per phase, print medians → assert both medians
  `> 0` and `big >= small`.
  red: test does not exist (compile error); green: prints the two medians.
  command: `CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp cargo test -p camel-language-rhai --release --lib bench_json_host_per_eval_cost -- --ignored --nocapture`

**Acceptance:**
- The baseline line is captured and recorded in the task report as
  `small_ns=<a> big_ns=<b>` (Task 1.5 compares against it).
- `CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp cargo fmt --check -p camel-language-rhai` exits 0.
- `CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp cargo clippy -p camel-language-rhai -- -D warnings` exits 0.
- `git diff --stat` shows only `src/lib.rs`.

- [x] 1.1

### Task 1.2: Bounded raw-token tree and strict RFC 8259 parser

Implement the private data model and the level-at-a-time parser. No public
registration yet.

**Files:**
- `crates/languages/camel-language-rhai/Cargo.toml` (modified)
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)
- `crates/languages/camel-language-rhai/src/json/mod.rs` (new)
- `crates/languages/camel-language-rhai/src/json/parse.rs` (new)
- `Cargo.lock` (modified: parser dependency declaration)

**Steps:**
1. In `Cargo.toml` `[dependencies]` add
   `serde_json = { workspace = true, features = ["raw_value", "preserve_order"] }`
   and `indexmap = { workspace = true }`, plus `serde = { workspace = true }`
   for a duplicate-preserving map visitor. No new serde_json features are added.
   Verify how the features arrive with
   `cargo tree -e features -i serde_json 2>/dev/null | grep -E 'preserve_order|raw_value'`
   (`preserve_order` is already unified: Cargo.lock resolves serde_json →
   indexmap 2.14.2); report if either feature is absent.
2. In `src/lib.rs` add `mod json;` (private).
3. In `src/json/mod.rs` declare `pub(crate) mod parse;` and define:
   - `pub(crate) struct JsonValue(Arc<JsonNode>)` deriving `Clone, Debug, PartialEq`.
   - `pub(crate) struct JsonNode { pub(crate) kind: JsonKind, pub(crate) depth: usize, pub(crate) size: usize }`.
   - `pub(crate) enum JsonKind { Null, Bool(bool), Number(JsonNumber), String(String), Array(Vec<JsonValue>), Object(IndexMap<String, JsonValue>) }`.
   - `pub(crate) struct JsonNumber(Arc<str>)` deriving `Clone, Debug, PartialEq`.
   - `pub(crate) const JSON_VALUE_TYPE_NAME: &str = "json value";`
   - `pub(crate) const JSON_NUMBER_TYPE_NAME: &str = "json number";`
   - `pub(crate) const MAX_JSON_DEPTH: usize = 128;`
   - `pub(crate) enum JsonHostError { Parse, Limit, TypeMismatch, Arithmetic }`.
4. Implement `JsonValue::new(kind) -> Self` computing cached `depth` and `size`
   (scalar depth 0; container depth `1 + max(child.depth)`; size = token/UTF-8
   byte length plus child sizes plus 2 per container). Implement
   `JsonValue::kind(&self) -> &JsonKind`, `JsonValue::depth(&self) -> usize`,
   `JsonValue::size(&self) -> usize`, and
   `JsonValue::mutate<R>(&mut self, f: impl FnOnce(&mut JsonKind) -> Result<R, JsonHostError>) -> Result<R, JsonHostError>`
   which uses `Arc::make_mut(&mut self.0)`, applies `f`, then recomputes
   `depth`/`size` on the same node (this is the copy-on-write / cached
   size-depth contract).
5. Implement `JsonNumber::new(token: impl Into<Arc<str>>) -> Self`,
   `JsonNumber::as_str(&self) -> &str`, and
   `JsonNumber::to_f64(&self) -> Result<f64, JsonHostError>` returning
   `Err(JsonHostError::Arithmetic)` when the parsed `f64` is not finite.
6. Implement `pub(crate) fn token_projects_to_i64(token: &str) -> Option<i64>`:
   return `None` when the token contains `.`, `e`, or `E`; otherwise
   `token.parse::<i64>().ok()`, normalizing `-0` to `0`.
7. Implement `pub(crate) fn project_dynamic(value: &JsonValue) -> rhai::Dynamic`:
   `Object`/`Array` → `Dynamic::from(JsonValue)` clone; `String` → `Dynamic::from(String)`;
   `Bool` → `Dynamic::from(bool)`; `Null` → `Dynamic::UNIT`; `Number` → native
   `i64` when `token_projects_to_i64` is `Some`, else `Dynamic::from(JsonNumber)`.
8. In `src/json/parse.rs` define
   `pub(crate) struct JsonLimits { pub(crate) max_string_size: usize, pub(crate) max_array_size: usize, pub(crate) max_map_size: usize }`
   and `pub(crate) fn parse(input: &str, limits: &JsonLimits) -> Result<JsonValue, JsonHostError>`.
9. Enforce `max_string_size` on the raw input byte length BEFORE descent; `0`
   means unlimited. Descend one level at a time with
   `serde_json::from_str::<Box<serde_json::value::RawValue>>` for the value and,
   for arrays, `Vec<Box<RawValue>>`. For objects define private
   `struct RawEntries(Vec<(String, Box<RawValue>)>)` with a `serde::Deserialize`
   implementation using `Visitor::visit_map` and `MapAccess::next_entry`.
   Retain EVERY raw key/value occurrence and push every value onto the work
   stack for scalar and depth validation, including values overwritten later.
   Only in `Work::FinishObject`, after all occurrences validate, insert parsed
   entries into `IndexMap` in authored order (first position, last value).
   Use an explicit owned work stack, NOT recursion, and an owned depth counter.
10. Depth accounting: top scalar 0, top container 1, a container is
    `1 + max(child container depth)`; accept 128, refuse 129 with
    `JsonHostError::Limit`. Enforce `max_array_size`/`max_map_size` per
     container (`0` = unlimited) with `Limit`. The map cap counts distinct stored
     keys, not raw duplicate occurrences; every occurrence must still validate.
11. Validate scalars: strings via `serde_json::from_str::<String>` (rejects
    unpaired surrogates, accepts `\/`); numbers via a hand-written
    `fn validate_number_token(token: &str) -> bool` implementing
    `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?` (do NOT use
    `serde_json::Number`, which rejects `1e400`). Any parse failure maps to
    `JsonHostError::Parse` with NO payload, so no input text and no line/column
    can leak.
12. Add `#[cfg(test)] mod tests` at the bottom of both files.

**Tests:** (module tests; run with `CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp cargo test -p camel-language-rhai --lib json::`)
- `token_projects_integer_forms`: assert `token_projects_to_i64("9223372036854775807") == Some(i64::MAX)`, `("-0") == Some(0)`, and `("1.0")`, `("1e2")`, `("18446744073709551615")` are all `None`. red: compile error; green: passes. (Scenario: integer forms project, decimals and exponents wrap.)
- `depth_counts_scalars_zero_containers_one`: assert scalar depth 0, `[1]` depth 1, `[[1]]` depth 2. red: compile error; green: passes.
- `parse_escaped_solidus`: `parse(r#"{"path":"a\/b"}"#, unlimited)` → object key `path` is `JsonKind::String("a/b")`. red: compile error; green: passes. (Scenario: escaped solidus accepted.)
- `parse_unpaired_surrogate_refused`: `parse(r#""\uD800""#, unlimited)` → `Err(JsonHostError::Parse)`. red: compile error; green: passes. (Scenario: unpaired surrogate refused.)
- `parse_integer_forms_project_decimals_wrap`: parse `[9223372036854775807,-0,1.0,1e2,18446744073709551615]`; `project_dynamic` yields native int for indices 0,1 and `JsonNumber` (via `Dynamic::is::<JsonNumber>()`) for 2,3,4. red: compile error; green: passes. (Scenario: integer forms project, decimals and exponents wrap.)
- `parse_large_magnitude_exact_tokens`: parse the three numbers from the spec; assert each stored `JsonNumber::as_str()` equals its source token (`18446744073709551615`, `123456789012345678901234567890`, `1e400`). red: compile error; green: passes. (Scenario: large magnitude round-trips exactly.)
- `parse_to_float_1e400_arithmetic`: `JsonNumber::new("1e400").to_f64()` → `Err(JsonHostError::Arithmetic)`. red: compile error; green: passes. (Scenario: to_float is explicit and checked.)
- `parse_authored_order_retained`: parse `{"z":1,"a":2,"m":3}`; object key iteration order is `z,a,m`. red: compile error; green: passes. (Scenario: authored order retained.)
- `parse_duplicate_key_first_position_last_value`: parse `{"b":1,"a":2,"b":3}`; keys are `b,a` and `b` holds `3`. red: compile error; green: passes. (Scenario: duplicate key keeps first position, last value.)
- `parse_overwritten_surrogates_refused`: setup unlimited limits; action parse `{"a":"\uD800","a":1}` and `{"a":"\uDC00","a":1}`; assert each returns payload-free `Err(JsonHostError::Parse)`. Command is the parser test filter above. Expected red: pre-validation dedup accepts both; green: validates overwritten strings and refuses both.
- `parse_overwritten_depth_limit`: setup unlimited limits and an object whose first `a` value contains 128 nested arrays, followed by `"a":1` (input container depth 129); action parse it; assert `Err(JsonHostError::Limit)`. A first `a` value with 127 nested arrays (input depth 128), followed by `"a":1`, must succeed and retain `a=1`. Command is the parser test filter above. Expected red: premature dedup accepts depth 129; green: all raw occurrences obey depth 128.
- `parse_duplicate_map_cap_counts_stored_keys`: setup map cap 1; action parse `{"a":1,"a":2}`; assert success with one key `a` and last value 2, while `{"a":1,"b":2}` fails Limit. Command is the parser test filter above. Expected before and after: pass; this regression pins existing stored-key cap accounting.
- `parse_depth_128_accepted_129_limit`: `parse("[".repeat(128) + &"]".repeat(128))` is `Ok`; 129 nested arrays is `Err(Limit)`. red: compile error; green: passes. (Scenario: depth boundary.)
- `parse_max_string_size_before_descent`: `JsonLimits { max_string_size: 4, ..unlimited }` with a longer input → `Err(Limit)`. red: compile error; green: passes.
- `parse_array_and_map_caps_limit`: `max_array_size: 2` refuses `[1,2,3]`; `max_map_size: 1` refuses `{"a":1,"b":2}` → `Err(Limit)`. red: compile error; green: passes. (Scenario: bounded by sandbox limits, caps.)
- `parse_zero_caps_are_unlimited`: `max_array_size: 0` accepts `[1,2,3]`; depth 128 invariant still enforced. red: compile error; green: passes. (Spec: zero keeps unlimited meaning.)
- `parse_malformed_redacted`: parse `"secret-token-xyz"` (invalid) → `Err(JsonHostError::Parse)`, which is payload-free — it carries NO input text and NO parser `line`/`column` representation (the script call-site `L:C` is attached only later by the host mapping, not by the parser). red: compile error; green: passes.

**Acceptance:**
- `cargo clippy -p camel-language-rhai -- -D warnings` exits 0 (with the env prefix).
- `cargo test -p camel-language-rhai --lib json::` passes all parser/tree tests.
- No `unwrap()`/`expect()`/`panic!` in non-test code in `src/json/` (`cargo xtask lint-unwrap` later confirms).
- `serde_json::Number`, `arbitrary_precision`, and `unbounded_depth` are NOT used anywhere in `src/json/`.

- [x] 1.2

### Task 1.3: Faithful bounded serializer

Implement the bounded tree serializer. No public registration yet.

**Files:**
- `crates/languages/camel-language-rhai/src/json/mod.rs` (modified)
- `crates/languages/camel-language-rhai/src/json/serialize.rs` (new)

**Steps:**
1. In `src/json/mod.rs` add `pub(crate) mod serialize;`.
2. In `src/json/serialize.rs` implement
   `pub(crate) fn to_json_string(value: &JsonValue, max_size: usize) -> Result<String, JsonHostError>`.
   `max_size` is the serialized-output bound (`0` = unlimited). Before each
   append, if `out.len() + chunk.len() > max_size` and `max_size != 0`, return
   `Err(JsonHostError::Limit)` and stop; never truncate.
3. `fn write_value(out: &mut String, value: &JsonValue, max_size: usize) -> Result<(), JsonHostError>`
   dispatches: `Null`→`null`; `Bool`→`true`/`false`; `Number(t)`→`t.as_str()`
   verbatim (exact token); `String(s)`→`write_string`; `Array`→`[..]` children
   in stored order; `Object`→`{..}` keys in `IndexMap` stored order.
4. `fn write_string(out, s)`: emit raw UTF-8 (non-ASCII bytes verbatim), escape
   only `"`, `\`, and U+0000–U+001F as `\b`/`\f`/`\n`/`\r`/`\t` or `\u00XX`.
   Do NOT escape `/`.
5. Add `#[cfg(test)] mod tests` with a helper `fn obj(pairs) -> JsonValue` and
   `fn arr(items) -> JsonValue` built from `JsonKind`.

**Tests:** (run with `CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp cargo test -p camel-language-rhai --lib json::serialize::`)
- `serialize_authored_order`: build `{"z":1,"a":2,"m":3}` (via `parse`) → `to_json_string(.., 0)` == `{"z":1,"a":2,"m":3}`. red: compile error; green: passes. (Scenario: authored order retained.)
- `serialize_duplicate_key_order_and_value`: `parse(r#"{"b":1,"a":2,"b":3}"#)` → `{"b":3,"a":2}`. red: compile error; green: passes. (Scenario: duplicate key keeps first position, last value.)
- `serialize_remove_keeps_remaining_order`: parse `{"a":1,"b":2,"c":3}`, `shift_remove("a")` on the object, serialize → `{"b":2,"c":3}`. red: compile error; green: passes. (Scenario: removing `a` keeps the order of the rest.)
- `serialize_large_magnitude_exact`: parse `[18446744073709551615,123456789012345678901234567890,1e400]` → serialize emits the three tokens verbatim with no `f64`. red: compile error; green: passes. (Scenario: large magnitude round-trips exactly.)
- `serialize_raw_utf8_no_slash_escape`: string value `café` and `a/b` → output contains literal `café` and literal `a/b` (assert `!out.contains("\\/")`). red: compile error; green: passes. (Scenario: raw UTF-8 and no slash escaping.)
- `serialize_control_chars_escaped`: string `"\u{1}"` → output contains `\u0001` and the raw control byte is absent. red: compile error; green: passes.
- `serialize_escapes_quote_and_backslash`: string `"` + `\` round-trips through `parse(to_json_string(..))`. red: compile error; green: passes.
- `serialize_bounded_stops_with_limit`: `to_json_string(big_value, 4)` → `Err(JsonHostError::Limit)` and the returned error is not a truncated string. red: compile error; green: passes. (Scenario: serializer is bounded.)

**Acceptance:**
- `cargo clippy -p camel-language-rhai -- -D warnings` exits 0 (with env prefix).
- `cargo test -p camel-language-rhai --lib json::serialize::` passes.

- [x] 1.3

### Task 1.4: Shared host module — registration, indexers, helpers, guards

Build the process-wide `rhai::Module` and the full wrapper surface. Still no
`create_base_engine` change (Task 1.5 wires it in).

**Files:**
- `crates/languages/camel-language-rhai/src/json/mod.rs` (modified)
- `crates/languages/camel-language-rhai/src/json/host.rs` (new)

**Steps:**
1. In `src/json/mod.rs` add `pub(crate) mod host;`.
2. At the top of `src/json/host.rs` define the crate-local alias
   `type RhaiResultOf<T> = Result<T, Box<rhai::EvalAltResult>>;` (rhai's own
   `RhaiResultOf` is private to the rhai crate). Implement
   `pub(crate) fn host_module() -> rhai::Shared<rhai::Module>` backed by a
   `static HOST: OnceLock<rhai::Shared<rhai::Module>>`; it calls `build_module()`
   once and returns a clone of the shared module.
3. `fn build_module() -> rhai::Module`: `Module::new()`, then
   `module.set_custom_type::<JsonValue>(JSON_VALUE_TYPE_NAME)` and
   `module.set_custom_type::<JsonNumber>(JSON_NUMBER_TYPE_NAME)`.
4. `fn limits_from(ctx: &rhai::NativeCallContext) -> parse::JsonLimits`: read
   `ctx.engine().max_string_size()`, `.max_array_size()`, `.max_map_size()`
   (the calling engine, never captured state).
5. `fn host_err(err: JsonHostError, actual: &str, pos: rhai::Position) -> Box<rhai::EvalAltResult>`:
   `Parse` → `EvalAltResult::ErrorParsing(rhai::ParseErrorType::BadInput(rhai::LexError::MalformedEscapeSequence("invalid JSON".to_string())), pos)`
   (static text only); `Limit` → `EvalAltResult::ErrorDataTooLarge("json limit exceeded".to_string(), pos)`;
   `TypeMismatch` → `EvalAltResult::ErrorMismatchDataType(actual.to_string(), "expected json".to_string(), pos)`;
   `Arithmetic` → `EvalAltResult::ErrorArithmetic("json number is not finite".to_string(), pos)`.
   Call sites pass `actual` as a static Rhai type name (`d.type_name()`, `"json value"`, `"json number"`), never exchange data.
6. Register `parse_json` via
   `module.set_native_fn("parse_json", |ctx: NativeCallContext, s: ImmutableString| -> RhaiResultOf<Dynamic> { let limits = limits_from(&ctx); let value = parse::parse(&s, &limits).map_err(|e| host_err(e, "json input", ctx.call_position()))?; Ok(project_dynamic(&value)) })`.
   Root `null` therefore returns unit.
7. Register `to_json` as EXACT typed overloads — a single `Dynamic` overload
   does NOT shadow Rhai's exact stock `Map` method, so the native-`Map` overload
   MUST be exact. Define one converter
   `fn serialize_dynamic(ctx: &NativeCallContext, d: Dynamic) -> RhaiResultOf<String>`:
   `let limits = limits_from(ctx); let tree = dynamic_to_json(&d, &limits, 0).map_err(|e| host_err(e, d.type_name(), ctx.call_position()))?; serialize::to_json_string(&tree, ctx.engine().max_string_size()).map_err(|e| host_err(e, "json value", ctx.call_position()))`.
   Register, each delegating to `serialize_dynamic(&ctx, Dynamic::from(v))`:
   - `to_json(Map)` — EXACT native-`Map` overload
     `|ctx: NativeCallContext, m: &mut rhai::Map|` (shadows the stock Map method;
     serialize a clone through the shared converter);
   - `to_json(Array)` — `|ctx: NativeCallContext, a: rhai::Array|`;
   - `to_json(JsonValue)` and `to_json(JsonNumber)`;
   - `to_json(String)`, `to_json(bool)`, `to_json(i64)`, `to_json(f64)`,
     `to_json(())` (unit).
   A `Dynamic` argument is NOT used as the native-`Map`/`Array` overload because
   Rhai prefers the exact stock overload over a `Dynamic` one. Both the function
   form `to_json(x)` and the method form `x.to_json()` resolve to these
   registrations (Requirement 8: registered for Map, Array, JsonValue,
   JsonNumber, and the scalar types, function and method form).
   Convert `d` to a tree with `fn dynamic_to_json(d: &Dynamic, limits: &JsonLimits, depth_budget: usize) -> Result<JsonValue, JsonHostError>`
   (`JsonValue`/`JsonNumber`/String/bool/i64/finite f64/unit/Map/Array accepted;
   non-finite float, `FnPtr`, timestamp, and any other custom type →
   `TypeMismatch` with no Debug/Display fallback).
8. Register the wrapper surface on `JsonValue`:
   - `set_indexer_get_fn`/`set_indexer_set_fn` for `ImmutableString` (string key)
     and for `i64` (numeric index). Getters return `RhaiResultOf<Dynamic>` via
     the shared projection; a missing key and a JSON null both return unit.
     Setters are the ONLY setter overloads: `|ctx, obj: &mut JsonValue, idx, val: Dynamic|`
     validate the value type internally and return
     `RhaiResultOf<()>`; a wrong-container target, an unsupported value type, or
     an out-of-range mutation limit returns `TypeMismatch`/`Limit` — NEVER
     `ErrorIndexingType`.
   - `set_native_fn("len", |obj: &mut JsonValue| -> RhaiResultOf<i64>)` (object
     entries or array length).
   - `set_native_fn("contains", |obj: &mut JsonValue, key: ImmutableString| -> RhaiResultOf<bool>)`
     (objects only; an array target → `TypeMismatch`).
   - `set_native_fn("keys", |obj: &mut JsonValue| -> RhaiResultOf<Array>)`
     (stored-string order).
   - `set_native_fn("remove", |ctx, obj: &mut JsonValue, idx: Dynamic| -> RhaiResultOf<Dynamic>)`
     shift-remove for a string key; a removed key re-inserted later goes to the
     end; absent key/index or non-array/object target as specified; returns the
     removed value or unit.
   - `set_native_fn("push", |ctx, obj: &mut JsonValue, val: Dynamic| -> RhaiResultOf<()>)`
     (array only; appends, rechecks `max_array_size` and depth).
    - `set_native_fn("to_string", |ctx: NativeCallContext, v: &mut JsonValue| -> RhaiResultOf<String>)`
      and `set_native_fn("to_debug", |ctx: NativeCallContext, v: &mut JsonValue| -> RhaiResultOf<String>)`
      on `JsonValue`, and the same two names on `JsonNumber`, each returning the
      compact JSON (for `JsonNumber`, the token), enforcing the calling engine's
      string cap through the same bounded serializer as `to_json`.
   - `set_native_fn("to_float", |num: &mut JsonNumber| -> RhaiResultOf<f64>)`.
9. Mutation rules, implemented as pure helpers that take `&JsonLimits` (so they
   are directly unit-testable) and delegated to by the registered indexer setter
   and the `push`/`remove` functions:
   - `enum JsonIndex { Key(ImmutableString), Index(i64) }`
   - `fn set_at(obj: &mut JsonValue, index: JsonIndex, value: &Dynamic, limits: &JsonLimits) -> Result<(), JsonHostError>`
   - `fn push_to(obj: &mut JsonValue, value: &Dynamic, limits: &JsonLimits) -> Result<(), JsonHostError>`
   - `fn remove_at(obj: &mut JsonValue, index: JsonIndex) -> Result<Dynamic, JsonHostError>`
   Registered closures read `limits_from(&ctx)` and delegate; the rule below
   applies inside the helpers:
   - value type accepted = unit, bool, i64, finite f64, String, `JsonNumber`,
     `JsonValue`, native `Map`, native `Array`; unit stores `Null`.
   - integer-index reads/writes: negative counts from the end; out-of-range
     read → `EvalAltResult::ErrorArrayBounds(len, idx, pos)`.
    - depth invariant: build a candidate kind, compute its exact depth through
      `metrics`, and refuse `> MAX_JSON_DEPTH` before applying. Do not add the
      existing subtree depth to the inserted value depth: unrelated siblings
      do not deepen the insertion path. Self-assignment at the cap is refused.
   - size/cap invariant: recheck array/map caps and the total size estimate
     against `max_string_size` (0 = unlimited) before applying.
   - validate the value fully BEFORE mutating so a failed setter rolls back.
   - use `JsonValue::mutate` (`Arc::make_mut`) for copy-on-write.
10. Comparison guards: for each registered pair register `==`, `!=`, `<`, `>`,
    `<=`, `>=` via `module.set_native_fn(op, |ctx: NativeCallContext, _a, _b| -> RhaiResultOf<bool> { Err(host_err(JsonHostError::TypeMismatch, "json value", ctx.call_position())) })`
    (a `NativeCallContext` first parameter supplies `ctx.call_position()`; bind the
    two operands with the exact wrapper/scalar types of the pair), for both operand orders
    of (`JsonValue`|`JsonNumber`) × (`i64`,`f64`,`String`,`bool`) and between
     (`JsonValue`,`JsonNumber`) in both orders, plus (`JsonValue`,`JsonValue`)
     and (`JsonNumber`,`JsonNumber`). No numeric comparison logic
    exists. (Do NOT register comparisons with unit — unit is outside the
    registered set and keeps Rhai builtin behavior.)
11. Add `#[cfg(test)] mod tests` with helpers
   `fn host_engine() -> Engine` (`Engine::new_raw()`,
   `StandardPackage::new().register_into_engine(&mut engine)`,
   `engine.register_global_module(host_module())`, generous limits),
   `fn unlimited() -> JsonLimits` (all three caps `0`), and
   `fn parsed(s: &str) -> JsonValue` (`parse::parse(s, &unlimited()).unwrap()`).

**Tests:** (run with `CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp cargo test -p camel-language-rhai --lib json::host::`)
- `host_module_is_single_instance`: `rhai::Shared::ptr_eq(&host_module(), &host_module())` is true. red: compile error; green: passes.
- `host_module_registers_types`: `host_module().get_custom_type_display::<JsonValue>() == Some(JSON_VALUE_TYPE_NAME)` and `host_module().get_custom_type_display::<JsonNumber>() == Some(JSON_NUMBER_TYPE_NAME)`. red: compile error; green: passes.
- `host_shadows_stock_parse_json`: on `host_engine()`, `eval::<String>("type_of(parse_json(\"{}\"))")` `== "json value"` (assert the CAST string; `type_of` returns a Dynamic holding a String, so do NOT call `Dynamic::type_name`). red: compile error; green: passes. (Scenario: host helpers win.)
- `host_shadows_stock_to_json`: on `host_engine()`, `eval::<String>("to_json(parse_json(\"[1,2]\"))")` == `[1,2]`. red: compile error; green: passes.
- `host_shadows_stock_to_json_map_nonfinite`: on `host_engine()`, `eval::<String>("to_json(#{x: 0.0/0.0})")` → `Err` whose boxed inner matches `EvalAltResult::ErrorMismatchDataType(..)` (the EXACT native-`Map` overload shadows the stock Map method; a `Dynamic` overload would not). red: compile error; green: passes. (Scenario: non-finite float is refused.)
- `host_indexer_roundtrip_smoke`: `let j = parse_json("{\"a\":1}"); j["a"] = 2; to_json(j)` == `{"a":2}`. red: compile error; green: passes.
- `host_compare_guard_smoke`: on `host_engine()`, loop all six comparison operators and all 20 registered pairs (16 wrapper/scalar operand orders, two cross-wrapper orders, and the two same-wrapper pairs), using `{}` and `1.5` parsed wrappers and native integer/float/string/bool values. Assert every evaluation fails with `EvalAltResult::ErrorMismatchDataType`, including `parse_json("{}") == parse_json("{}")` and `parse_json("1.5") < parse_json("1.5")`. Command is the host test filter above; red: missing same-wrapper guards yield `ErrorFunctionNotFound`; green: all 120 cases refuse with the correct class.
- `host_run_with_scope_push_rollback`: setup `let mut engine = host_engine(); engine.set_max_array_size(2); let original = parsed("[1,2]"); let mut scope = Scope::new(); scope.push("j", original.clone());` → action `let err = engine.run_with_scope(&mut scope, "j.push(3)").unwrap_err();` → assert the inner is `EvalAltResult::ErrorDataTooLarge(..)` AND `scope.get_value::<JsonValue>("j").unwrap()` serializes to the same JSON as `original` (limit aborts the whole run; `ErrorDataTooLarge` is not catchable in-script, so the unchanged check is observed from the scope). red: compile error; green: passes. (Requirement 9: exceeding a bound commits no change.)
- `host_set_at_depth_limit_unchanged`: construct a valid object of depth 127; directly call `set_at` at a root key with a depth-128 array value (limits `unlimited()`). Candidate depth is 129: assert `Err(JsonHostError::Limit)` and serialization unchanged. Command is the host test filter above; red: absent guard accepts invalid depth; green: refuses without mutation.
- `host_set_at_exact_depth_boundary`: setup a parsed object containing a depth-6 subtree (object depth 7). Insert a depth-127 array at a different root key with unlimited limits; assert success and exact depth 128. Replace the old deep child with a depth-1 array and assert success. Command is the host test filter above; red: whole-subtree-depth addition refuses the valid insertion; green: both mutations succeed.
- `host_string_methods_enforce_exact_output_limit`: setup `host_engine()` with string cap 9, parse `[1,2,3,4]` (9 bytes), then lower engine cap to 8 via Rust and retain the wrapper in a Scope. Evaluate `j.to_string()` and `j.to_debug()` separately and assert each refuses with `ErrorDataTooLarge`; raising the cap to 9 permits exactly `[1,2,3,4]`. Command is the host test filter above; red: the context-taking `to_string`/`to_debug` overloads do not exist yet (compile error). Note: Rhai's post-call `check_data_size` also refuses the over-cap result, so the cap-8 refusal is a backstop-consistent assertion. The 9-byte success at cap 9 pins the exact boundary. green: both methods refuse at cap 8 with `ErrorDataTooLarge` and return exactly `[1,2,3,4]` at cap 9.
- `host_set_at_size_limit_unchanged`: directly call `set_at` with `JsonLimits { max_string_size: 8, ..unlimited() }` on `parsed("{}")` for key `"key"`/value `"value"` → `Err(JsonHostError::Limit)` and the object serializes unchanged. red: compile error; green: passes. (Underlying size check.)
- `host_push_cap_limit_unchanged`: directly call `push_to(&mut parsed("[1,2]"), &Dynamic::from(3_i64), &JsonLimits { max_array_size: 2, ..unlimited() })` → `Err(JsonHostError::Limit)` and the array serializes `[1,2]`. red: compile error; green: passes. (Underlying array-cap check.)
- `host_set_at_wrong_container_type_mismatch`: directly call `set_at(&mut parsed("{}"), JsonIndex::Index(0), &Dynamic::from(1_i64), &unlimited())` → `Err(JsonHostError::TypeMismatch)`. red: compile error; green: passes.

**Acceptance:**
- `cargo clippy -p camel-language-rhai -- -D warnings` exits 0 (with env prefix).
- `cargo test -p camel-language-rhai --lib json::host::` passes.
- No public API signature exposes a `json` type (the module stays `mod json;`).
- `grep -n "unwrap\|expect\|panic" src/json/host.rs` shows no non-test production hits.

- [x] 1.4

### Task 1.5: Engine integration, outbound refusal, regression suite, perf check

Wire the module into every sandbox engine, add the converter refusal arms, and
pin every spec scenario through `RhaiLanguage`. Then re-run the Task 1.1
benchmark and compare.

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)
- `crates/languages/camel-language-rhai/src/converter.rs` (modified)
- `crates/languages/camel-language-rhai/src/json/mod.rs` (modified: remove transitional dead-code allowance)
- `crates/languages/camel-language-rhai/src/json/host.rs` (modified: host-layer array-bounds test)

**Steps:**
1. In `RhaiLanguage::create_base_engine`, immediately after
   `StandardPackage::new().register_into_engine(&mut engine);`, call
   `engine.register_global_module(json::host::host_module());` (Rhai inserts at
   index 1, so the host module precedes `StandardPackage`).
2. In `converter.rs::dynamic_to_value`, BEFORE the generic fallback, add
   `} else if d.is::<crate::json::JsonValue>() { Err(refused(JSON_VALUE_TYPE_NAME)) } else if d.is::<crate::json::JsonNumber>() { Err(refused(JSON_NUMBER_TYPE_NAME)) }`.
   Use the `json value`/`json number` constants; add no other converter change.
   `json_to_dynamic` and `rhai_map_to_value_map` are untouched.
3. Add engine-level regression tests to `lib.rs` `mod tests`, with helper
   `async fn eval_script(script: &str, ex: &Exchange) -> Result<Value, LanguageError>`
   (build `RhaiLanguage::new().create_expression(script)`, call `evaluate(ex)`),
   and helper `fn eval_err_class(err: &LanguageError) -> ExpressionErrorClass`.
4. Add a mutual helper `fn json_script_out(script: &str) -> String` returning
   `Value::String` from a successful eval.
5. Remove the transitional `#![allow(dead_code)]` from `json/mod.rs` now that
   production engines use the module; update its module comment accordingly.

**Tests:** (run with `CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp cargo test -p camel-language-rhai --lib`)

Red/green interpretation: new helper symbols are absent before this task.
Without host registration, type labels, exact large numbers, authored order,
depth limits and the comparison matrix discriminate the implementation. Other
scenario tests may already pass on stock Rhai; their purpose is regression
coverage, and they are not evidence that registration works.

- `host_get_out_of_range_error_array_bounds` (in `json/host.rs`): setup `host_engine()`, action evaluate `let j = parse_json("[1]"); j[5]`, assert the boxed error is `EvalAltResult::ErrorArrayBounds(1, 5, _)`. Command: `cargo test -p camel-language-rhai --lib host_get_out_of_range_error_array_bounds` with the fixed environment. Expected: existing implementation may already pass; test pins the exact variant invisible at the LanguageError boundary.

Integration / requirement 1 (every engine):
- `json_host_helpers_win_on_expression_engine`: `create_expression("type_of(parse_json(\"{}\"))")` evaluates to `Value::String("json value")` (assert the cast `Value::String`; `type_of` returns a Rhai String, not a `Dynamic` whose `type_name` is meaningful). red: compile error; green: passes. (Scenario: host helpers win.)
- `json_host_helpers_win_on_mutating_engine`: `create_mutating_expression("type_of(parse_json(\"[]\"))")` returns `Value::String("json value")`. red: compile error; green: passes.
- `json_host_helpers_win_on_predicate_engine`: `create_predicate("type_of(parse_json(\"{}\")) == \"json value\"")` matches. red: compile error; green: passes.

Parse / serialization scenarios:
- `json_escaped_solidus_accepted`: `json_script_out(r#"to_json(parse_json("{\"path\":\"a\\/b\"}"))"#)` contains `"path":"a/b"`. red: compile error; green: passes. (Scenario: escaped solidus accepted.)
- `json_large_magnitude_round_trips_exact`: `to_json(parse_json("[18446744073709551615,123456789012345678901234567890,1e400]"))` emits the three tokens verbatim. red: compile error; green: passes. (Scenario: large magnitude.)
- `json_integer_forms_project_decimals_wrap`: script `let j = parse_json("[9223372036854775807,-0,1.0,1e2,18446744073709551615]"); [type_of(j[0]),type_of(j[1]),type_of(j[2]),type_of(j[3]),type_of(j[4])]` yields `Value::Array` == `[Value::String("i64"),Value::String("i64"),Value::String("json number"),Value::String("json number"),Value::String("json number")]`. red: compile error; green: passes. (Scenario: integer forms project, decimals and exponents wrap.)
- `json_to_float_1e400_is_arithmetic`: `parse_json("1e400").to_float()` → `EvalFailure { class: Arithmetic }`. red: compile error; green: passes. (Scenario: to_float.)
- `json_authored_order_retained`: `to_json(parse_json("{\"z\":1,\"a\":2,\"m\":3}"))` == `{"z":1,"a":2,"m":3}`. red: compile error; green: passes. (Scenario: authored order.)
- `json_duplicate_key_keeps_first_position_last_value`: `to_json(parse_json("{\"b\":1,\"a\":2,\"b\":3}"))` == `{"b":3,"a":2}`. red: compile error; green: passes. (Scenario: duplicate key.)
- `json_remove_a_keeps_order`: `let j = parse_json("{\"a\":1,\"b\":2,\"c\":3}"); j.remove("a"); to_json(j)` == `{"b":2,"c":3}`. red: compile error; green: passes. (Scenario: removing a.)
- `json_round_trip_equivalent_not_byte_identical`: `to_json(parse_json("{ \"a\" : 1 , \"b\" : \"x\\/y\" }"))` parses back to the same value with keys `a,b` (whitespace may differ). red: compile error; green: passes. (Scenario: equivalent, not byte-identical.)

Copy-on-write / type names:
- `json_bare_read_isolated`: `let j = parse_json("{\"a\":{\"b\":1}}"); let x = j["a"]; x["b"] = 9; to_json(j)` contains `"b":1`. red: compile error; green: passes. (Scenario: bare read is isolated.)
- `json_type_of_stable_labels`: `type_of(parse_json("{}"))` yields `Value::String("json value")`; `type_of(parse_json("1.5"))` yields `Value::String("json number")` (assert the cast string value, not `Dynamic::type_name`). red: compile error; green: passes. (Scenario: type_of uses stable labels.)

Mutation / indexing:
- `json_chained_dot_negative_index`: `let j = parse_json("{\"a\":[{\"b\":1}]}"); j["a"][0]["b"] = 2; j.a[-1].b = 3; to_json(j)` == `{"a":[{"b":3}]}`. red: compile error; green: passes. (Scenario: chained, dot, negative index.)
- `json_out_of_range_index_error_array_bounds`: `let j = parse_json("[1]"); j[5]` → `EvalFailure { class: Runtime }`. The LanguageError boundary carries only the class; the inner `ErrorArrayBounds` variant is pinned by `host_get_out_of_range_error_array_bounds`. green: passes. (Scenario: chained/negative index, out-of-range.)
- `json_missing_key_versus_null`: for `{"n":null}`, `j["n"]==()`, `j["m"]==()`, `contains("n")==true`, `contains("m")==false`, `"m" in j == false`. red: compile error; green: passes. (Scenario: missing key versus null.)
- `json_contains_on_array_type_mismatch`: `parse_json("[1]").contains("k")` → `EvalFailure { class: TypeMismatch }`. red: compile error; green: passes. (Scenario: contains on array.)
- `json_wrong_container_getter_type_mismatch`: `parse_json("[1]")["x"]` (string key on an array) and `parse_json("{}")[0]` (integer index on an object) each → `EvalFailure { class: TypeMismatch }`. red: compile error; green: passes. (Requirement 6: wrong-container reads are type-mismatch.)
- `json_push_grows_remove_returns`: `let j = parse_json("[1,2]"); j.push(3); let r = j.remove(0); to_json(j) == "[2,3]" && r == 1 && j.remove(9) == ()`. red: compile error; green: passes. (Scenario: push grows and remove returns; absent remove is unit.)
- `json_unit_assignment_stores_null`: `let j = parse_json("{}"); j["k"] = (); to_json(j)` contains `"k":null`. red: compile error; green: passes. (Scenario: unit assignment stores null.)
- `json_nested_setter_failure_rolls_back`: `let j = parse_json("{\"a\":{\"b\":1}}"); try { j["a"]["b"] = || 1; } catch (e) {} to_json(j)` still contains `"b":1`. red: compile error; green: passes. (Scenario: nested setter failure rolls back.)

Comparisons:
- `json_all_six_comparisons_refuse_type_mismatch`: for each op in `==`, `!=`, `<`, `>`, `<=`, `>=`, and for both operand orders of a `JsonNumber`/`JsonValue` against `i64`, `f64`, `String`, `bool`, and wrapper/wrapper, the eval fails with `EvalFailure { class: TypeMismatch }`. This is the approved refusal contract; it asserts NO operator returns a bool. red: compile error; green: passes. (Scenarios: mixed numeric comparison refuses; requirement 7 fully.)
- `json_null_compares_with_unit`: `parse_json("null") == ()` is `true` (unit is outside the registered set). red: compile error; green: passes. (Scenario: null compares with unit.)

Serialization:
- `json_native_map_serializes_sorted_nested_inline`: `let p = parse_json("{}"); to_json(#{"b":1,"a":p})` == `{"a":{},"b":1}` (native Map keys sorted, wrapper inlined). red: compile error; green: passes. (Scenario: native containers serialize sorted with nested wrappers.)
- `json_function_and_method_forms_agree`: `to_json(j) == j.to_json()` for a parsed value. red: compile error; green: passes. (Scenario: function and method forms.)
- `json_raw_utf8_no_slash_escape`: `to_json(parse_json("{\"s\":\"café\",\"p\":\"a/b\"}"))` contains literal `café` and literal `a/b`, and `!contains("\\/")`. red: compile error; green: passes. (Scenario: raw UTF-8 and no slash escaping.)
- `json_nonfinite_float_refused`: `to_json(#{"x": 0.0/0.0})` → `EvalFailure { class: TypeMismatch }`, and the evaluated error string does not contain `NaN`. red: compile error; green: passes. (Scenario: non-finite float is refused.)
- `json_unsupported_value_refused_type_mismatch`: for four scripts — `fn secret_fn_name() { 1 } to_json([Fn("secret_fn_name")])`, `fn secret_fn_name() { 1 } to_json(#{"f": Fn("secret_fn_name")})`, `to_json([timestamp()])`, and `to_json(#{"t": timestamp()})` — each eval returns `EvalFailure { class: TypeMismatch, detail: None }`; for the two `Fn("secret_fn_name")` scripts, `format!("{err}")` does NOT contain `secret_fn_name` (no leaked value / no Debug fallback). red: compile error; green: passes. (Requirement 8: function pointers and custom values — timestamp — nested in a native `Array` or `Map` are refused `type-mismatch` without including the value.)
- `json_to_string_and_to_debug_are_compact_json`: `parse_json("{\"a\":1}").to_string() == parse_json("{\"a\":1}").to_debug()` and both equal compact JSON. red: compile error; green: passes. (Requirement 8.)

Bounds:
- `json_depth_boundary`: `to_json(parse_json(s))` on 128 nested arrays evaluates to a JSON string; 129 → `EvalFailure { class: Limit }`. Serialization avoids the intentional outbound wrapper refusal. green: passes. (Scenario: depth boundary.)
- `json_native_container_depth_limit`: for native `Map` (`let m = #{}; let n = 0; while n < 127 { m = #{ "x": m }; n += 1; } to_json(m)`) and native `Array` (`let a = []; let n = 0; while n < 127 { a = [a]; n += 1; } to_json(a)`) the eval returns `Value::String` at depth 128; the same scripts with `< 128` (depth 129) each → `EvalFailure { class: Limit }`. This pins the native-container `to_json` depth invariant for Maps as well as Arrays, matching parsing. red: compile error; green: passes. (Requirement 9: every `to_json` of a native container checks the depth invariant; scenario: depth boundary.)
- `json_mutation_depth_and_cap_rechecked`: with `RhaiLimitsConfig { max_array_size: Some(2), .. }`, `let j = parse_json("[1,2]"); j.push(3)` → `EvalFailure { class: Limit }` ONLY. Assert the class; do NOT attempt an in-script unchanged check (`ErrorDataTooLarge` is not catchable, and no partial-commit observation is possible from the script result). The observable unchanged/rollback check lives in Task 1.4 `host_run_with_scope_push_rollback` plus the underlying `set_at`/`push_to` helper tests. red: compile error; green: passes. (Scenario: mutation depth and cap are rechecked.)
- `json_mutation_size_limit`: with `RhaiLimitsConfig { max_string_size: Some(8), .. }`, `let j = parse_json("{}"); j["key"] = "value";` → `EvalFailure { class: Limit }` ONLY (same not-catchable reasoning; rollback is pinned by `host_set_at_size_limit_unchanged`). red: compile error; green: passes. (Requirement 9: a mutation exceeding `max-string-size` fails with `limit` before applying.)
- `json_self_assignment_at_cap_refused`: parse a wrapper whose `["a"]` subtree reaches depth 128, then `j["a"] = j` → `EvalFailure { class: Limit }` before a stack crash (class assertion only; underlying depth refusal pinned by `host_set_at_depth_limit_unchanged`). red: compile error; green: passes. (Scenario: self-assignment at cap is refused.)
- `json_limits_read_from_calling_engine`: `RhaiLanguage::with_limits(RhaiLimitsConfig { max_array_size: Some(2), .. }).create_expression("to_json(parse_json(\"[1,2,3]\"))")` → `Limit`; the default-limits language returns `Value::String("[1,2,3]")`. green: passes. (Requirement: limits read from the calling engine.)
- `json_zero_limit_is_unlimited`: `RhaiLimitsConfig { max_array_size: Some(0), .. }` evaluates `to_json(parse_json("[1,2,3]"))` to a JSON string; depth 128 still enforced. green: passes. (Spec: zero keeps unlimited meaning.)

Errors:
- `json_arithmetic_and_helpers_function_not_found`: each of `parse_json("1.5") + 1`, `let j = parse_json("1.5"); j += 1`, `parse_json("{}").values()`, `parse_json("{}").merge(#{})` → `EvalFailure { class: FunctionNotFound }`. The `+=` case binds `j` first because `parse_json("1.5") += 1` is an invalid assignable-lhs parse, not a runtime classification. red: compile error; green: passes. (Scenario: arithmetic and container helpers are function-not-found.)
- `json_iteration_runtime_error`: `for k in parse_json("{}") {}` → `EvalFailure { class: Runtime }`. red: compile error; green: passes. (Scenario: iteration is a runtime error.)
- `json_setter_failure_propagates`: `let j = parse_json("{}"); j["k"] = || 1;` → `EvalFailure { class: TypeMismatch }`, discriminating from an unregistered setter or `ErrorIndexingType`, which would surface as Runtime. The eval returns Err, so failure was not discarded. green: passes. (Scenario: setter failure propagates.)
- `json_parse_error_is_parse_class_with_position`: malformed input holding a secret string → `EvalFailure { class: Parse, position: Some(_), detail: None }`. `format!("{err}")` MAY end at the script call site `line:column` (allowed) but MUST NOT contain the parser wording substrings `"line "` or `"column"`, and MUST NOT contain the secret. red: compile error; green: passes. (Scenario: parse error is redacted.)

Converter boundary:
- `json_inbound_u64_property_still_refused`: an Exchange with a property holding `Value::from(u64::MAX)` → `eval_script("property(\"big\")", &ex)` → `LanguageError::ConversionError`, class `Conversion` (existing inbound behavior unchanged). This existing behavior is expected-green before registration. (Scenario: exchange inbound behavior is unchanged.)
- `json_explicit_serialization_yields_text_body`: `create_mutating_expression("body = to_json(parse_json(\"{\\\"a\\\":1}\"))")` applied to an Exchange; the resulting `exchange.input.body` is `Body::Text` holding valid JSON `{"a":1}`. red: compile error; green: passes. (Scenario: explicit serialization yields a text body.)

Converter unit tests in `converter.rs`:
- `json_value_refused_outbound_stable_label`: `dynamic_to_value(Dynamic::from(JsonValue::new(JsonKind::Null)), "value")` → `ConversionError { source_type: "json value", .. }`, no Rust type path. red: compile error; green: passes. (Scenario: outbound refusal uses stable labels.)
- `json_number_refused_outbound_stable_label`: same for `JsonNumber::new("1")` → `source_type == "json number"`. red: compile error; green: passes. (Scenario: outbound refusal uses stable labels.)
- `u64_above_i64_max_refused_inbound` (existing) stays green.

Perf re-measure:
- Re-run the Task 1.1 command. Record `small_ns`/`big_ns` as the post-change median.

**Acceptance:**
- All tests above pass: `CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp cargo test -p camel-language-rhai --lib` exits 0.
- `CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp cargo clippy -p camel-language-rhai -- -D warnings` exits 0.
- Perf comparison recorded: take the post-change `small_ns`/`big_ns`. If either phase exceeds `1.15 ×` the Task 1.1 baseline, re-run the Task 1.1 command up to TWO additional times for 3 post runs total and use the LOWEST median per phase. Report `park: perf regression` ONLY when all 3 post-run medians for that phase exceed `1.15 ×` the baseline. `big_ns >= small_ns` must still hold across the chosen medians (the `rc-m01r9` path is untouched, not masked).
- THIS TASK's `git diff` touches only `src/lib.rs`, `src/converter.rs`, `src/json/mod.rs` (transitional allowance removal), and `src/json/host.rs` (array-bounds test); do NOT evaluate this against the cumulative branch diff.

- [x] 1.5

### Task 1.6: Operator docs, crate CONTEXT, and final gate evidence

Document the surface and the compatibility break, then run the mandatory mission
gate block and record exact exit codes.

**Files:**
- `docs/src/languages/rhai.md` (modified)
- `crates/languages/camel-language-rhai/CONTEXT.md` (modified)
- `crates/languages/camel-language-rhai/src/lib.rs` (modified: gate-only test metadata and private doc-link repairs)

**Steps:**
1. In `docs/src/languages/rhai.md` add a "JSON helpers" section: `parse_json`
   is strict RFC 8259 (accepts `\/`, refuses unpaired surrogates with a
   redacted parse error) and stores exact number tokens; integer-form tokens
   project to native `INT` while decimals, exponents, and out-of-i64 integers
   are `json number`; `to_json`/`to_string`/`to_debug` re-emit compact JSON with
   raw UTF-8, no `/` escaping, control-character escaping, and insertion order;
   native `Map` keys are sorted. Document the wrapper surface (`len`, `contains`
   objects-only, `keys`, `remove`, `push`, indexing/chained/dot, negative
   indexes) and the depth-128 + cap rules.
2. Document the compatibility break explicitly: decimals/exponents are now
   `json number` (not `f64`); all six registered comparisons fail
   `type-mismatch` (use `JsonNumber.to_float()` for explicit lossy comparison);
   `set_body(parse_json(j))` now fails (direct outbound wrapper conversion is
   deferred) so persistence uses `to_json` or a native leaf. State the
   round-trip caveat: semantic, not byte-for-byte; native i64 spelling may
   normalize (`-0` → `0`).
3. In `crates/languages/camel-language-rhai/CONTEXT.md` add the shared host
   JSON module to the sandbox/module description, list the two stable outbound
   labels `json value`/`json number`, and reference `rc-qka42` (and note
   `rc-m01r9`/`rc-141om` remain out of scope).
4. Repair gate-only issues in `lib.rs`: rename the `secret` format capture in
   `json_parse_error_is_parse_class_with_position` to `canary`, retaining its
   literal and no-leak assertion; use the ADR-0054 `slow test:` benchmark ignore
   label above; replace the public intra-doc link to private
   `RhaiMutatingExpression` with inline code. These repairs change no runtime
   behavior. Re-run lint-secrets, lint-ignore, the redaction test, and rustdoc.
5. Run and record the mandatory gate block below.

**Tests:** (documentation + evidence task; the executable checks are the gates)
- `doc_build_json_helpers`: `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-language-rhai --no-deps` exits 0. This validates crate rustdoc, not mdbook text; review the operator JSON section in `docs/src/languages/rhai.md` separately against the spec. red: private intra-doc link fails; green: exits 0 after inline-code correction.

**Acceptance — mandatory mission gates.** Run every command from `$WT` with
`CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp`, each
as its own command, and record the exit code:
- `df -h "$PWD" | awk 'NR==2{print $5}'`; if `>= 78%`, STOP and report `park: disk usage`.
- `cargo fmt --check --all`
- `cargo build --workspace`
- `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings`
- `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`
- `cargo clippy -p camel-cli -- -D warnings`
- `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings`
- `cargo test --workspace --lib`
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`
- `cargo test -p camel-language-rhai --lib`
- `cargo test -p camel-language-rhai --release --lib bench_json_host_per_eval_cost -- --ignored --nocapture`
- `cargo xtask lint-unwrap`
- `cargo xtask lint-secrets`
- `cargo xtask lint-single-source`
- `cargo xtask lint-non-exhaustive`
- `cargo xtask lint-log-levels`
- `cargo xtask lint-log-redaction`
- `cargo xtask lint-cancel-tokens`
- `cargo xtask lint-test-sleep`
- `cargo xtask lint-unbounded-wait`
- `cargo xtask lint-ignore`
- `cargo xtask lint-publish-cycles`
- `cargo xtask lint-publish-registration`
- `cargo xtask lint-component-deps`
- `cargo xtask lint-gate-forwarding`
- `cargo xtask lint-context-citations`
- `cargo xtask lint-metric-labels`
- `cargo xtask schema --check`
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint --no-deps`
- `cargo audit`
- Commit-diff gate (local base, NO `git fetch`, NO remote): `cargo xtask changelog --check --from c6c5f203 --to HEAD`
- `openspec validate rhai-json-fidelity --type change --json`

Recording contract: every gate is enumerated with `pass`/`fail`, or marked
`N/A — no Rust changed` (not applicable here: Rust changed), or
`integration-verification-deferred-to-CI`. `lint-commits` is replaced by the
local base-c6c5f203 changelog command above; no gate may `git fetch` or touch
the main checkout.

- [x] 1.6

## Spec coverage map

| Spec scenario | Owning test(s) |
|---|---|
| Host helpers win on every engine | `json_host_helpers_win_on_expression_engine`, `json_host_helpers_win_on_mutating_engine`, `json_host_helpers_win_on_predicate_engine`, `host_shadows_stock_parse_json` |
| Escaped solidus accepted | `parse_escaped_solidus`, `json_escaped_solidus_accepted` |
| Unpaired surrogate refused | `parse_unpaired_surrogate_refused`, `json_parse_error_is_parse_class_with_position` |
| Integer forms project, decimals wrap | `token_projects_integer_forms`, `parse_integer_forms_project_decimals_wrap`, `json_integer_forms_project_decimals_wrap` |
| Large magnitude round-trips | `parse_large_magnitude_exact_tokens`, `serialize_large_magnitude_exact`, `json_large_magnitude_round_trips_exact` |
| to_float explicit and checked | `parse_to_float_1e400_arithmetic`, `json_to_float_1e400_is_arithmetic` |
| Authored order retained | `parse_authored_order_retained`, `serialize_authored_order`, `json_authored_order_retained` |
| Duplicate key keeps first position, last value | `parse_duplicate_key_first_position_last_value`, `serialize_duplicate_key_order_and_value`, `json_duplicate_key_keeps_first_position_last_value` |
| Removing a keeps the order of the rest | `serialize_remove_keeps_remaining_order`, `json_remove_a_keeps_order` |
| Bare read is isolated | `json_bare_read_isolated` |
| type_of uses stable labels | `json_type_of_stable_labels`, `host_module_registers_types` |
| Chained, dot, negative-index | `json_chained_dot_negative_index`, `host_indexer_roundtrip_smoke` |
| Out-of-range index `ErrorArrayBounds` | `json_out_of_range_index_error_array_bounds` |
| Missing key versus null | `json_missing_key_versus_null` |
| Contains on an array fails typed | `json_contains_on_array_type_mismatch` |
| Wrong-container getter fails typed | `json_wrong_container_getter_type_mismatch`, `host_set_at_wrong_container_type_mismatch` |
| Push grows and remove returns | `json_push_grows_remove_returns` |
| Unit assignment stores null | `json_unit_assignment_stores_null` |
| Nested setter failure rolls back | `json_nested_setter_failure_rolls_back` |
| Mixed numeric comparison refuses | `json_all_six_comparisons_refuse_type_mismatch`, `host_compare_guard_smoke` |
| Null compares with unit | `json_null_compares_with_unit` |
| Native containers sorted + nested inline | `json_native_map_serializes_sorted_nested_inline` |
| Function and method forms | `json_function_and_method_forms_agree` |
| Raw UTF-8 and no slash escaping | `serialize_raw_utf8_no_slash_escape`, `json_raw_utf8_no_slash_escape` |
| Non-finite float refused | `json_nonfinite_float_refused`, `host_shadows_stock_to_json_map_nonfinite` |
| Unsupported values (FnPtr/timestamp) refused, no leak | `json_unsupported_value_refused_type_mismatch` |
| Depth boundary | `parse_depth_128_accepted_129_limit`, `json_depth_boundary` |
| Native container `to_json` depth invariant (Map + Array) | `json_native_container_depth_limit` |
| Mutation depth and cap rechecked | `parse_array_and_map_caps_limit`, `json_mutation_depth_and_cap_rechecked`, `json_mutation_size_limit`, `host_set_at_depth_limit_unchanged`, `host_set_at_size_limit_unchanged`, `host_push_cap_limit_unchanged` |
| Bound violation commits no change | `host_run_with_scope_push_rollback`, `host_set_at_depth_limit_unchanged`, `host_set_at_size_limit_unchanged`, `host_push_cap_limit_unchanged` |
| Self-assignment at cap refused | `json_self_assignment_at_cap_refused`, `host_set_at_depth_limit_unchanged` |
| Arithmetic/container helpers function-not-found | `json_arithmetic_and_helpers_function_not_found` |
| Iteration is a runtime error | `json_iteration_runtime_error` |
| Setter failure propagates | `json_setter_failure_propagates` |
| Parse error is redacted | `parse_malformed_redacted`, `json_parse_error_is_parse_class_with_position` |
| Exchange inbound unchanged | `json_inbound_u64_property_still_refused`, `u64_above_i64_max_refused_inbound` |
| Outbound refusal stable labels | `json_value_refused_outbound_stable_label`, `json_number_refused_outbound_stable_label` |
| Explicit serialization text body | `json_explicit_serialization_yields_text_body` |
| Equivalent, not byte-identical | `json_round_trip_equivalent_not_byte_identical` |
| Limits read from calling engine / zero = unlimited | `json_limits_read_from_calling_engine`, `json_zero_limit_is_unlimited`, `parse_zero_caps_are_unlimited` |
