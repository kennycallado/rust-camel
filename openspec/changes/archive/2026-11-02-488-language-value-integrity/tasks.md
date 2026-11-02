# Tasks: 488-language-value-integrity

## Phase 1: Read surfaces and key presence (rc-u8t5c)

### camel-language-simple

#### Task 1.1: Simple has_header/has_property

**Files:**
- `crates/languages/camel-language-simple/src/parser.rs` (modified)
- `crates/languages/camel-language-simple/src/evaluator.rs` (modified)
- `crates/languages/camel-language-simple/src/lib.rs` (modified: tests)

**Steps:**
1. Add `HasHeader` and `HasProperty` variants to `enum CallKind` in `parser.rs`.
2. In `parse_call` map `"has_header" => CallKind::HasHeader` and `"has_property" => CallKind::HasProperty`.
3. Extend the expected-arity rule so `HasHeader | HasProperty` require exactly 1 argument (they join `Int | String` in the 1-arg branch).
4. In `evaluator.rs`, add a helper `fn presence_arg(kind: &CallKind, args: &[Expr], exchange: &Exchange) -> Result<bool, LanguageError>`: evaluate `args[0]`; require `Value::String(key)` else `LanguageError::TypeMismatch { expected: "string".into(), actual: type_name.into(), position: None }`, where `type_name` is one of the static strings null/bool/number/string/array/object and never the value; return `exchange.input.headers.contains_key(&key)` for HasHeader and `exchange.properties.contains_key(&key)` for HasProperty.
5. Add the `CallKind::HasHeader | CallKind::HasProperty` branch to BOTH `Expr::Call` arms (`eval_sync_expr` and `eval_async_expr` in `evaluator.rs`), returning `Value::Bool(presence_arg(...)?)`.
6. Add tests to `lib.rs` `mod tests` using the existing `eval`/`SimpleLanguage` helpers.

**Tests:** command `cargo test -p camel-language-simple --lib`
- `has_header_absent_is_false`: exchange with no header `flag` → evaluate `${has_header("flag")}` → `Value::Bool(false)`. Expected before impl: FAIL (parse error "unknown function").
- `has_header_stored_null_is_true`: header `flag` set to `Value::Null` → `${has_header("flag")}` → `Value::Bool(true)`; `${header.flag}` → `Value::Null`. Expected before impl: FAIL.
- `has_property_absent_and_null`: property `q` = `Value::Null`, no property `p` → `${has_property("q")}` true, `${has_property("p")}` false, `${exchangeProperty.p}` and `${exchangeProperty.q}` both `Value::Null`. Expected before impl: FAIL.
- `has_header_non_string_arg_is_type_mismatch`: `${has_header(1)}` → `LanguageError::TypeMismatch { expected: "string", actual: "number" }`; the error Display contains no value text. Expected before impl: FAIL.
- `has_header_delegated_walk_is_consistent`: in the `evaluator.rs` inline test module, build `Expr::Call { kind: CallKind::Default, args: vec![Expr::Call { kind: CallKind::HasHeader, args: vec![Expr::StringLit("p".into())] }, Expr::LanguageDelegate { language: "mock".into(), expression: "true".into() }] }` (delegates parse only as a whole slot, so construct the AST directly; the mock `ResolverFn` from `evaluate_delegate_and_delegate_errors` forces `eval_async_expr`) → the same bool as the sync `${has_header('p')}` on the same fixture; repeat independently for `CallKind::HasProperty`. Expected before impl: FAIL.

**Acceptance:**
- `cargo test -p camel-language-simple --lib` passes.
- `cargo clippy -p camel-language-simple --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-language-simple` exits 0.

- [x] 1.1

### camel-language-rhai

#### Task 1.2: Rhai has_header/has_property on both engines

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)

**Steps:**
1. In `register_snapshot_readers` (around line 681), after `property`, register `has_header` and `has_property` closures over the same captured maps, returning `bool` from `headers.contains_key(&name)` / `properties.contains_key(&name)`.
2. Confirm `create_eval_engine` (read-only) and the mutating script-step engine (the `register_snapshot_readers` call around line 861) both obtain the functions; if the mutating path does not call `register_snapshot_readers`, add the call.
3. Do NOT add the functions to `READ_ONLY_MUTATION_FNS` — they are readers.
4. Update the module doc comment (lines 4-6) to list `has_header()`/`has_property()`.
5. Add tests in the crate `#[cfg(test)]` module using the existing exchange helpers.

**Tests:** command `cargo test -p camel-language-rhai --lib`
- `has_property_separates_absent_and_null_read_only`: exchange with property `q` = `Value::Null` and no `p`; read-only predicate `has_property("p") == false && has_property("q") == true` → `matches` returns `true`.
- `has_header_separates_absent_and_null_read_only`: header `flag` = `Value::Null`; expression `has_header("flag")` → `true`, and no-header `nope` → `false`.
- `has_header_and_property_work_in_script_step`: mutating script `headers["x"] = "1"; has_header("x")` (map assignment is the supported mutating surface; `set_header()` is NOT registered) → evaluates `false` because readers are snapshot-backed and `x` was absent before the step, while the header `x` is written back to the exchange. Separately, a header `k` and property `p` present BEFORE the step → `has_header("k")` and `has_property("p")` both true inside the script.
- `has_header_is_not_a_read_only_mutation`: `create_predicate("has_header('x')")` succeeds (not rejected as a mutator).
Expected before impl: FAIL (function not found).

