# Tasks: language-failure-observability

## camel-core

### Task 1.1: Emit the evaluation-failure metric once per top-level failure

**Files:**
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified)
- `crates/camel-core/src/shared/observability/adapters/tracer_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_tests.rs` (modified)

**Steps:**
1. Add `pub(crate) fn record_language_eval_failure(metrics: &dyn MetricsCollector, current_route_id: &str, error: &CamelError)` to `tracer.rs`. It returns unless `error` is `CamelError::ExpressionFailed { .. }` whose `route_id == current_route_id`. It then calls `metrics.record_counter("language_eval_failures_total", 1.0, &[("language", language.as_str()), ("route", route_id.as_str()), ("verb", verb.as_str()), ("class", class.as_str())])`. Annotate the call with `// allow-open-label rc-5r117 (language/route/verb: operator-authored route config, bounded by route definitions; class: closed ExpressionErrorClass set)`.
2. In `run_steps`, at the top of the `PipelineOutcome::Failed(err)` arm (before the `handler` early return), call `record_language_eval_failure(ctx.metrics.as_ref(), route_id, &err)` — the `route_id` parameter of `run_steps`, not `ctx.route_id` (production passes the same value; tests set both explicitly). Do not add it to `TracingProcessor` or to language crates.
3. Add a `record_counter` capture to the `RecordingMetrics` double in `tracer_tests.rs` (push `record_counter:<name>:k=v:...`), and add a separate local double in `route_compiler_tests.rs` that records `record_counter` calls (the tracer-test double is private to that module). The `run_steps` tests construct `PipelineRuntimeCtx { metrics: recorder, route_id: "r1".into() }` directly; `compile_time()` would use `NoOpMetrics` with an empty route id and trip the foreign-route guard.
4. Add a doc comment on the helper stating: handler-recovered failures count; handler retries do not re-enter `run_steps`; handler-raised failures and wrapped causes are out of scope; all label values are config-derived.

**Tests:**
- `record_language_eval_failure_counts_typed_labels`: recorder + `ExpressionFailed { language: "rhai", route_id: "r1", step_id: "set_property#0", verb: "set_property", class: Arithmetic, position: Some(3,8), conversion: None, cause: None }`, current route `"r1"` → action `record_language_eval_failure(...)` → assert exactly one call `language_eval_failures_total`, value `1.0`, labels exactly `[("language","rhai"),("route","r1"),("verb","set_property"),("class","arithmetic")]`. Command: `cargo test -p camel-core --lib record_language_eval_failure_counts_typed_labels`. Expected: fails before the helper exists, passes after.
- `record_language_eval_failure_skips_foreign_route`: same error with current route `"outer"` → assert recorder has zero calls.
- `record_language_eval_failure_skips_non_expression_errors`: `CamelError::ProcessorError("x")` → assert zero calls.
- `run_steps_emits_language_metric_for_segment_failure`: a `CompiledStep::Segment` whose `OutcomePipeline::run` returns `PipelineOutcome::Failed(ExpressionFailed { language:"rhai", route_id:"r1", verb:"filter", class: TypeMismatch, .. })`; `PipelineRuntimeCtx { metrics: recorder, route_id: "r1" }`; call `run_steps(...)` → assert one `language_eval_failures_total` call with `verb = "filter"`, `class = "type-mismatch"`, `route = "r1"`. Command: `cargo test -p camel-core --lib run_steps_emits_language_metric_for_segment_failure`. Expected: passes only after step 2.
- `run_steps_does_not_emit_for_foreign_route_failure`: same segment returns `ExpressionFailed { route_id: "sub" }`, ctx route `"r1"` → assert zero calls (sub-route double-count guard).
- `run_steps_emits_once_for_handler_recovered_failure`: a failing `CompiledStep::Process` returning `ExpressionFailed { route_id: "r1", .. }` plus a stub `RouteErrorHandler` whose `handle_step` returns `Ok(StepDisposition::Handled(ex))` → call `run_steps` → assert exactly one `language_eval_failures_total` call.
- `run_steps_emits_once_for_retried_failure`: same failing step plus a stub handler whose `retry_step` invokes the step twice (two evaluations) then returns `RetryOutcome::Exhausted`, and whose `handle_step` returns `StepDisposition::Propagate(error)` → call `run_steps` → assert exactly one `language_eval_failures_total` call (retries never re-enter the `Failed` arm).

