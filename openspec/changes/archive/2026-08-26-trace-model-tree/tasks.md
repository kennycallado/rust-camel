# Tasks: trace-model-tree

## Task 1.1 — Wrap configured sampler in ParentBased (camel-otel)

**Files:**
- `crates/services/camel-otel/src/service.rs` (modified)

**Steps:**
1. In `to_sdk_sampler` (service.rs:308), wrap every arm in `Sampler::ParentBased`: `OtelSampler::AlwaysOn` → `Sampler::ParentBased(Box::new(Sampler::AlwaysOn))`, `OtelSampler::AlwaysOff` → `Sampler::ParentBased(Box::new(Sampler::AlwaysOff))`, `OtelSampler::TraceIdRatioBased(r)` → `Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(*r)))`. (Crates.io sdk 0.32 takes `Box<dyn ShouldSample>`; adapt the call if the constructor shape differs, not the semantics.)
2. Do NOT change the public `OtelSampler` enum or `OtelConfig`.
3. Update the doc comment on `to_sdk_sampler`: children inherit the parent sampling decision; an unsampled parent records nothing.

**Tests (existing `#[cfg(test)]` module in service.rs):**
- `name:` `sampler_always_on_wraps_parent_based`
  - `setup:` `OtelSampler::AlwaysOn`.
  - `action:` call `OtelService::to_sdk_sampler`.
  - `assert:` `matches!(result, Sampler::ParentBased(_))`.
- `name:` `sampler_ratio_based_wraps_parent_based`
  - `setup:` `OtelSampler::TraceIdRatioBased(0.5)`.
  - `action:` call `to_sdk_sampler`.
  - `assert:` `matches!(result, Sampler::ParentBased(_))`.
- `name:` `sampler_always_off_wraps_parent_based`
  - `setup:` `OtelSampler::AlwaysOff`.
  - `action:` call `to_sdk_sampler`.
  - `assert:` `matches!(result, Sampler::ParentBased(_))`.
- `name:` `sampler_unsampled_parent_drops_child`
  - `setup:` wrapped sampler from `TraceIdRatioBased(1.0)`; `SpanContext` with valid `TraceId`/`SpanId`, `TraceFlags::default()` (sampled flag off).
  - `action:` build `opentelemetry::Context` with that remote span context; call `sampler.should_sample(Some(&cx), trace_id, "child", SpanKind::Internal, &[], &[])`.
  - `assert:` `SamplingResult::Drop`.
- `name:` `sampler_sampled_parent_records_child_regardless_of_ratio`
  - `setup:` wrapped sampler from `TraceIdRatioBased(0.0)`; parent `SpanContext` with `TraceFlags::SAMPLED`.
  - `action:` same `should_sample` call with that parent context.
  - `assert:` `SamplingResult::RecordAndSample`.

**Command:** `cargo test -p camel-otel --lib sampler` — the two behavioral tests fail before, pass after.

**Acceptance:**
- `cargo test -p camel-otel --lib` exits 0.
- `cargo clippy -p camel-otel -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.

- [x] 1.1

## Task 1.2 — TracingProcessor: context restore, exception event, drop duration_ms attr

**Files:**
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified)
- `crates/camel-core/src/shared/observability/adapters/span_test_util.rs` (new, `#[cfg(test)]`)
- `crates/camel-core/src/shared/observability/adapters/mod.rs` (modified — `#[cfg(test)] pub(crate) mod span_test_util;`)
- `crates/camel-core/Cargo.toml` (modified — `[dev-dependencies]`: `opentelemetry_sdk = { workspace = true, features = ["testing"] }` — the `testing` feature gates `in_memory_exporter`)

**Steps:**
1. Add the dev-dependency with the `testing` feature.
2. Create `span_test_util.rs` with this exact contract:
   - NEW `pub(crate) async fn test_spans() -> TestSpans`. Internals: `static HARNESS: OnceLock<(SdkTracerProvider, Arc<InMemorySpanExporter>)>` plus `static LOCK: OnceLock<tokio::sync::Mutex<()>>` — the FIRST call builds one `InMemorySpanExporter` and one `SdkTracerProvider` with it, calls `global::set_tracer_provider` exactly once per process (0.32 replaces the provider); later calls reuse both. `test_spans()` awaits `LOCK`'s `lock_owned()` → `OwnedMutexGuard<()>`, calls `exporter.reset()`, and returns `TestSpans { provider, exporter, _guard }`. The guard serializes whole test bodies; spans cannot leak between tests. The fn is `async` because `lock_owned()` is awaited.
   - NEW `pub(crate) fn finish(spans: TestSpans) -> Vec<SpanData>` — consumes `TestSpans` (guard held), `provider.force_flush().expect("flush exported spans")` (synchronous in sdk 0.32; if the resolved signature is async, await it — semantics unchanged), then `exporter.get_finished_spans().expect("read exported spans")` (expect-with-reason satisfies lint-unwrap; `unwrap()` forbidden).
   - Module doc states: one global provider per test binary; tests filter returned spans by their own `trace_id` (set up via a parent span they create) as defense-in-depth.
