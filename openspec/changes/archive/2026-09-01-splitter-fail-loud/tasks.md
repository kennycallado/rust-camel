# Tasks: splitter-fail-loud

## camel-api

### Task 1.1: Fallible `SplitExpression`, typed-error arms, public type-name helpers

**Files:**
- `crates/camel-api/src/splitter.rs` (modified)
- `crates/camel-api/src/body.rs` (modified)
- `crates/camel-api/src/value.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified)

**Steps:**
1. In `crates/camel-api/src/body.rs`, add `pub fn body_type_name(body: &Body) -> &'static str` with exact outputs: `Body::Empty => "empty"`, `Body::Bytes(_) => "bytes"`, `Body::Text(_) => "text"`, `Body::Json(_) => "json"`, `Body::Xml(_) => "xml"`, `Body::Stream(_) => "stream"`, catch-all `_ => "unknown"` (copy verbatim from `crates/camel-core/src/shared/observability/adapters/tracer.rs:29-38`; do not delete the tracer copy in this task).
2. In `crates/camel-api/src/value.rs`, add `pub fn value_type_name(v: &Value) -> &'static str` with exact outputs: `Value::String(_) => "string"`, `Value::Array(_) => "array"`, `Value::Number(_) => "number"`, `Value::Bool(_) => "boolean"`, `Value::Object(_) => "object"`, `Value::Null => "null"`, catch-all `_ => "other"`.
3. In `crates/camel-api/src/lib.rs`, make both helpers reachable as `camel_api::body_type_name` and `camel_api::value_type_name` (re-export next to the existing `Body`/`Value` re-exports).
4. In `crates/camel-api/src/splitter.rs`, change the alias to `pub type SplitExpression = Arc<dyn Fn(&Exchange) -> Result<Vec<Exchange>, CamelError> + Send + Sync>;`.
5. Rewrite `split_body_lines` match arms: `Body::Text(s)` fragments per line (unchanged); `Body::Empty => Ok(Vec::new())`; catch-all `_ => Err(CamelError::TypeConversionFailed(format!("split expression 'body_lines' requires body type text, got {received}; add an unmarshal step before split", received = body_type_name(&exchange.input.body))))`.
6. Rewrite `split_body_json_array` match arms: `Body::Json(Value::Array(arr))` fragments per element (unchanged); `Body::Empty => Ok(Vec::new())`; `Body::Json(_) => Err(CamelError::TypeConversionFailed("split expression 'body_json_array' requires body type json (array), got json (non-array); add an unmarshal step before split".to_string()))`; catch-all `_ => Err(CamelError::TypeConversionFailed(format!("split expression 'body_json_array' requires body type json (array), got {received}; add an unmarshal step before split", received = body_type_name(&exchange.input.body))))`.
7. Keep `split_body` closure bound `Fn(&Body) -> Vec<Body>`; wrap its collected fragments in `Ok(...)` inside the returned closure. Custom closures stay infallible.
8. Update the module doc comments on both built-ins: wrong-type bodies error, empty-content bodies pass through.
9. Wrap in `Ok(...)` the output of the two direct test closures at `crates/camel-api/src/splitter.rs:462` and `:468` (`Arc::new(|_: &Exchange| Vec::new()) as SplitExpression`).
10. Update the unit tests in the same file per the Tests block. Invert `test_split_body_json_array_not_array` to assert `Err` (do not delete it). Adapt the sibling direct-call tests to the new `Result` return: `test_split_body_custom`, `test_split_body_lines`, `test_split_body_json_array`, and any other direct caller surfaced by compilation — assertions stay identical, add the `Ok` unwrap at the call site.

**Tests:**
- `test_body_type_name_variants` (body.rs tests): construct one `Body` per variant → call `body_type_name` → assert each exact string (`"empty"`, `"bytes"`, `"text"`, `"json"`, `"xml"`, `"stream"`).
  - command: `cargo test -p camel-api --lib test_body_type_name_variants`
  - expected: red before implementation (helper absent, compile error) → green after step 1.