**Acceptance:**
- `cargo test -p camel-language-rhai --lib` passes.
- `cargo clippy -p camel-language-rhai --all-targets -- -D warnings` exits 0.

- [x] 1.2

### camel-language-js

#### Task 1.3: JS camel.header/has_header/has_property readers

**Files:**
- `crates/languages/camel-language-js/src/bindings.rs` (modified)
- `crates/languages/camel-language-js/src/expression.rs` (modified: tests)

**Steps:**
1. In `build_camel_global`, add a `header` native reader beside `property`, reading key `name` from the `headers` map's `__data` object exactly as `property` reads from `properties`.
2. Add `has_header(name)` and `has_property(name)`: resolve `__data` of the respective map and return `JsValue::from(data_obj.has_own_property(js_string!(key), ctx)?)`. Install them in BOTH read-only and writable mode (readers are not mutators; do not use `throwing_mutator`).
3. In read-only mode the map object is a proxy; resolve `__data` through the proxy (`map_obj.get("__data", ctx)`), then call `has_own_property` on the returned object.
4. Add tests in `expression.rs` `#[cfg(test)]` using `make_exchange` and `engine()`.

**Tests:** command `cargo test -p camel-language-js --lib`
- `header_reader_reads_value_and_null_for_absent`: read-only expression `camel.header('k')` with header `k` = `"v"` → `Value::String("v")`; absent `nope` → `Value::Null`.
- `has_header_separates_absent_and_null`: header `flag` absent and `nil` = `Value::Null`; read-only expression `[camel.has_header('flag'), camel.has_header('nil')]` → `[false, true]`.
- `has_property_separates_absent_and_null_mutating`: mutating script `camel.has_property('p')` with property `p` = `null` → true; absent → false.
- `has_header_is_allowed_in_read_only`: `create_predicate("camel.has_header('x')")` succeeds and `matches` returns a bool (not the read-only-mutation refusal).
Expected before impl: FAIL (undefined function → runtime error).

**Acceptance:**
- `cargo test -p camel-language-js --lib` passes.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0.

- [x] 1.3

### camel-language-minijinja

#### Task 1.4: MiniJinja header/property/has_* callables

**Files:**
- `crates/languages/camel-language-minijinja/src/engine.rs` (modified)
- `crates/languages/camel-language-minijinja/src/lib.rs` (modified: tests)

**Steps:**
1. In `engine.rs`, add `#[derive(Debug)] struct ContextReader { kind: ReaderKind }` with `enum ReaderKind { Header, Property, HasHeader, HasProperty }`, both `Send + Sync`.
2. Implement `minijinja::value::Object for ContextReader`: `call(self: &Arc<Self>, _state, args)` requires exactly one string argument (else `minijinja::Error::new(ErrorKind::InvalidOperation, "reader expects one string argument")`); captures `Arc<HashMap<String, Value>>` headers and properties in the struct. For Header/Property return `minijinja::Value::from_serialize(v)` or `minijinja::Value::from(())` for absent (unit); for Has* return `Value::from(map.contains_key(k))`.
3. `ContextReader` must hold `headers: Arc<HashMap<String, Value>>` and `properties: Arc<HashMap<String, Value>>` (cloned once per `build_context_bounded`), since `call` cannot borrow the exchange.
4. In `build_context_bounded`, insert the four readers into the `BTreeMap<String, minijinja::Value>` used to build the context (`Value::from_object(ctx_reader(...))`), keeping `body`, `headers`, and `exchangeProperty` entries.
5. Add tests in `lib.rs` using `render`/`MinijinjaExpression::compile`.

**Tests:** command `cargo test -p camel-language-minijinja --lib`
- `has_header_absent_vs_null`: template `{% autoescape "none" %}{{ has_header("flag") }}|{{ has_header("nil") }}{% endautoescape %}`, header `nil` = `Value::Null` → rendered `"false|true"`.
- `has_property_absent_vs_null`: property `q` = `Value::Null`, no `p` → `{{ has_property("q") }}{{ has_property("p") }}` → `"truefalse"`.
- `header_and_property_readers`: template `{{ header("k") }}-{{ property("p") }}` with header `k = "v"` and property `p = "w"` → `"v-w"`; absent reads render as empty (unit).
- `reader_rejects_non_string_arg`: `{{ has_header(1) }}` → render error; the error Display contains no exchange data.
Expected before impl: FAIL (unknown function).

**Acceptance:**
- `cargo test -p camel-language-minijinja --lib` passes.
- `cargo clippy -p camel-language-minijinja --all-targets -- -D warnings` exits 0.

- [x] 1.4

### camel-language-jsonpath / camel-language-xpath

#### Task 1.5: JSONPath/XPath body-only decision tests

**Files:**
- `crates/languages/camel-language-jsonpath/src/lib.rs` (modified: tests only)
- `crates/languages/camel-language-xpath/src/lib.rs` (modified: tests only)

