# Tasks: splittrace

## camel-api

### Task 1.1: SplitterConfig trace threshold field + canonical spec plumbing

**Files:**
- `crates/camel-api/src/splitter.rs` (modified)
- `crates/camel-api/src/runtime.rs` (modified)

**Steps:**
1. In `SplitterConfig` (splitter.rs:212) add field
   `/// Threshold above which split fragments start new traces (0 = off).`
   `pub trace_item_threshold: usize,`
2. In `SplitterConfig::new` (splitter.rs:250) set
   `trace_item_threshold: 100` in the struct literal (same pattern as
   `max_fragments: 100_000`).
3. Add builder method (mirror `parallel_limit` style):
   `pub fn trace_item_threshold(mut self, threshold: usize) -> Self`.
4. Extend the manual `Debug for SplitterConfig` impl (splitter.rs:235) with
   `.field("trace_item_threshold", &self.trace_item_threshold)`.
5. In `crates/camel-api/src/runtime.rs` `CanonicalStepSpec::Split`
   (runtime.rs:138) add field
   `trace_item_threshold: Option<usize>,`. Fix every exhaustive
   `match`/destructure site on `CanonicalStepSpec::Split` in that file
   (e.g. runtime.rs:451) by threading the new field the same way
   `parallel_limit` is threaded.

**Tests:** (add to the existing `#[cfg(test)]` mod in splitter.rs)
- `trace_item_threshold_defaults_to_100`: `SplitterConfig::new(split_body_lines())` → `assert_eq!(config.trace_item_threshold, 100)`.
- `trace_item_threshold_builder_sets_value`: `.trace_item_threshold(0)` → `assert_eq!(config.trace_item_threshold, 0)`; and `.trace_item_threshold(7)` → 7.
- `canonical_split_spec_carries_trace_item_threshold`: construct `CanonicalStepSpec::Split` with `trace_item_threshold: Some(5)` and one with `None`; assert both compile (field exists and is threaded; if runtime.rs has an accessor/equality path for `parallel_limit`, mirror the assertion there).

**Acceptance:**
- `cargo test -p camel-api` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo fmt --check` clean for touched files.

- [x] 1.1

## camel-processor

### Task 1.2: SplitSegment stamps fragment metadata

**Files:**
- `crates/camel-processor/src/split_segment.rs` (modified)

**Steps:**
1. In `SplitSegment::run` (the async block that starts with
   `let original = exchange;`), after
   the `if fragments.is_empty()` early return and BEFORE the
   `parallel_split`/`sequential_split` dispatch, stamp metadata on each
   fragment — exact parity with `SplitterService::call`
   (splitter.rs:122-126):
   ```rust
   let total = fragments.len();
   for (i, frag) in fragments.iter_mut().enumerate() {
       frag.set_property(CAMEL_SPLIT_INDEX, Value::from(i as u64));
       frag.set_property(CAMEL_SPLIT_SIZE, Value::from(total as u64));
       frag.set_property(CAMEL_SPLIT_COMPLETE, Value::Bool(i == total - 1));
   }
   ```
   Import `CAMEL_SPLIT_INDEX`, `CAMEL_SPLIT_SIZE`, `CAMEL_SPLIT_COMPLETE`
   from `crate::splitter` (they are `pub const` there, values
   `CamelSplitIndex` / `CamelSplitSize` / `CamelSplitComplete`).
2. Note: `fragments` must become `mut` at the binding site if it is not
   already.

**Tests:** (in the existing tests module of split_segment.rs; use a custom
splitter closure producing N fragment exchanges — the file already has
custom-splitter helpers, e.g. the 3-fragment splitter at line ~528)
- `sequential_split_stamps_fragment_metadata`: 3 fragments via custom splitter, body that captures `exchange.get_property("CamelSplitIndex")`, `get_property("CamelSplitSize")`, `get_property("CamelSplitComplete")` into an `Arc<Mutex<Vec<(u64,u64,bool)>>>` → run `SplitSegment` sequentially → assert the captured triples are `(0,3,false)`, `(1,3,false)`, `(2,3,true)` in order.
- `parallel_split_stamps_fragment_metadata`: same splitter with `parallel: true` → collect triples → sort by index → same three triples as above (complete flag only on index 2).
- `single_fragment_is_complete`: 1 fragment → `(0, 1, true)`.

**Acceptance:**
- `cargo test -p camel-processor --lib split_segment` exits 0 (all
  existing split_segment tests stay green).
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `cargo fmt --check` clean.

- [x] 1.2

## camel-dsl

### Task 1.3: YAML trace_item_threshold knob threading + schema regen

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-core/src/lifecycle/application/commands.rs` (modified —
  the `CanonicalStepSpec::Split` → `BuilderStep::Split` conversion near
  line 1032: destructure the new field and apply
  `.trace_item_threshold(trace_item_threshold.unwrap_or(100))` to both
  `SplitterConfig` constructions, same rule as the parallel_limit
  threading in that arm)