- `test_value_type_name_variants` (value.rs tests): construct one `Value` per listed variant → call `value_type_name` → assert each exact string (`"string"`, `"array"`, `"number"`, `"boolean"`, `"object"`, `"null"`).
  - command: `cargo test -p camel-api --lib test_value_type_name_variants`
  - expected: red before (compile error) → green after step 2.
- `test_split_body_lines_wrong_type_json_errors`: exchange with `Body::Json(json!({"a":1}))`, `split_body_lines()` applied → `Err` whose `to_string()` contains `"body_lines"`, `"json"`, `"text"`, and `"add an unmarshal step before split"`.
  - command: `cargo test -p camel-api --lib test_split_body_lines_wrong_type_json_errors`
  - expected: red before implementation (current code returns empty `Vec`, assertion on `Err` fails) → green after step 5.
- `test_split_body_json_array_wrong_type_text_errors`: exchange with `Body::Text("x")`, `split_body_json_array()` applied → `Err` containing `"body_json_array"`, `"text"`, `"json (array)"`, and the unmarshal phrase.
  - command: `cargo test -p camel-api --lib test_split_body_json_array_wrong_type_text_errors`
  - expected: red before (empty `Vec` returned) → green after step 6.
- `test_split_body_json_array_non_array_json_errors`: exchange with `Body::Json(json!({"o":1}))` → `Err` containing `"json (non-array)"`.
  - command: `cargo test -p camel-api --lib test_split_body_json_array_non_array_json_errors`
  - expected: red before → green after step 6.
- `test_split_body_lines_empty_body_ok`: exchange with `Body::Empty` → `Ok` with zero fragments.
  - command: `cargo test -p camel-api --lib test_split_body_lines_empty_body_ok`
  - expected: red before (non-`Text` arm currently indistinguishable; assertion compiles only after alias change) → green after steps 4-5.
- `test_split_body_json_array_empty_body_ok`: exchange with `Body::Empty` → `Ok` with zero fragments.
  - command: `cargo test -p camel-api --lib test_split_body_json_array_empty_body_ok`
  - expected: red before (compile error against old signature) → green after steps 4-6.
- `test_split_body_json_array_empty_array_ok`: exchange with `Body::Json(json!([]))` → `Ok` with zero fragments.
  - command: `cargo test -p camel-api --lib test_split_body_json_array_empty_array_ok`
  - expected: red before (compile error) → green after steps 4-6.
- `test_split_body_lines_empty_text_ok`: exchange with `Body::Text("")` → `Ok` with zero fragments.
  - command: `cargo test -p camel-api --lib test_split_body_lines_empty_text_ok`
  - expected: red before (compile error) → green after steps 4-5.
- `test_split_error_omits_payload`: exchange with `Body::Json(json!({"secret":"SECRET-8f31a"}))`, `split_body_lines()` applied → the error matches `CamelError::TypeConversionFailed`, its message contains `"body_lines"`, `"json"`, `"text"`, and `"add an unmarshal step before split"`, and does NOT contain `SECRET-8f31a`.
  - command: `cargo test -p camel-api --lib test_split_error_omits_payload`
  - expected: red before → green after step 5.
- `test_split_body_lines_empty` (existing, preserved; exercises `Body::Empty` via `Message::default()`, not empty text): assertions unchanged, add the `Ok` unwrap at the call site.
  - command: `cargo test -p camel-api --lib test_split_body_lines_empty`
  - expected: green before and after (mechanical unwrap only).
- `test_split_body_json_array_not_array` (existing, inverted): asserts `Err(TypeConversionFailed)` containing `"json (non-array)"`.
  - command: `cargo test -p camel-api --lib test_split_body_json_array_not_array`
  - expected: red after the alias change under the OLD assertions (they expect an empty `Vec`) → green once inverted per step 10.