**Steps:**
1. No production code change. Add one test per crate pinning the body-only boundary.
2. JSONPath test: exchange with a `Json` body `{"header": "from-body"}` and a header named `header` with value `"from-header"`; evaluate `$.header` → `Value::String("from-body")`.
3. XPath test: exchange with `Xml` body `<r><header>from-body</header></r>` and header `header = "from-header"`; evaluate `/r/header/text()` → `Value::String("from-body")`.
4. XPath test: `create_expression("header('x')")` succeeds syntactically but evaluation against a body where no `header` function exists returns an evaluation error (unknown function), proving no key-reader surface exists.
5. JSONPath negative test: `create_expression("has_header('x')")` returns `Err(LanguageError::ParseError)` (the query parser rejects the call), proving no key-reader surface exists.

**Tests:** commands `cargo test -p camel-language-jsonpath --lib` and `cargo test -p camel-language-xpath --lib`
- `jsonpath_reads_body_not_headers`: setup as step 2 → result `"from-body"`.
- `xpath_reads_body_not_headers`: setup as step 3 → result `"from-body"`.
- `jsonpath_has_no_header_reader`: `create_expression("has_header('x')")` returns `Err(LanguageError::ParseError)`.
- `xpath_has_no_header_reader`: `header('x')` evaluation errors with `ExpressionErrorClass::FunctionNotFound` (or `Runtime` if the crate's mapper uses Runtime; assert it is an error and the detail is payload-free).
Expected before impl: PASS (pins existing body-only behavior).

**Acceptance:**
- Both test commands pass.
- `cargo clippy -p camel-language-jsonpath -p camel-language-xpath --all-targets -- -D warnings` exits 0.

- [x] 1.5

### docs

#### Task 1.6: Document read surfaces and body-only languages

**Files:**
- `docs/src/languages/simple.md` (modified)
- `docs/src/languages/rhai.md` (modified)
- `docs/src/languages/js.md` (modified)
- `docs/src/languages/minijinja.md` (modified)
- `docs/src/languages/jsonpath.md` (modified)
- `docs/src/languages/xpath.md` (modified)

**Steps:**
1. In each script-language page, add `has_header(name)`/`has_property(name)` to the reader table and state that presence is key containment (a stored null is present) and that the backing view is snapshot (Rhai) or live (JS).
2. Document JS `camel.header(name)` and MiniJinja `header()`/`property()` readers.
3. In `jsonpath.md` and `xpath.md`, add a one-line boundary note: body-only languages, no header/property readers (audit matrix section 4 decision).
4. Keep prose ASCII and in English; no non-ASCII identifiers.

**Tests:** command `cd docs && mdbook test`
- No code-block test is required for prose changes; `mdbook test` must exit 0 (if the pages contain no testable blocks it reports 0 tests and exits 0).
Expected before impl: PASS (docs-only change).

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- `cargo xtask lint-context-citations` exits 0.

- [x] 1.6

## Phase 2: Access-aware streaming refusal (rc-xnpg7)

### camel-language-simple

#### Task 2.1: Simple refuses a stream body only when read

**Files:**
- `crates/languages/camel-language-simple/src/evaluator.rs` (modified)
- `crates/languages/camel-language-simple/src/lib.rs` (modified: tests)

**Steps:**
1. Add `fn stream_body_error() -> LanguageError` returning `LanguageError::ConversionError { source_type: "Body::Stream".into(), target: "body".into() }`.
2. Add `fn body_value(exchange: &Exchange) -> Result<Value, LanguageError>`: `Body::Text`/`Xml` → `Value::String`; `Body::Json(v)` → `Value::String(v.to_string())` (unchanged; Phase 3 replaces this arm with `v.clone()`); `Body::Bytes(b)` → `Value::String(String::from_utf8_lossy(b)...)` (unchanged; Phase 3 replaces with an array); `Body::Empty` → `Value::Null`; `Body::Stream(_)` → `Err(stream_body_error())`.
3. Add `fn body_field_value(exchange: &Exchange, segments: &[PathSegment]) -> Result<Value, LanguageError>`: if `Body::Stream(_)` → `Err(stream_body_error())`; if `Body::Json(root)` walk the existing path logic and return the value or `Value::Null`; every other variant → `Value::Null`.
4. Remove `Expr::Body` and `Expr::BodyField` from `eval_leaf` so the sync fast path cannot produce a silent null for a stream.
5. Add explicit `Expr::Body => body_value(...)` and `Expr::BodyField(segments) => body_field_value(...)` arms to BOTH `eval_sync_expr` and `eval_async_expr` (remove any now-dead duplicate bodies).
6. Add tests in `lib.rs`.

**Tests:** command `cargo test -p camel-language-simple --lib`
- `stream_body_read_is_conversion_error`: exchange body `Body::Stream`; `${body}` → `Err(LanguageError::ConversionError { source_type: "Body::Stream", .. })`; the exchange body is still `Body::Stream`.
- `stream_body_field_read_is_conversion_error`: `${body.x}` on a stream → same conversion error.
- `header_only_filter_passes_stream_body`: body `Body::Stream`, header `type = "order"`; predicate `${header.type} == 'order'` → `matches` returns `true`; body is still `Body::Stream`.
- `header_only_interpolation_passes_stream_body`: expression `Hello ${header.type}` on a stream → `"Hello order"`.
- `stream_body_read_is_conversion_error_in_delegated_walk`: build `Expr::Call { kind: CallKind::Default, args: vec![Expr::Body, Expr::LanguageDelegate { language: "mock".into(), expression: "true".into() }] }` in the inline test module (forces `eval_async_expr`; `default` returns its first non-null arg) and evaluate on a stream → `ConversionError`; the body is still `Body::Stream`.
- `header_only_filter_passes_stream_body_delegated`: build `Expr::LogicalOp { left: Box::new(Expr::BinOp { left: Box::new(Expr::Header("type".into())), op: Op::Eq, right: Box::new(Expr::StringLit("order".into())) }), op: LogicalOp::And, right: Box::new(Expr::LanguageDelegate { language: "mock".into(), expression: "true".into() }) }` and evaluate on a stream → `true`.
Expected before impl: FAIL (tests 1-2 return `Value::Null`; 3 passes today).

**Acceptance:**
- `cargo test -p camel-language-simple --lib` passes.
- `cargo clippy -p camel-language-simple --all-targets -- -D warnings` exits 0.

- [x] 2.1

### camel-language-rhai

#### Task 2.2: Rhai header-only stream regression test

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified: tests only)