- `schemas/dsl/route-schema.json` (modified, regenerated)
- `schemas/canonical-route-spec.json` (modified, regenerated)

**Steps:**
1. `route_ast.rs` `SplitData` (line 925): add
   `#[serde(default)] pub trace_item_threshold: Option<usize>,` next to
   `parallel_limit`.
2. `model.rs` `SplitStepDef` (line 381): add
   `pub trace_item_threshold: Option<usize>,`.
3. `yaml.rs` split arm (the `SplitStepDef` construction near line 1160):
   thread `trace_item_threshold: split.trace_item_threshold`.
4. `compile.rs` `compile_split_step_to_canonical` (line 1700): add
   `trace_item_threshold: def.trace_item_threshold` to the
   `CanonicalStepSpec::Split` literal.
5. `compile.rs` canonical→builder sites (the `SplitterConfig::new(<expression>)`
   constructions near lines 617 and 632, and any sibling site near 1824 and
   1839): after the existing builder chain add
   ```rust
   let config = config.trace_item_threshold(
       trace_item_threshold.unwrap_or(100),
   );
   ```
   binding `trace_item_threshold` from the destructured canonical split
   spec alongside `parallel_limit`.
6. Regenerate schemas: run `cargo xtask schema` (write mode) from the
   worktree root; commit the regenerated `schemas/dsl/route-schema.json`
   and `schemas/canonical-route-spec.json`.

**Tests:**
- In camel-dsl tests (model.rs or yaml.rs test mod, follow where existing
  SplitData round-trips live — model.rs:930-940 has the precedent):
  `split_trace_item_threshold_parses`: YAML `split:` block with
  `trace_item_threshold: 5` → parsed `SplitData.trace_item_threshold ==
  Some(5)`; without the key → `None`.
- In compile.rs tests: `split_trace_item_threshold_default_is_100`: compile
  a canonical split spec with `trace_item_threshold: None` → resulting
  `BuilderStep::Split` config has `trace_item_threshold == 100`;
  `split_trace_item_threshold_zero_is_threaded`: `Some(0)` → config `0`.

**Acceptance:**
- `cargo test -p camel-dsl` exits 0.
- `cargo xtask schema --check` exits 0.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 1.3

## camel-core

### Task 2.1: TraceRestartBody — per-item linked root spans above threshold