**Acceptance:**
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `rg "TODO|TBD|placeholder" crates/camel-api/src/splitter.rs` returns no new hits.

- [x] 1.1

## camel-processor

### Task 2.1: Eager splitter and split segment propagate the typed error

**Files:**
- `crates/camel-processor/src/splitter.rs` (modified)
- `crates/camel-processor/src/split_segment.rs` (modified)

**Steps:**
1. In `SplitterService::call` (`splitter.rs:100`), replace `let mut fragments = expression(&exchange);` with `let mut fragments = expression(&exchange)?;`. The `fragments.is_empty()` branch keeps returning `Ok(original)`.
2. Wrap in `Ok(...)` the output of every direct `Arc::new` test closure that returns a `Vec<Exchange>` in `splitter.rs` tests (line 839 `test_splitter_rejects_fragment_flood` and any sibling raw closures; `split_body`-based call sites need no change).
3. In `SplitSegment::run` (`split_segment.rs`), propagate an `Err` from the split expression as `PipelineOutcome::Failed` carrying the original error. Do not touch `Stopped` handling (ADR-0025).
4. Wrap in `Ok(...)` the output of the five direct `Arc::new` test closures in `split_segment.rs` tests (lines 524, 630, 716, 921, 988).
5. Add the new tests per the Tests block.

**Tests:**
- `test_splitter_wrong_type_body_fails_loud` (splitter.rs): `SplitterService::new(SplitterConfig::new(split_body_json_array()), passthrough)`; exchange with `Body::Text("a,b")`; `svc.call(ex).await` → `Err` whose message contains `"body_json_array"`, `"text"`, `"json (array)"`, and `"add an unmarshal step before split"`.
  - command: `cargo test -p camel-processor --lib test_splitter_wrong_type_body_fails_loud`
  - expected: red before implementation (task 1.1 alone leaves `call` returning `Ok(original)` for empty fragments — assertion on `Err` fails) → green after step 1.
- `test_split_empty_fragments` (existing, preserved): `Body::Empty` → `Ok(original)`.
  - command: `cargo test -p camel-processor --lib test_split_empty_fragments`
  - expected: green before and after (no assertion change).
- `test_split_segment_expression_error_is_failed` (split_segment.rs): `SplitSegment` whose `splitter` closure returns `Err(CamelError::TypeConversionFailed("declarative split requires a text or array value, got number; add an unmarshal step before split".to_string()))` and whose body is a recording segment; run the segment on any exchange → outcome is `PipelineOutcome::Failed`, the carried error message contains `"declarative split"`, and the body segment recorded zero invocations.
  - command: `cargo test -p camel-processor --lib test_split_segment_expression_error_is_failed`
  - expected: red before implementation (closure cannot return `Err` under the old alias — compile-driven) → green after steps 3-4.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0 (covers `benches/splitter.rs` compiling unchanged).

- [x] 2.1

### Task 2.2: Streaming service propagates the typed error; test mirror synced

NOTE: the PRODUCTION streaming non-Stream error is constructed in `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs:265-272` (DeclarativeStreamSplit compile arm). That edit belongs to task 3.1. This task syncs the test mirror in `streaming_splitter.rs` and proves the service propagates the typed error without swallowing it.

**Files:**
- `crates/camel-processor/src/streaming_splitter.rs` (modified)

**Steps:**
1. In the `mod tests` helper `ndjson_stream_expression` (`streaming_splitter.rs:233-246`), replace the `CamelError::ProcessorError("streaming split requires Body::Stream")` construction with `CamelError::TypeConversionFailed(format!("streaming split requires body type stream, got {received}; add an unmarshal step before split", received = camel_api::body_type_name(&exchange.input.body)))`. Add a comment on the helper: it mirrors the production arm at `splitting.rs:265` and must stay in sync with it.
2. Add the propagation test per the Tests block.