**Steps:**
1. No production change expected; the lazy `StreamBodyRef` marker already refuses only materializing reads. Verify by reading `stream_body.rs` and the existing stream tests.
2. Add the header-only predicate test; if it fails, fix only the scope binding so `register_snapshot_readers` is installed without cloning/probing the body marker.
3. Run the existing exemption tests explicitly and record them as the coverage for the spec's introspection/handled-read/discarded-statement scenarios: `stream_body_type_of_returns_marker_name_documented`, `stream_body_caught_method_read_is_suppressed_e7`, `stream_body_caught_index_read_is_suppressed_e7`, `stream_body_discarded_statement_is_noop`.

**Tests:** command `cargo test -p camel-language-rhai --lib`
- `header_only_predicate_passes_stream_body`: body `Body::Stream`, header `type = "order"`; predicate `header("type") == "order"` → `matches` true; the body is still `Body::Stream`.
- `header_only_script_passes_stream_body`: mutating script `headers["x"] = "1"` (map assignment; `set_header()` is not registered) on a stream → succeeds, header written, body still `Body::Stream`.
Expected before impl: PASS (regression pin).

**Acceptance:**
- `cargo test -p camel-language-rhai --lib` passes.

- [x] 2.2

### camel-language-minijinja

#### Task 2.3: MiniJinja lazy stream refusal and non-UTF-8 Bytes

**Files:**
- `crates/languages/camel-language-minijinja/src/engine.rs` (modified)
- `crates/languages/camel-language-minijinja/src/lib.rs` (modified: tests)

**Steps:**
1. Add `pub(crate) const STREAM_REFUSAL: &str = "minijinja cannot render a Body::Stream directly; add `stream_cache` upstream (crates/camel-processor/src/stream_cache.rs:37-54)";`.
2. Remove the up-front `if matches!(&exchange.input.body, Body::Stream(_)) { return Err(...) }` block in `MinijinjaExpression::evaluate`.
3. In `MinijinjaExpression::evaluate`, before building the context, add the non-UTF-8 Bytes refusal: `if let Body::Bytes(b) = &exchange.input.body && std::str::from_utf8(b).is_err() { return Err(LanguageError::ConversionError { source_type: "Body::Bytes".into(), target: "body".into() }); }`. Do this in the INLINE language, not in the shared `build_context_bounded`: the external `camel-template` component (ADR-0047) keeps its documented lossy projection and is covered by the existing `producer_renders_bytes_body_lossy_utf8` test.
4. In `build_context_bounded`, build the `body` value as: for `Body::Stream(_)` → `minijinja::Value::from(minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, STREAM_REFUSAL))`; otherwise `minijinja::Value::from_serialize(&BodyAsJson::from(...))` as today. Skip the body size check for streams.
5. In the measurement pass, make `BodyAsJson::serialize` handle `Body::Stream(_)` by `s.serialize_str("")` (remove the `unreachable!`).
6. Leave `BodyAsJson::serialize`'s `Body::Bytes(b)` arm unchanged (`s.serialize_str(&String::from_utf8_lossy(b))`) so the external component keeps its lossy contract; the inline refusal happens in step 3 before this path is reached.
7. In `MinijinjaExpression::compile`, after adding the template, call `env.set_formatter(|out, state, value| { if value.kind() == minijinja::value::ValueKind::Invalid && value.to_string().contains(STREAM_REFUSAL) { Err(minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, STREAM_REFUSAL)) } else { minijinja::escape_formatter(out, state, value) } })`.
8. In `map_render_error`, before the kind match: if `e.kind() == minijinja::ErrorKind::InvalidOperation && e.detail() == Some(STREAM_REFUSAL)` return `LanguageError::EvalError(STREAM_REFUSAL.to_string())` (preserves the runtime class and the stream_cache hint).
9. Update `build_context_bounded` doc comment (`Stream` is no longer unreachable).
10. Add/update tests in `lib.rs`.