**Files:**
- `crates/camel-core/src/lifecycle/adapters/trace_restart.rs` (new)
- `crates/camel-core/src/lifecycle/adapters/mod.rs` (modified — add module)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/split_trace_restart_tests.rs` (new)

**Steps:**
1. New module `trace_restart.rs`:
   ```rust
   pub(crate) struct TraceRestartBody {
       inner: camel_api::OutcomeSegment,
       route_id: Arc<str>,
       threshold: usize,
   }
   ```
   with `pub(crate) fn wrap(inner: camel_api::OutcomeSegment, route_id:
   Arc<str>, threshold: usize) -> camel_api::OutcomeSegment` returning
   `OutcomeSegment::new(Box::new(TraceRestartBody { .. }))`.
2. Implement `camel_api::OutcomePipeline for TraceRestartBody`:
   - `clone_box` clones inner.
   - `run(mut exchange)`: read
     `exchange.get_property("CamelSplitSize")` (the `camel_api::Value`
     variant produced by `Value::from(u64)` in splitter.rs — match that
     variant; missing property → treat as below threshold). If the total
     is `<= threshold` → `self.inner.run(exchange)` unchanged (nested
     mode, zero span overhead).
   - Above threshold: `let origin_cx = exchange.otel_context.clone();`
     `let origin_sc = origin_cx.span().span_context().clone();`
     `let tracer = global::tracer(<same InstrumentationScope value that
     segment_span in route_compiler.rs:760 uses>)`; build the item root:
     ```rust
     let span = tracer
         .span_builder(format!("{}:split-item", self.route_id))
         .with_kind(SpanKind::Internal)
         .with_attributes(vec![
             KeyValue::new("split.item.index", index as i64),
             KeyValue::new("split.item.total", total as i64),
         ])
         .with_links(vec![Link::new(origin_sc, Vec::new())])
     ```
     started via `tracer.build_with_context(span_builder,
     &OtelContext::new())` (empty context = new trace id, no parent span
     id). `index` comes from `get_property("CamelSplitIndex")`. Set
     `exchange.otel_context = OtelContext::new().with_span(span);`
     Run `self.inner.run(exchange).await`; then set span status per
     outcome (Ok on Completed/Stopped, record the exception on Failed —
     reuse the same status semantics as `finish_span_outcome` in
     route_compiler.rs:802; if it is not visible from the new module,
     replicate the small match), restore
     `ex.otel_context = origin_cx` on exchange-carrying outcomes, end the
     item span, and return the outcome.
3. Wire in `splitting.rs`: in BOTH the `BuilderStep::Split` arm and the
   `BuilderStep::DeclarativeSplit` arm, after
   `let body_segment = compose_outcome_segment(sub_segments);` wrap when
   enabled:
   ```rust
   let body_segment = if config.trace_item_threshold >= 1 {
       trace_restart::TraceRestartBody::wrap(
           body_segment,
           route_id_arc,
           config.trace_item_threshold,
       )
   } else {
       body_segment
   };
   ```
   `route_id` is available on the `CompilationContext` used by these arms
   (locate the route_id accessor the file already uses; if the Declarative
   arm binds different names, apply the same rule there).
4. New test file `split_trace_restart_tests.rs` using the existing
   `span_test_util` harness (`crate::shared::observability::adapters::
   span_test_util` — see route_compiler_span_tests.rs for the usage
   contract). Build pipelines by hand (no full route compile): a
   `camel_processor::SplitSegment` whose `body` is
   `TraceRestartBody::wrap(step_segment, "r".into(), T)` where
   `step_segment` is a segment producing one step span, driven by
   `compose_traced_pipeline`-equivalent tracing from route_compiler tests
   (reuse the minimal traced-pipeline helper those tests use; if that
   helper is private, replicate the 3-line root-span setup from
   route_compiler_span_tests.rs).

**Tests:** (all in split_trace_restart_tests.rs)
- `at_threshold_stays_nested_single_trace`: threshold 2, splitter yields 2 fragments → collect spans → all spans share one trace id; no exported span carries links (`span.links.is_empty()` on `SpanData`); step spans nest under the split segment span (`parent_span_id` chain).
- `above_threshold_one_trace_per_item_with_link`: threshold 2, splitter yields 3 fragments → assert: exactly 3 spans named `r:split-item`; each has a trace id distinct from the route root trace and distinct from the other item roots; each has `parent_span_id == None` (SpanData exposes the parent via the span context — assert `span.parent_span_id.is_none()` per the SpanData field); each carries exactly 1 link whose span context equals the split segment span's span context; each item's step span nests under its item root.
- `zero_threshold_disables_restart`: threshold 0 wiring rule — construct the body WITHOUT the wrapper (mirroring the compiler's `>= 1` gate) with 4 fragments → single trace, no links. (Asserts the compiler-gate semantics; the gate itself is compile-time wiring.)
- `parallel_above_threshold_links_survive`: threshold 1, 3 fragments, `parallel: true` → 3 item roots, each linked, outcomes still aggregate (body outcome equals the sequential aggregation result).
- `failed_fragment_marks_item_span_error`: threshold 1, 2 fragments, second fragment body returns `PipelineOutcome::Failed` → that item root span has error status recorded and the item span is ended (exported).

**Acceptance:**
- `cargo test -p camel-core --lib split_trace_restart` exits 0.
- `cargo test -p camel-core --lib` exits 0 (no regression in route_compiler_span_tests).
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo fmt --check` clean.