3. In `TracingProcessor::call` (tracer.rs ~line 90):
   - Derive the step context from the captured `parent_cx`: replace `OtelContext::current_with_span(span)` with `parent_cx.with_span(span)`.
   - After `inner.call(exchange).await` returns `Ok(ex)`, restore `ex.otel_context = parent_cx.clone()` on the result exchange before returning it (inside the async body, on the result — never on `self` state). On `Err` there is no exchange to restore — the step span closes with the exception event and the error propagates.
4. Replace the error-branch event (tracer.rs ~line 233): `add_event("error", [error.type, error.message])` becomes `add_event("exception", [KeyValue::new("exception.type", error_class.to_string()), KeyValue::new("exception.message", e.to_string())])`. Keep `set_status(Status::error(...))` and the tracing-log field recordings unchanged.
5. Delete `cx.span().set_attribute(KeyValue::new("duration_ms", duration_ms as i64))` (tracer.rs ~line 204). Keep `tracing::Span::current().record("duration_ms", duration_ms)` and `metrics.record_exchange_duration`.
6. Make `capped_correlation_id` `pub(crate)`.
7. Extract the Minimal-level attribute array from `TracingProcessor::call` into NEW `pub(crate) fn step_span_attributes(route_id: &str, step_index: usize, correlation_id: &str) -> Vec<KeyValue>` returning `[messaging.system=camel, correlation_id (apply `capped_correlation_id` HERE — the single capping site; callers pass the raw id), route_id, step_id ("step-{index}"), step_index]`. Medium/Full extras (`headers_count`, `body_type`, `has_error`) stay inline in `TracingProcessor::call`, NOT in the helper.

**Tests (tracer.rs `mod tests`, via `span_test_util`):**
- `name:` `step_span_has_no_duration_ms_attribute`
  - `setup:` `let spans = test_spans().await`; create parent span `p` via global tracer and set `exchange.otel_context = OtelContext::current_with_span(p)`; record `p`'s span id and trace id. `TracingProcessor::new(IdentityProcessor, "r", 0, Minimal, None)`.
  - `action:` `ready().call(exchange).await`; `let all = finish(spans)`; filter `all` by the recorded trace id.
  - `assert:` span `r:step-0` exists; no attribute key `duration_ms`; its parent span id == `p`'s id.
- `name:` `step_restores_parent_context_after_call`
  - `setup:` as above, plus parent context carries baggage `baggage_test=1` (via `opentelemetry::baggage`).
  - `action:` call the processor; inspect the returned exchange directly (no span flush needed).
  - `assert:` returned `otel_context` active span id == `p`'s span id; `otel_context.baggage()` contains `baggage_test`.
- `name:` `step_error_emits_exception_event`
  - `setup:` parent span as in test 1; inner processor = locally defined `ErrProcessor` double (`call` returns `Err(CamelError::ProcessorError("boom".into()))`).
  - `action:` call; `finish`; filter by trace id.
  - `assert:` span `r:step-0` has exactly one event; name `exception`; non-empty `exception.type` and `exception.message`; status Error.

**Command:** `cargo test -p camel-core --lib tracer` — the three new tests fail before steps 3-5, pass after.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.
- `rg 'set_attribute\(KeyValue::new\("duration_ms"' crates/camel-core/src` — no hits.
- `rg 'record\("duration_ms"' crates/camel-core/src/shared/observability/adapters/tracer.rs` — still present (log field kept).

- [x] 1.2