**Acceptance:**
- `cargo test -p camel-core --lib record_language_eval_failure` exits 0.
- `cargo test -p camel-core --lib run_steps_emits_once_for_handler_recovered_failure` exits 0.
- `cargo test -p camel-core --lib run_steps_emits_once_for_retried_failure` exits 0.
- `cargo test -p camel-core --lib run_steps_emits_language_metric_for_segment_failure` exits 0.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-metric-labels` exits 0.
- No new `MetricsCollector` method and no change to `NoOpMetrics`, `MetricsHandle`, or `CompositeMetricsCollector`.

- [x] 1.1

### Task 1.2: Record class and position on the failing step span

**Files:**
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_span_tests.rs` (modified)

**Steps:**
1. In `record_exception`, after the existing status/event writes, add: when `e` is `CamelError::ExpressionFailed { class, position, .. }`, set OTel attributes `camel.expression.class` = `class.as_str()` and, only when `position` is `Some(p)`, `camel.expression.position` = `format!("{}:{}", p.line, p.column)`. Do not write any attribute when `position` is `None`.
2. In the process tracer's `tracing::info_span!` declaration add `expression_class = tracing::field::Empty` and `expression_position = tracing::field::Empty`.
3. In the `Err(e)` arm of `TracingProcessor::call`, when `e` is `ExpressionFailed`, record `expression_class` = `class.as_str()` and, only when `position` is `Some(p)`, `expression_position` = `"line:column"` on `tracing::Span::current()`. Do not record either field for other error variants.
4. Keep the existing `exception.*` event and `error_type`/`error` fields unchanged.

**Note:** `record_exception` is shared by process step spans, segment step spans, `finish_span_outcome`'s route root span, and `trace_restart` split-item spans. Accepting the two new attributes on all of them is intentional: they describe the failure carried by that span, and the spec does not restrict them to step spans. Record this in the helper's doc comment.

**Tests:**
- `process_step_span_carries_expression_class_and_position`: `test_spans()`; a failing `CompiledStep::Process` returning `ExpressionFailed { class: Arithmetic, position: Some(ErrorPosition { line: 3, column: 8 }), .. }`; `compose_traced_pipeline(vec![step], "rt", true, DetailLevel::Minimal, None, None, PipelineRuntimeCtx::compile_time())` (the same literal `true` form as the existing tests in that file); call once → collect exported span named `rt:step-0` → assert an attribute `camel.expression.class` = `"arithmetic"` and `camel.expression.position` = `"3:8"`. Command: `cargo test -p camel-core --lib process_step_span_carries_expression_class_and_position`. Expected: fails before step 1.
- `process_step_span_omits_position_when_absent`: same with `position: None` → assert class attribute present and no `camel.expression.position` attribute.
- `segment_step_span_carries_expression_attributes`: `run_steps` with `trace = true` and a `CompiledStep::Segment` returning `ExpressionFailed { class: TypeMismatch, position: Some(2,4), .. }`; ctx `compile_time()` → assert the exported segment step span carries `camel.expression.class = "type-mismatch"` and `camel.expression.position = "2:4"`.
- `process_step_tracing_fields_carry_expression_class_and_position`: a `tracing_subscriber` test capture (or a scoped `tracing::subscriber::with_default` visitor) over the same failing step → assert the `step` span's `expression_class = "arithmetic"` and `expression_position = "3:8"` fields. Command: `cargo test -p camel-core --lib process_step_tracing_fields_carry_expression_class_and_position`. Expected: fails before steps 2-3.

**Acceptance:**
- `cargo test -p camel-core --lib expression_class_and_position` exits 0.
- `cargo test -p camel-core --lib segment_step_span_carries_expression_attributes` exits 0.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-log-levels` exits 0 (no new log sites).

- [x] 1.2

## camel-cli

### Task 1.3: Pin the metric labels for Simple, Rhai, JsonPath and XPath failures

**Files:**
- `crates/camel-cli/tests/language_failure_observability_test.rs` (new)

**Steps:**
1. Add a `RecordingMetrics` test double implementing `MetricsCollector` (five required methods; `record_counter` appends `(name, labels)` to a `Mutex<Vec<...>>`).
2. Add a helper `run_route(language_yaml: &str, body: &str) -> Vec<CounterCall>`: build `CamelContext::builder().metrics(Arc::new(recorder.clone())).build()`, register `DirectComponent` + `MockComponent`, parse a one-route YAML with `camel_dsl::parse_routes_with_env` (top-level `routes:`), `add_route_definition`, `start()`, deliver one exchange through a `direct:` producer via `ServiceExt::oneshot`, then `stop()`. Return the recorded calls.
3. Each test uses `from: "direct:start"`, one `set_body` step with the language value source, and input body as named. Gate the rhai/jsonpath/xpath tests with `#[cfg(feature = "lang-rhai")]` / `#[cfg(feature = "lang-jsonpath")]` / `#[cfg(feature = "lang-xpath")]` so lang-free builds still compile.