**Tests:** command `cargo test -p camel-language-minijinja --lib`
- `stream_body_rejected_with_stream_cache_hint` (existing, `{{ body }}`): still `Err`; message contains `"stream_cache"`. Expected: PASS after change (FAIL between steps 2 and 7 if run early).
- `header_only_template_passes_stream_body`: `{% autoescape "none" %}{{ headers.type }}{% endautoescape %}` with header `type = "order"` on a stream → rendered `"order"`; body still `Body::Stream`.
- `unexecuted_body_branch_passes_stream_body`: `{% if false %}{{ body }}{% endif %}ok` → `"ok"`.
- `captured_stream_body_refuses_at_capture`: `{% set captured = body %}ok` → `Err` with the stream hint (MiniJinja validates the invalid value at variable lookup; capture is a read).
- `stream_body_truth_test_refuses`: `{% if body %}x{% endif %}` → `Err` with the stream hint.
- `stream_body_filter_refuses`: `{{ body|upper }}` on a stream → `Err`; message contains `"stream_cache"`.
- `stream_body_comparison_refuses`: `{% if body == 1 %}x{% endif %}` on a stream → `Err`; message contains `"stream_cache"`.
- `stream_body_iteration_refuses`: `{% for x in body %}{{ x }}{% endfor %}` on a stream → `Err`; message contains `"stream_cache"`.
- `stream_body_member_access_refuses`: `{{ body.x }}` on a stream → `Err`; message contains `"stream_cache"`.
- `mini_non_utf8_bytes_is_conversion_error`: `Body::Bytes(vec![0xff, 0xfe])` and `{{ body }}` → `Err`; no U+FFFD in the message; class `Conversion` (assert via `err.class()`).
Expected before impl: FAIL for header-only/unexecuted/capture/non-UTF8 (up-front refusal).

**Probe-resolution rule:** every consuming probe above must fail with the stream hint. If any probe silently succeeds or yields a non-stream error (marker erased by that engine path), do NOT weaken the test and do NOT add an ad-hoc exemption: STOP, amend the blessed spec/design/tasks to state the exact engine limitation and the required behavior, and obtain a fresh plan blessing before continuing the task. Declaring a new exemption during implementation is not permitted.

**Acceptance:**
- `cargo test -p camel-language-minijinja --lib` passes.
- `cargo clippy -p camel-language-minijinja --all-targets -- -D warnings` exits 0.

- [x] 2.3

### camel-language-js

#### Task 2.4: JS body-stream state and accessor binding

**Files:**
- `crates/languages/camel-language-js/src/engine.rs` (modified)
- `crates/languages/camel-language-js/src/error.rs` (modified)
- `crates/languages/camel-language-js/src/bindings.rs` (modified)
- `crates/languages/camel-language-js/src/expression.rs` (modified)
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)
- `crates/languages/camel-language-js/src/language.rs` (modified: test engine construction)

**Steps:**
1. `engine.rs`: add `pub body_is_stream: bool` to `JsExchange` (default false); `from_headers_body_properties` sets it false.
2. `engine.rs`: add `pub body_unread_stream: bool` and `pub body_stream_replaced: bool` to `JsEvalResult`; update every struct literal (`worker.rs` two sites, `language.rs` test engine, `expression.rs` test engine) with `false` defaults.
3. `error.rs`: add `StreamBodyRead` variant to `JsLanguageError`.
4. `expression.rs::snapshot_exchange`: for `Body::Stream(_)` return `JsExchange { body: Value::Null, body_is_stream: true, .. }` instead of an error; keep the stream_cache hint as a `pub(crate) const STREAM_HINT` used by the error mapping.
5. `expression.rs::js_err_to_lang_err`: map `StreamBodyRead` to `LanguageError::EvalError(STREAM_HINT.to_string())` (preserves the runtime class and hint).
6. `bindings.rs`: add `pub(crate) struct StreamSentinel(JsObject)` with `new()`, `matches(&JsError)`, and a `sentinel_function` twin of the read-only helper (identity comparison). Add a new return struct `pub(crate) struct CamelGlobal { pub camel: JsObject, pub stream_sentinel: Option<JsObject>, pub stream_getter: Option<JsValue> }` and change `build_camel_global` to return it.
7. In `build_camel_global`, when `exchange.body_is_stream`: create the sentinel, a getter native function that returns `Err(JsError::from_opaque(sentinel_obj))`, and a setter native function that calls `this.as_object().define_property_or_throw("body", PropertyDescriptor::builder().value(val).writable(true).enumerable(true).configurable(true).build(), ctx)`; define `body` as an accessor via `camel.define_property_or_throw(...)`. In read-only mode use `throwing_mutator(sentinel, ctx)` only for the SETTER (the read-only proxy's `set` trap still runs first). Do not add a data `body` property in the stream case.
8. `worker.rs`: extend `InstalledGlobals` with `stream_sentinel: Option<JsObject>` and `stream_getter: Option<JsValue>`; populate from `CamelGlobal`. Thread both into `eval_cached` and `classify_eval_error`.
9. `worker.rs::classify_eval_error`: add a `stream_sentinel: Option<&JsObject>` parameter; if the caught error's opaque value `strict_equals` the sentinel, return `JsLanguageError::StreamBodyRead`. Keep the existing read-only-sentinel check first.
10. `worker.rs::eval_cached`: when `exchange.body_is_stream` (added parameter), set `body_snapshot = Value::Null` and do not call `extract_body_snapshot`; read-only path passes `Value::Null`.
11. `expression.rs::apply_result_to_exchange`: skip the body write when `result.body_unread_stream`; write `value_to_body` when `result.body_stream_replaced`; otherwise keep the existing snapshot comparison.
12. Update `test_mutating_expression_rejects_stream_body` to `test_mutating_expression_header_only_on_stream_succeeds` (asserts `Ok`, header set, body still `Body::Stream`).

**Tests:** command `cargo test -p camel-language-js --lib`
- `js_header_only_predicate_passes_stream_body`: read-only predicate `camel.header('type') === 'order'` on a stream with header `type = "order"` → `Ok(true)`.
- `js_header_only_script_passes_stream_body`: mutating script `camel.headers.set('x','1')` on a stream → `Ok`; header `x` set; body still `Body::Stream`.
- `js_reading_stream_body_refuses`: read-only expression `camel.body` on a stream → `Err(LanguageError::EvalError(msg))` with `msg` containing `"stream_cache"`.
- `js_reading_stream_body_in_script_refuses`: mutating script `camel.body` on a stream → `Err`; exchange unchanged.
- `js_null_on_empty_is_noop` (existing) still passes.
Expected before impl: FAIL (header-only currently rejects).

**Acceptance:**
- `cargo test -p camel-language-js --lib` passes.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0.

- [x] 2.4

#### Task 2.5: JS descriptor extraction and stream replacement

**Files:**
- `crates/languages/camel-language-js/src/bindings.rs` (modified)
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)
- `crates/languages/camel-language-js/src/expression.rs` (modified: tests)