## Task 1.3 — TracedPipeline route root span

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_tests.rs` (modified)
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified — `SpanEndGuard` becomes `pub(crate)`)

**Steps:**
1. In `compose_traced_pipeline` and `compose_traced_pipeline_with_contracts`: remove the empty-pipeline `IdentityProcessor` early return ONLY on the traced path (`trace_enabled == true`); empty traced routes fall through to `TracedPipeline` with zero steps. Untraced path unchanged.
2. In `TracedPipeline::call` (route_compiler.rs ~331), before invoking `run_steps`:
   - `entry_cx = exchange.otel_context.clone()`.
   - Root span: `global::tracer("camel-core").span_builder(route_id).with_kind(SpanKind::Internal).with_attributes([KeyValue::new("messaging.system","camel"), KeyValue::new("route_id", route_id), KeyValue::new("correlation_id", capped_correlation_id(exchange.correlation_id()))]).start_with_context(&tracer, &entry_cx)`.
   - `exchange.otel_context = entry_cx.with_span(root_span)`.
3. After `run_steps(...).await`, before mapping to the tower result: set root status from the outcome (`Ok`/`Completed`/`Stopped` → `Status::Ok`; `Failed(e)` → `Status::error(e.to_string())` plus the same `exception` event as Task 1.2 step 4). Restore `exchange.otel_context = entry_cx` only when an exchange comes back (`Ok` outcome); `Failed` maps to `Err` with no exchange — the root span closes and the error propagates. Root ends via `SpanEndGuard` (now `pub(crate)`).
4. Root span handle lives in the async body (not `self`) — hot-reload swaps unaffected.

**Tests (route_compiler_tests.rs; use `span_test_util` from Task 1.2):**
- `name:` `traced_pipeline_opens_root_span_with_step_children`
  - `setup:` `test_spans().await`; `compose_traced_pipeline` with 2 IdentityProcessor steps, route `rt`, `trace_enabled=true`; exchange with default (empty) context.
  - `action:` `pipeline.ready().call(exchange).await`; `finish`; filter by trace id (from the root span).
  - `assert:` span `rt` has no parent; `rt:step-0`, `rt:step-1` exist; both parent == root span id; both start after root start and end before root end.
- `name:` `traced_pipeline_nested_entry_roots_under_caller`
  - `setup:` create caller span `caller` via global tracer; `exchange.otel_context = OtelContext::current_with_span(caller)`.
  - `action:` call pipeline; `finish`; filter by caller trace id.
  - `assert:` span `rt` parent == caller span id; same trace id as caller.
- `name:` `traced_pipeline_restores_entry_context`
  - `setup:` as nested-entry; caller context carries baggage `outer=1`.
  - `action:` call; inspect returned exchange.
  - `assert:` returned active span id == caller span id; baggage `outer=1` present.
- `name:` `empty_traced_route_opens_and_closes_root`
  - `setup:` `test_spans().await`; create entry span `e` with baggage `keep=1` via global tracer; `exchange.otel_context = OtelContext::current_with_span(e)`; `compose_traced_pipeline(vec![], "empty", true, Minimal, None, None, ctx)`; retain the returned exchange.
  - `action:` call; `finish`; filter by `e`'s trace id.
  - `assert:` span `empty` exists (parent == `e`'s span id), status Ok, zero child spans; returned exchange's active span id == `e`'s span id and baggage `keep=1` present (restoration).
- `name:` `traced_pipeline_failed_root_records_exception`
  - `setup:` one step = locally defined `ErrProcessor` double (`Err(CamelError::ProcessorError("boom".into()))`), no handler.
  - `action:` call; `finish`; filter by trace id.
  - `assert:` root span `rt` status Error; exactly one event `exception` with non-empty `exception.type`/`exception.message`.

**Command:** `cargo test -p camel-core --lib traced_pipeline` — fails before, passes after.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.

- [x] 1.3

## Task 1.4 — Segment step spans for every attempt (initial + retries)

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_tests.rs` (modified)