**Tests:**
- `metric_labels_for_simple_evaluation_failure`: route step `set_body: { simple: "${lang:nope:body}" }`, body `"x"` → assert one `language_eval_failures_total` with labels `language="simple"`, `route="metric-route"`, `verb="set_body"`, `class="runtime"`. Command: `cargo test -p camel-cli --test language_failure_observability_test metric_labels_for_simple_evaluation_failure`.
- `metric_labels_for_rhai_evaluation_failure`: step `set_body: { rhai: "nonexistent_fn()" }` → assert `language="rhai"`, `verb="set_body"`, `class="function-not-found"`.
- `metric_labels_for_jsonpath_evaluation_failure`: step `set_body: { jsonpath: "$.x" }`, body `"not json"` → assert `language="jsonpath"`, `verb="set_body"`, `class="conversion"`.
- `metric_labels_for_xpath_evaluation_failure`: step `set_body: { xpath: "/a" }`, body `"not xml"` → assert `language="xpath"`, `verb="set_body"`, `class="conversion"`.
- `metric_labels_exclude_exchange_data`: rhai route with body `"SECRET-SENTINEL"` → assert no recorded label key or value contains `"SECRET-SENTINEL"` and no label value contains `"Processor error"`.
- Each test also asserts exactly one matching call (no double count).

**Acceptance:**
- `cargo test -p camel-cli --test language_failure_observability_test` exits 0.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.
- No test uses sleeps (other than existing bounded helpers), free ports, or raw HTTP servers.

- [x] 1.3

### Task 1.4: Pin typed evaluation output in `camel test`

**Files:**
- `crates/camel-cli/tests/test_runner.rs` (modified)

**Steps:**
1. Add one unit-tier test using the existing `run(yaml, dir)` helper. Route `r1` consumes `direct:start`, its first step is `filter: { rhai: "nonexistent_fn()", steps: [ { to: "mock:out" } ] }`, input body is `"SECRET-SENTINEL"`, and `expects: mock:out: { count: 1 }`.
2. Assert `result.doc_error` is `Some(text)` and the text contains `"function-not-found"`, `"filter#0"`, `` "`filter`" `` (the verb), and `" at 1:"` (the language-reported position).
3. Assert the text does not contain `"SECRET-SENTINEL"` (redaction preserved).
4. Do not change exit-code classification or add any production code.

**Tests:**
- `camel_test_reports_typed_filter_evaluation_failure`: setup as above → run `run(yaml, dir)` → assert `doc_error.unwrap()` contains `function-not-found`, `filter#0`, `` `filter` ``, ` at 1:`; assert no `SECRET-SENTINEL`. Command: `cargo test -p camel-cli --test test_runner camel_test_reports_typed_filter_evaluation_failure`. Expected: passes once the route surfaces the typed error (existing production behavior). If it fails, pick a failing expression that reports a position (the language crate already pins `nonexistent_fn()` as position-bearing) — do not weaken the contract and do not add production code.

**Acceptance:**
- `cargo test -p camel-cli --test test_runner camel_test_reports_typed_filter_evaluation_failure` exits 0.
- Existing `test_runner` tests still pass: `cargo test -p camel-cli --test test_runner` exits 0.

- [x] 1.4

## docs

### Task 1.5: Document the metric label table and `camel test` diagnostics

**Files:**
- `docs/src/services/metrics.md` (modified)
- `docs/src/testing/camel-test.md` (modified)

**Steps:**
1. Create a new `## Metric families` section in `docs/src/services/metrics.md` (it does not exist yet) with a markdown table header `| Metric | Type | Labels | Values | Disableable |` and a `|---|---|---|---|---|` separator. Add one row for `camel_language_eval_failures_total` (counter; labels `language`, `route`, `verb`, `class`; values described; non-disableable like the error family) and one context row for `camel_errors_total`.
2. State in that section that label values come from operator-authored route configuration and the typed error class, never from exchange data, and that the counter increments once per top-level evaluation failure (handler retries do not increment).
3. In `docs/src/testing/camel-test.md`, add a `## Typed evaluation failures` section: the document-error line renders the typed `CamelError` display with class, verb, step id, route id and position when the language reports one; a handled failure is observable only through metrics/spans; output stays redacted (no exchange data).
4. Do not add `ignore` code fences; use prose and inline `code` so `mdbook test` stays green.

**Tests:**
- Documentation-only task; no Rust test. Verification: `cd docs && mdbook test` exits 0; `grep -q "camel_language_eval_failures_total" docs/src/services/metrics.md` and `grep -q "Typed evaluation failures" docs/src/testing/camel-test.md` both succeed.

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- The metrics table names `camel_language_eval_failures_total` and all four label keys.
- The `camel test` section names class, position and step id and the redaction rule.

- [x] 1.5