**Tests:**
- `test_streaming_splitter_non_stream_body_typed_error`: `StreamingSplitterService` fed `ndjson_stream_expression` (synced mirror); exchange with `Body::Text("x")`; pulling the first item → `Err(TypeConversionFailed)` whose message contains `"streaming split"`, `"text"`, `"stream"`, and `"add an unmarshal step before split"`.
  - command: `cargo test -p camel-processor --lib test_streaming_splitter_non_stream_body_typed_error`
  - expected: red before step 1 (mirror returns `ProcessorError` with the old text) → green after step 1.

**Acceptance:**
- `cargo test -p camel-processor --lib streaming_splitter` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 2.2

## camel-core

### Task 3.1: Declarative eager + streaming compile arms error on non-splittable input; tracer reuses public helper

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified)
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified)

**Steps:**
1. In `splitting.rs`, change the eager `split_fn` closure (lines 126-149) to return `Result<Vec<Exchange>, CamelError>`: `Value::String(s)` and `Value::Array(arr)` arms wrap their fragment vectors in `Ok(...)`; the `_ => vec![exchange.clone()]` arm becomes `Err(CamelError::TypeConversionFailed(format!("declarative split requires a text or array value, got {received}; add an unmarshal step before split", received = camel_api::value_type_name(&value))))`.
2. In `splitting.rs`, change the DeclarativeStreamSplit compile arm's non-`Body::Stream` error (lines 265-272): replace `CamelError::ProcessorError("streaming split requires Body::Stream")` with `CamelError::TypeConversionFailed(format!("streaming split requires body type stream, got {received}; add an unmarshal step before split", received = camel_api::body_type_name(&exchange.input.body)))`.
3. Delete the private `body_type_name` in `tracer.rs:29-38` and replace its call sites with `camel_api::body_type_name`. Tracing field values do not change.
4. Add the new tests per the Tests block in the `splitting.rs` test module, following its existing test style. Fixture for the eager test: build the language registry with `languages_with_simple()` (pattern at `step_resolution.rs:313-321`: `HashMap<String, Arc<dyn Language>>` with `SimpleLanguage` under key `"simple"`), set the step's `LanguageExpressionDef` to `language: "simple"`, `source: "${header.num}"`, and give the input exchange a `num` header holding `Value::Number(1)` so the expression evaluates to a number. If the simple language cannot yield a number this way, register a test-local `Language` (trait at `crates/languages/camel-language-api/src/lib.rs:23`) under key `"stub-number"` whose `create_expression` returns an `Expression` evaluating to `Value::Number(1)`.

**Tests:**
- `test_declarative_split_non_text_non_array_fails`: compile a declarative split step whose language expression evaluates to `Value::Number(1)` (fixture per step 4); run the compiled segment on an exchange → outcome is `PipelineOutcome::Failed`, the error message contains `"declarative split"`, `"number"`, `"text or array"`, and `"add an unmarshal step before split"`, and the body segment recorded zero invocations (no cloned fragment).
  - command: `cargo test -p camel-core --lib test_declarative_split_non_text_non_array_fails`
  - expected: red before step 1 (the `_` arm clones the exchange, outcome is success) → green after step 1.
- `test_declarative_stream_split_non_stream_body_typed_error`: compile a `DeclarativeStreamSplit` step with `StreamSplitConfig` in a non-Zip format (e.g. JsonLines) and any child steps; run the compiled expression on an exchange with `Body::Text("x")` → the first pull returns `Err(TypeConversionFailed)` whose message contains `"streaming split"`, `"text"`, `"stream"`, and `"add an unmarshal step before split"`.
  - command: `cargo test -p camel-core --lib test_declarative_stream_split_non_stream_body_typed_error`
  - expected: red before step 2 (old `ProcessorError` text) → green after step 2.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0 (includes existing tracer tests after the helper swap).
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 3.1

## camel-dsl / camel-builder / camel-bench