**Steps:**
1. `bindings.rs`: add `pub(crate) struct ExtractedState { pub headers: HashMap<String, Value>, pub body: Value, pub properties: HashMap<String, Value>, pub body_unread_stream: bool, pub body_stream_replaced: bool }`.
2. Change `extract_camel_state` to `extract_camel_state(ctx, detector, body_is_stream: bool, read_only: bool, stream_getter: Option<&JsValue>) -> Result<ExtractedState, JsLanguageError>`.
3. When `body_is_stream && read_only`: set `body_unread_stream = true`, `body = Value::Null`; extract headers/properties normally.
4. When `body_is_stream && !read_only`: inspect `camel.borrow().properties().get(&PropertyKey::from(js_string!("body")))`:
   - accessor whose `get()` `strict_equals` `stream_getter` → `body_unread_stream = true`, `body = Value::Null`.
   - accessor with a different getter → `Err(JsLanguageError::Execution { message: "camel.body was replaced with an accessor; assign a plain value instead".into() })`.
   - data descriptor with a value → `js_to_value_with_detector(value, ...)`, `body_stream_replaced = true`.
   - no descriptor (deleted) → `body = Value::Null`, `body_stream_replaced = true` (maps to `Body::Empty`).
5. `worker.rs::eval_cached`: pass `exchange.body_is_stream`, `read_only`, and `installed.stream_getter` to `extract_camel_state`; build `JsEvalResult` from `ExtractedState` (headers/body/properties/unread/replaced).
6. `expression.rs::apply_result_to_exchange`: (already handles the flags from Task 2.4) confirm a deleted body (`body_stream_replaced` with `Value::Null`) writes `Body::Empty`.
7. Add tests in `expression.rs`.

**Tests:** command `cargo test -p camel-language-js --lib`
- `js_stream_body_assignment_writes_text`: mutating script `camel.body = 'done'` on a stream → body `Body::Text("done")`.
- `js_stream_body_define_property_value_writes_text`: `Object.defineProperty(camel, 'body', {value: 'x', writable: true, configurable: true, enumerable: true})` on a stream → body `Body::Text("x")`.
- `js_stream_body_custom_accessor_is_refused`: `Object.defineProperty(camel, 'body', {get(){return 'x'}})` on a stream → `Err` (runtime class, payload-free); exchange still `Body::Stream`.
- `js_stream_body_delete_yields_empty`: `delete camel.body` on a stream → body `Body::Empty`.
- `js_caught_stream_body_read_is_suppressed`: mutating script `try { camel.body } catch (e) {} 'ok'` on a stream → `Ok("ok")`; body still `Body::Stream` (caught getter exception is a handled read).
- `js_stream_body_unread_is_preserved`: mutating script `camel.headers.set('x','1')` → body `Body::Stream` and header `x` set.
Expected before impl: FAIL.

**Acceptance:**
- `cargo test -p camel-language-js --lib` passes.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0.

- [x] 2.5

## Phase 3: Value integrity (rc-zwy7e)

### camel-language-simple

#### Task 3.1: Simple native JSON and Bytes body exposure (R3 flip)