**Steps:**
1. NEW `pub(crate) fn segment_span(tracer: &opentelemetry::global::GlobalTracer, route_id: &str, index: usize, entry_cx: &OtelContext, correlation_id: &str) -> opentelemetry::global::BoxedSpan` — starts `{route_id}:step-{index}` (kind Internal) with attributes from `step_span_attributes` (Task 1.2), parented by `entry_cx`.
2. NEW `struct TracedSegmentStep` (route_compiler.rs) implementing `camel_api::error_handler::RetryableStep` with the same method shape as the existing impls (see `RetryableStep` for `OutcomeSegment`, camel-api `error_handler.rs`): it holds the segment (`camel_api::OutcomeSegment` clone), `route_id: String`, `index: usize`. Its `invoke` does the full triple per call: `entry_cx = ex.otel_context.clone()`; span via `segment_span`; `ex.otel_context = entry_cx.with_span(span)`; await inner segment run; restore `ex.otel_context = entry_cx` on `Completed`/`Stopped` outcomes (those carry the exchange); `Failed` returns the error with no exchange to restore — the attempt's span closes with the `exception` event; `Completed`/`Stopped` → `Status::Ok`, `Failed` → `Status::error` + `exception` event; span ends (guard or explicit end before return — spans must not outlive the future). Retry inputs are the error handler's preserved pre-attempt exchange, which already carries the root context (restored by the previous attempt's Ok path or never left it on the first attempt).
3. In `run_steps`: when `trace` is set and the step is a `CompiledStep::Segment`, construct `TracedSegmentStep` and use it for BOTH the initial invocation AND `handler.retry_step(...)` — every attempt path (initial dispatch and per-attempt invokes inside the error-handler impls) goes through the adapter, so no code in camel-api's error_handler changes. When `trace` is off, the existing `OwnedRetryable::Segment` path is unchanged.
4. The `Process` arm keeps its `TracingProcessor` wrapper — no double-wrap.

**Tests (route_compiler_tests.rs; `span_test_util`):**
- `name:` `segment_step_opens_span_and_restores_root`
  - `setup:` `test_spans().await`; pipeline from `compose_traced_pipeline` with one `CompiledStep::Segment` (test double returning `PipelineOutcome::Completed(ex)` — reuse existing segment doubles in this file) then one IdentityProcessor step; route `srt`.
  - `action:` call; `finish`; filter by trace id.
  - `assert:` spans `srt` (root) and `srt:step-0` exist; `srt:step-0` parent == root; `srt:step-1` (the IdentityProcessor step) parent == root too, NOT the segment span.
- `name:` `segment_failure_records_exception_event`
  - `setup:` segment double returns `PipelineOutcome::Failed(CamelError::ProcessorError("boom".into()))`, no handler.
  - `action:` call; `finish`; filter.
  - `assert:` `srt:step-0` status Error with one `exception` event; root `srt` status Error with its own `exception` event.
- `name:` `segment_retry_attempts_each_get_span`
  - `setup:` `test_spans().await`; segment double fails on attempt 1; on attempt 2 it (a) creates a fragment via `camel_api::fragment_exchange` from the incoming exchange, (b) runs that fragment through a locally composed traced sub-pipeline `compose_traced_pipeline(..., "srt-sub", true, ...)` awaiting it to completion, then returns `Completed`; error handler = `DefaultRouteErrorHandler` with a retry/redelivery policy (pattern: camel-processor/src/error_handler.rs ~line 1187 and camel-core/tests/continued_e2e.rs ~line 54).
  - `action:` call; `finish`; filter by trace id.
  - `assert:` exactly two spans named `srt:step-0`, both parented by root; a span `srt-sub` exists whose parent is the SECOND `srt:step-0` span (retry fragments nest under the retry attempt's span, not the route root); the double's recorded second-attempt active span id equals the second `srt:step-0` span id.
- `name:` `untraced_segment_emits_no_span`
  - `setup:` `test_spans().await`; same segment pipeline but `trace_enabled=false`.
  - `action:` call; `finish` (no filter).
  - `assert:` zero spans named `srt:step-0`.

**Command:** `cargo test -p camel-core --lib segment` — fails before, passes after.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.

- [x] 1.4

## Task 1.5 — Splitter context-inheritance doc contract (camel-api)

**Files:**
- `crates/camel-api/src/splitter.rs` (modified)

**Steps:**
1. Rewrite the doc comment above `fragment_exchange` (splitter.rs ~line 287) to state: fragments inherit the LIVE segment step span context (the splitter step's span — not the route root, not a previous fragment's span); fragment-driven sub-route roots open as children of that segment span in the same trace; entry-context restoration is the segment wrapper's job (route_compiler `TracedSegmentStep`), not `fragment_exchange`'s.
2. Scan the rest of splitter.rs doc comments for any claim contradicting that contract (e.g. "inherits parent's span for chaining"); update contradictions to the nesting wording. No code changes.

**Tests:**
- `name:` `test_fragment_exchange_inherits_otel_context` (EXISTING, splitter.rs:474)
  - `setup:` unchanged.
  - `action:` unchanged.
  - `assert:` still passes (`fragment_exchange` behavior is unchanged; doc-only edit).

**Command:** `cargo test -p camel-api --lib test_fragment_exchange_inherits_otel_context` — passes before and after.

**Acceptance:**
- `cargo test -p camel-api --lib splitter` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.
- Doc comment above `fragment_exchange` contains "segment step span" and "same trace".

- [x] 1.5

## Task 1.6 — Integration: trace tree shape end-to-end (camel-test)

**Files:**
- `crates/camel-test/Cargo.toml` (modified — `[dev-dependencies]`: `opentelemetry = { workspace = true }` AND `opentelemetry_sdk = { workspace = true, features = ["testing"] }` — camel-test has no otel deps today and the harness needs the global-tracer install from `opentelemetry` plus the in-memory exporter from the sdk)
- `crates/camel-test/tests/otel_direct_hop_regression.rs` (modified)
- `crates/camel-test/tests/otel_trace_tree_test.rs` (new)

**Steps:**
1. Add the dev-dependency. The existing `otel_direct_hop_regression.rs` runs on the noop global provider with body-based assertions only — its deadlock/readiness semantics are untouched. Run it first; if any assertion encodes the OLD chained span shape, update it to root+children; body-based asserts stay as-is.
2. Create `otel_trace_tree_test.rs` with its own harness (each integration-test file is its own binary; camel-core's `span_test_util` is `#[cfg(test)]`-private and NOT importable — replicate the EXACT contract of Task 1.2's `span_test_util` locally): NEW `async fn test_spans() -> TestSpans` returning `TestSpans { provider: SdkTracerProvider, exporter: Arc<InMemorySpanExporter>, _guard: tokio::sync::OwnedMutexGuard<()> }` (single `OnceLock`-installed provider + `OnceLock<tokio::sync::Mutex<()>>`, `reset()` in setup) + NEW `fn finish(spans: TestSpans) -> Vec<SpanData>` consuming the guard, `provider.force_flush().expect("flush exported spans")` (sync in sdk 0.32; await instead if the resolved signature is async), then `.expect("read exported spans")`; tests filter by their run's trace id.
3. Build routes with the same `CamelTestContext` wiring `otel_direct_hop_regression.rs` uses:
   - Route A `tree-main`: 3 process steps, step 1 dispatches `direct:tree-sub`.
   - Route B `tree-sub`: 2 process steps.
   - Route C `tree-split`: 1 split segment producing 2 fragments, each fragment dispatched to `direct:tree-sub`.
   - Drive one exchange through A (flush between runs via per-test `test_spans()`), and separately one through C.
4. Tree 1 assertions (filtered to that run's trace id): span `tree-main` root; `tree-main:step-0/1/2` siblings under root; `tree-sub` root child of `tree-main:step-1`; `tree-sub:step-0/1` children of `tree-sub`; every child's `[start,end]` contained in its parent's; `tree-main:step-1` starts after `tree-main:step-0` ends (sequential sibling ordering); one trace id for the run.
5. Tree 2 assertions: `tree-split` root; `tree-split:step-0` (segment span) child of root; two `tree-sub` roots, each child of `tree-split:step-0`; single trace id for the run.
6. Scan `examples/otel-demo/src/main.rs` doc comments for trace-shape claims (step chaining wording); update to root+children. No demo code changes.

**Tests:**
- `name:` `direct_hop_nests_subroute_root_under_caller_step` (otel_trace_tree_test.rs)
  - `setup:` routes A and B started.
  - `action:` one exchange through A; flush.
  - `assert:` Tree 1 parentage + containment + sibling ordering.
- `name:` `split_fragments_nest_under_segment_span_one_trace`
  - `setup:` routes C and B started.
  - `action:` one exchange through C; flush.
  - `assert:` Tree 2 parentage; exactly one trace id among that run's spans.
- existing `otel_direct_hop_regression.rs` tests: all pass after any span-shape assertion updates.

**Command:** `cargo test -p camel-test --test otel_direct_hop_regression --test otel_trace_tree_test` — new tests fail before Tasks 1.2-1.4, pass after.

**Acceptance:**
- That command exits 0.
- `cargo clippy -p camel-test -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.
- `rg -n '"duration_ms"' crates/camel-test/tests/otel_direct_hop_regression.rs crates/camel-test/tests/otel_trace_tree_test.rs` — no span-attribute expectation on `duration_ms`.

- [x] 1.6