- [x] 2.1

### Task 2.1b: DeclarativeSplit carries the threshold (spec-gap fix, bd rc-sdq82)

**Files:**
- `crates/camel-api/src/splitter.rs` (modified)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-core/src/lifecycle/application/commands.rs` (modified)
- `crates/camel-builder/src/lib.rs` (modified)

**Steps:**
1. `camel-api/src/splitter.rs`: add
   `pub const DEFAULT_TRACE_ITEM_THRESHOLD: usize = 100;` and use it in
   `SplitterConfig::new` (single source for the default).
2. `route_definition.rs` `BuilderStep::DeclarativeSplit` (line ~114): add
   `trace_item_threshold: Option<usize>,` beside `parallel_limit`.
3. `splitting.rs` `BuilderStep::DeclarativeSplit` arm: wrap the composed
   body exactly like the Split arm, resolving
   `trace_item_threshold.unwrap_or(DEFAULT_TRACE_ITEM_THRESHOLD)`.
4. `compile.rs` Language arms (lines ~652 and ~1862): thread the field
   from the canonical spec / `SplitStepDef`.
5. `commands.rs` `CanonicalSplitExpressionSpec::Language` arm (~1080):
   thread `trace_item_threshold` from the canonical destructure.
6. `camel-builder/src/lib.rs` (~1095): name the new field in the
   destructure; the programmatic language-split builder keeps
   `trace_item_threshold: None` (default applies at wrap; knob surface
   is YAML + SplitterConfig — asymmetry noted as deferral).

**Tests:**
- camel-dsl compile test `declarative_language_split_threads_trace_threshold`: a canonical split spec with `CanonicalSplitExpressionSpec::Language` + `trace_item_threshold: Some(4)` compiles to `BuilderStep::DeclarativeSplit` carrying `Some(4)`; `None` threads as `None`.

**Acceptance:**
- `cargo test -p camel-dsl -p camel-core --lib` exits 0.
- `cargo clippy -p camel-api -p camel-core -p camel-dsl -p camel-builder -- -D warnings` exits 0.
- `cargo fmt --check` clean.

- [x] 2.1b

## camel-otel

### Task 3.1: LinkAwareSampler — links carry the sampling decision

**Files:**
- `crates/services/camel-otel/src/service.rs` (modified)
- `crates/services/camel-otel/src/sampler_tests.rs` (modified)

**Steps:**
1. In service.rs add (module-private):
   ```rust
   /// Root-sampler wrapper: a parentless span whose builder carries
   /// links inherits the first link's sampling flag (OTEL sampler-links
   /// guidance). Parented decisions never reach this sampler (it sits
   /// inside `Sampler::ParentBased`'s root slot); parentless spans
   /// without links delegate to the inner root sampler unchanged.
   #[derive(Clone, Debug)]
   struct LinkAwareSampler { inner: Sampler }
   ```
   Implement `opentelemetry_sdk::trace::ShouldSample for LinkAwareSampler`:
   - if `links` is non-empty → `SamplingResult { decision: if
     links[0].span_context().is_sampled() { RecordAndSample } else {
     Drop }, ..same defaults the SDK's own samplers produce (empty
     attributes, trace_state from parent_context or default) }`
   - else → `self.inner.should_sample(parent_context, trace_id, name,
     span_kind, attributes, links)`.
2. Change `to_sdk_sampler` (service.rs:324): wrap the inner root in the
   new wrapper:
   `OtelSampler::AlwaysOn => Sampler::ParentBased(Box::new(LinkAwareSampler { inner: Sampler::AlwaysOn }))`
   and the same for `AlwaysOff` and `TraceIdRatioBased(ratio)`.
   `LinkAwareSampler` must be `Clone + Debug + 'static` to satisfy
   `Box<dyn ShouldSample>` (note: the SDK requires `ShouldSample +
   Clone + 'static` via `CloneShouldSample`; derive accordingly — if
   `Sampler`'s Debug/Clone derives suffice, plain derives work).

**Tests:** (in sampler_tests.rs, following its existing style)
- `link_aware_root_follows_sampled_link`: build `Sampler::ParentBased(Box::new(LinkAwareSampler { inner: Sampler::AlwaysOff }))`; craft a sampled `SpanContext` (valid trace/span ids, `TRACE_FLAG_SAMPLED`); call `should_sample(None, trace_id, "n", &SpanKind::Internal, &[], &[Link::new(sampled_sc, vec![])])` → decision is `RecordAndSample`.
- `link_aware_root_follows_unsampled_link`: same with an unsampled SpanContext and inner `Sampler::AlwaysOn` → decision is `Drop`.
- `link_aware_root_without_links_delegates_inner`: no links, inner `AlwaysOff` → `Drop`; inner `AlwaysOn` → `RecordAndSample`.
- `provider_mints_linked_root_with_parent_flag`: end-to-end mechanism test — build a `SdkTracerProvider` with `Sampler::ParentBased(Box::new(LinkAwareSampler { inner: Sampler::AlwaysOff }))` + `InMemorySpanExporter`; hand-build two link targets: a sampled `SpanContext` (valid trace/span ids + `TRACE_FLAG_SAMPLED`) and an unsampled one (same ids, no sampled flag). For each: `tracer.build_with_context(tracer.span_builder("item").with_links(vec![Link::new(sc, vec![])]), &Context::new())`, end the span, flush → the exported item span `is_sampled()` equals the link target's flag. Run each case with its own provider (two runs) since the sampler is provider-level.
- `to_sdk_sampler_wraps_root_link_aware`: for each `OtelSampler` variant, `to_sdk_sampler` returns a `Sampler::ParentBased` whose delegate is the link-aware wrapper — assert via behavior (delegate returns Drop for an unsampled link even under `AlwaysOn`).

**Acceptance:**
- `cargo test -p camel-otel` exits 0 (existing sampler tests stay green — they must, since no existing span carries links).
- `cargo clippy -p camel-otel --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean.

- [x] 3.1

## camel-test

### Task 4.1: End-to-end trace-tree tests — split forest above threshold

**Files:**
- `crates/camel-test/tests/otel_trace_tree_test.rs` (modified)

**Steps:**
1. Add a new route builder `tree_split_forest_route()` mirroring
   `tree_split_route()` (line 266) but with the split body producing MORE
   fragments than the threshold and the split step configured with an
   explicit `trace_item_threshold` (construct via
   `camel_api::splitter::SplitterConfig::new(<split expression>).trace_item_threshold(n)`
   through the same route-definition path the existing split route uses).
2. Add tests following the file's harness contract (`test_spans()` /
   `finish` / `span` / `spans_named` helpers):

**Tests:**
- `split_above_threshold_starts_item_traces_with_links`: route with a 3-fragment split body and `trace_item_threshold(2)`; drive one exchange; assert: the route root + split segment spans share one trace id; exactly 3 `*:split-item` root spans exist; each item root's trace id differs from the route trace and from the other item roots; each item root carries exactly 1 link equal to the split segment span's span context; no item root has a parent span id; the per-fragment inner step spans (or sub-route roots, matching the existing tree_split_route body) nest under their item root, not under the split segment span.
- `split_below_threshold_stays_single_trace`: same route with 2 fragments (threshold 2) → reuse the assertions of `split_fragments_nest_under_segment_span_one_trace` shape: single trace id across all spans, no links anywhere.
- `split_trace_item_threshold_zero_keeps_nested`: route with 4 fragments and `trace_item_threshold(0)` → single trace, no `split-item` spans, no links.
- `split_default_threshold_from_yaml`: if the file's routes compile from YAML route definitions, add a YAML-driven variant asserting the default (100) applies when the key is absent — 2 fragments stay nested; if the harness only builds programmatic routes, assert instead that `SplitterConfig::new(..)` default is 100 (already covered in 1.1) and drive the YAML path with `trace_item_threshold: 1` + 2 fragments → forest (proves the knob threads from YAML to runtime).

**Acceptance:**
- `cargo test -p camel-test --test otel_trace_tree_test` exits 0
  (including the pre-existing
  `split_fragments_nest_under_segment_span_one_trace` — unchanged).
- `cargo clippy -p camel-test --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean.

- [x] 4.1