### Task 4.1: Verify downstream closures compile; Ok-wrap only where demanded

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified only if compilation demands it)
- `crates/camel-builder/src/lib.rs` (modified only if compilation demands it)
- `examples/streaming-split/src/main.rs` (modified)

**Steps:**
1. Run `cargo test -p camel-dsl --lib`. If any test closure constructs a `SplitExpression` via `Arc::new` with a `Vec` return, wrap its output in `Ok(...)`. `SplitterConfig::new(split_body_lines())`-style constructor call sites must NOT change.
2. Run `cargo test -p camel-builder --lib`. Same rule for builder test closures.
3. Run `cargo check -p camel-bench --benches` and confirm `crates/camel-bench/benches/pipeline.rs` compiles unchanged (it uses the infallible `split_body`).
4. Update `examples/streaming-split/src/main.rs:51`: its custom closure still builds `CamelError::ProcessorError("streaming split requires Body::Stream")`. Custom closures own their policy (it compiles unchanged), but replace that construction with `CamelError::TypeConversionFailed(format!("streaming split requires body type stream, got {received}; add an unmarshal step before split", received = camel_api::body_type_name(&exchange.input.body)))` for consistency with the production arm.

**Tests:**
- `compile_step_split_body_lines` and `compile_step_split_body_json_array` (existing, compile.rs:3582/3595): stay green with zero edits to their bodies unless the compiler demands the `Ok` wrap; if edited, the assertions on compiled step shape must stay identical.
  - command: `cargo test -p camel-dsl --lib compile_step_split`
  - expected: green before and after (constructor-only tests).

**Acceptance:**
- `cargo test -p camel-dsl --lib` exits 0.
- `cargo test -p camel-builder --lib` exits 0.
- `cargo check -p camel-bench --benches` exits 0.
- `cargo check -p streaming-split` exits 0 (the example is its own workspace member).

- [x] 4.1

## Documentation

### Task 5.1: CONTEXT.md updates for both contracts

**Files:**
- `crates/camel-processor/CONTEXT.md` (modified)
- `crates/camel-api/CONTEXT.md` (modified)

**Steps:**
1. In `crates/camel-processor/CONTEXT.md`, extend the "Aggregation contract (divergence from Apache Camel)" section (~line 200): a wrong-type split body fails with `TypeConversionFailed` before fragmentation; genuinely empty content (`Body::Empty`, empty text, empty array) still returns the original exchange and skips aggregation.
2. In `crates/camel-api/CONTEXT.md`, add a new `## Splitter` section directly after the existing `## Language` section (line 29): the fallible `SplitExpression` alias (`Result<Vec<Exchange>, CamelError>`), built-in wrong-type arms error with the unmarshal hint, `Body::Empty` arms pass through, custom `split_body` closures stay infallible and own their type policy.

**Tests:**
- No unit tests. Gate check only.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- Both files mention `TypeConversionFailed` and the unmarshal phrase.

- [x] 5.1

## Workspace closure

### Task 6.1: Compile-atomic closure over the whole workspace

**Steps:**
1. Run `cargo build --workspace` and confirm exit 0.
2. Run `cargo check --workspace --tests` and confirm exit 0 (compiles the autodiscovered `tests/*.rs` integration targets in camel-test and examples that use the changed API).
3. Run `cargo test --workspace --lib` and confirm exit 0.
4. Run `cargo fmt --check --all` and fix any formatting drift introduced by earlier tasks.

**Files:**
- Only files touched by tasks 1.1-5.1 if fmt or compilation demands a fix.

**Tests:**
- Workspace-level: `cargo test --workspace --lib` exits 0 with the new tests from 1.1/2.1/2.2/3.1 included.
  - command: `cargo test --workspace --lib`
  - expected: red until all of 1.1-3.1 land (alias break) → green at task completion.

**Acceptance:**
- `cargo build --workspace` exits 0.
- `cargo check --workspace --tests` exits 0.
- `cargo test --workspace --lib` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 6.1