**Files:**
- `crates/languages/camel-language-simple/src/evaluator.rs` (modified)
- `crates/languages/camel-language-simple/src/lib.rs` (modified: tests)
- `crates/languages/camel-language-simple/src/evaluator.rs` (modified: inline tests)

**Steps:**
1. In `body_value`, change `Body::Json(v)` to `Ok(v.clone())` and `Body::Bytes(b)` to `Ok(Value::Array(b.iter().map(|byte| Value::Number((*byte).into())).collect()))`.
2. Keep `Body::Text`/`Xml` as `Value::String`, `Body::Empty` as `Value::Null`, `Body::Stream` as `Err(stream_body_error())`.
3. In `body_field_value`, leave `Json` path/non-Json-absent logic unchanged; only the stream arm errors.
4. Update the pinned tests: `evaluate_body_variants_and_header_property` (Json arm now `json!({"a": 1})`, Bytes arm now `[97, 98, 99]`); rename `body_json_returns_serialized_string` → `body_json_returns_native_value` asserting `Value::Object` equality; rename `body_bytes_utf8_returns_string` → `body_bytes_returns_byte_array` asserting `Value::Array([104,101,...])`; update `body_bytes_in_interpolation` to assert the interpolation of the array renders the JSON array text (`Value::String("[112,105,110,103]".into())`).
5. Add the R3 flip contract test in `lib.rs`.

**Tests:** command `cargo test -p camel-language-simple --lib`
- `r3_body_preserves_json_object` (R3 flip): exchange `Body::Json(json!({"a": 1}))`; `${body}` → `Value::Object({"a": 1})`; then feed the result into `ValueSource::Body`-equivalent (`Body::from(value)`) and assert the body is `Body::Json({"a": 1})`, not text.
- `body_json_returns_native_value`: `${body}` on `{"msg": "world"}` → `Value::Object`, `value["msg"] == "world"`.
- `body_bytes_returns_byte_array`: `Body::Bytes([0, 128, 255])`; `${body}` → `Value::Array([0, 128, 255])`.
- `body_bytes_in_interpolation` (updated): `Body::Bytes(b"ping")`; `Received: ${body}` → `Value::String("Received: [112,105,110,103]")`.
- `body_json_returns_native_value_in_delegated_walk`: build `Expr::Call { kind: CallKind::Default, args: vec![Expr::Body, Expr::LanguageDelegate { language: "mock".into(), expression: "true".into() }] }` in the inline test module (forced async; `default` returns the body) on `Body::Json({"a":1})` → `Value::Object({"a": 1})`.
- `body_bytes_returns_byte_array_in_delegated_walk`: same constructed AST on `Body::Bytes([0,128,255])` → `Value::Array([0,128,255])`.
Expected before impl: FAIL (native/Bytes tests; interpolation expectation changes).

**Acceptance:**
- `cargo test -p camel-language-simple --lib` passes.
- `cargo clippy -p camel-language-simple --all-targets -- -D warnings` exits 0.

- [x] 3.1

### camel-language-js

#### Task 3.2: JS Bytes exposed as an integer array

**Files:**
- `crates/languages/camel-language-js/src/expression.rs` (modified)
- `crates/languages/camel-language-js/src/expression.rs` (modified: tests)

**Steps:**
1. In `snapshot_exchange`, replace the `Body::Bytes` fallthrough-to-null with `Value::Array(b.iter().map(|byte| Value::Number((*byte).into())).collect())`; keep `Body::Empty` as `Value::Null`.
2. Update the doc comment on `value_to_body` / the null-on-Bytes note.
3. Update `js_null_on_bytes_is_noop` → `js_null_on_bytes_becomes_empty` (null on an exposed integer array is a real change → `Body::Empty`).
4. Add native-array tests.

**Tests:** command `cargo test -p camel-language-js --lib`
- `js_json_body_reads_as_object`: read-only expression `camel.body.n` on `Body::Json({"n": 1})` → `Value::Number(1)` (pins the spec scenario).
- `js_bytes_body_reads_as_integer_array`: read-only expression `camel.body[1]` on `Body::Bytes([0, 128, 255])` → `Value::Number(128)`.
- `js_header_only_preserves_bytes_body`: mutating script `camel.headers.set('x','1')` on `Body::Bytes([1,2])` → body still `Body::Bytes([1,2])` (unchanged snapshot).
- `js_null_on_bytes_becomes_empty`: `camel.body = null` on `Body::Bytes` → `Body::Empty`.
Expected before impl: FAIL (`camel.body[1]` on null → error; null-on-Bytes currently a no-op).

**Acceptance:**
- `cargo test -p camel-language-js --lib` passes.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0.

- [x] 3.2

#### Task 3.3: JS changed-only transactional map write-back

**Files:**
- `crates/languages/camel-language-js/src/engine.rs` (modified)
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)
- `crates/languages/camel-language-js/src/language.rs` (modified: test engine)
- `crates/languages/camel-language-js/src/expression.rs` (modified + tests)

**Steps:**
1. `engine.rs`: add `pub headers_snapshot: HashMap<String, Value>` and `pub properties_snapshot: HashMap<String, Value>` to `JsEvalResult`.
2. `worker.rs`: add `fn extract_maps_snapshot(ctx, detector) -> Result<(HashMap<String, Value>, HashMap<String, Value>), JsLanguageError>` that reads the installed `camel.headers.__data` and `camel.properties.__data` with the same `extract_map` conversion used for post-state. Call it in `eval_cached` immediately before the script runs for mutating evaluation only (`sentinel.is_none()`); for read-only set both snapshots to the inbound maps (or empty).
3. Update all `JsEvalResult` construction sites with the new fields.
4. `expression.rs::apply_result_to_exchange`: implement `fn apply_map_changes(snapshot: &HashMap<String, Value>, post: &HashMap<String, Value>, target: &mut HashMap<String, Value>)`: insert `(k, v)` when `snapshot.get(k) != Some(v)`; remove `k` when `snapshot` has `k` and `post` does not; leave untouched entries exactly as they are.
5. Apply it for headers and properties; body handling stays as in Tasks 2.4/2.5. Compute both maps fully, then assign (no partial mutation on error is already guaranteed because all conversions happened inside the worker before this function).
6. Update tests in `expression.rs`.

**Tests:** command `cargo test -p camel-language-js --lib`
- `js_untouched_float_header_keeps_representation`: header `n = json!(10.0)`, script `camel.headers.set('other','1')` → `n` still `json!(10.0)` (not `10`).
- `js_changed_header_is_written`: header `n = 1`, script `camel.headers.set('n', 2)` → `2`.
- `js_removed_header_is_removed`: header `drop = "x"`, script `camel.headers.remove('drop')` → key absent.
- `js_untouched_nested_property_preserved`: property `p = {"a":[1,2]}`, script modifies a different property → `p` still the same object.
- `js_conversion_failure_commits_nothing`: script `camel.headers.set('ok','1'); camel.headers.set('bad', NaN)` → `Err`; header `ok` absent and original maps unchanged.
- Existing `js_header_only_preserves_xml_body` / `js_header_only_preserves_integral_float_json_body` still pass.
Expected before impl: FAIL (float header rewritten to `10`; wholesale rewrite).

**Acceptance:**
- `cargo test -p camel-language-js --lib` passes.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0.

- [x] 3.3

### camel-language-rhai / camel-language-js

#### Task 3.4: Mutating write-back pin tests (tests only)

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified: tests only)
- `crates/languages/camel-language-js/src/expression.rs` (modified: tests only)

**Steps:**
1. Re-run the existing Rhai pins: `header_only_script_preserves_json_body_bit_identical` (spec: header-only preserves a JSON body) and `same_value_body_assignment_writes_nothing` (spec: same-value assignment writes nothing). If either name does not exist verbatim in the crate, add the pin with that name.
2. Add the two missing Rhai pins: `rhai_unit_assignment_on_bytes_becomes_empty` (`script: body = ()` on `Body::Bytes([1,2])` → `Body::Empty`) and `rhai_unit_assignment_on_empty_is_unchanged` (`script: body = ()` on `Body::Empty` → `Body::Empty`).
3. Re-run the existing JS pin `js_null_body_becomes_empty` (spec: JS null body assignment yields Empty); if the name differs, add the pin with that name.

**Tests:** commands `cargo test -p camel-language-rhai --lib` and `cargo test -p camel-language-js --lib`
- `rhai_unit_assignment_on_bytes_becomes_empty`: exchange `Body::Bytes([1,2])`, mutating script `body = ()` → `matches!(body, Body::Empty)`.
- `rhai_unit_assignment_on_empty_is_unchanged`: exchange `Body::Empty`, mutating script `body = ()` → `matches!(body, Body::Empty)`.
- `header_only_script_preserves_json_body_bit_identical` and `same_value_body_assignment_writes_nothing` still pass.
- `js_null_body_becomes_empty` still passes.
Expected before impl: the two new Rhai pins may PASS already (Rhai Bytes is a Blob); if either FAILs, fix the Rhai body conversion rather than the test.

**Acceptance:**
- Both test commands pass.

- [x] 3.4

### docs

#### Task 3.5: Document value integrity and streaming behavior

**Files:**
- `docs/src/languages/simple.md` (modified)
- `docs/src/languages/js.md` (modified)
- `docs/src/languages/minijinja.md` (modified)
- `docs/src/languages/rhai.md` (modified)

**Steps:**
1. Document Simple `${body}` JSON-native and Bytes-as-integer-array behavior; document stream read refusal replacing the old silent null.
2. Document JS `Bytes` as an integer array, changed-only header/property write-back with JS normalization, and the lazy `camel.body` stream behavior (read refuses; assignment replaces; a replaced accessor is refused).
3. Document MiniJinja's empty-string body projection, non-UTF-8 Bytes refusal, and lazy stream behavior (header-only templates work; any lookup of `body` refuses immediately, including `{% set %}` capture and truth tests; an unexecuted branch does not refuse).
4. Document Rhai's header-only stream support beside the existing marker semantics.

**Tests:** command `cd docs && mdbook test`
- No testable code blocks added; `mdbook test` exits 0.
Expected before impl: PASS (docs-only change).

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- `cargo xtask lint-context-citations` exits 0.

- [x] 3.5
