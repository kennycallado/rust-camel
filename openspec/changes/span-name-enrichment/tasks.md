# Tasks: span-name-enrichment

## camel-core tracer

### Task 1.1: TracingProcessor label param, span name format, step_id attr drop

**Files:**
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/lib.rs` (modified, doc comment only)
- `crates/camel-core/src/shared/observability/domain/config.rs` (modified, doc comment only)
- `crates/services/camel-otel/tests/integration.rs` (modified — 5
  `TracingProcessor::new` call sites at ~217, ~432, ~441, ~503, ~526 use the
  public re-export; they pass `None`)

**Steps:**
1. Change `TracingProcessor::new` signature (tracer.rs:63) to take
   `label: Option<Arc<str>>` as a new parameter after `metrics`. Store it in
   a `label` field on `TracingProcessor`.
2. In `TracingProcessor::new` (tracer.rs:68-80), keep `step_id =
   step_id_for(step_index)`; change span name construction to
   `format!("{route_id}:{}", label.as_deref().unwrap_or(&step_id))`.
3. Update every `TracingProcessor::new` caller to pass `None` for now: the
   wrap site in `compose_traced_pipeline` in route_compiler.rs (after the
   followups-trivial delegation there is a single production wrap site —
   verify with `rg -n 'TracingProcessor::new' crates/`), plus ALL in-file
   test call sites inside tracer.rs (~365 through ~767 — rg-verify rather
   than trusting anchors) and the 5 sites in
   `crates/services/camel-otel/tests/integration.rs`.
4. In `step_span_attributes` (tracer.rs:280), delete the `step_id` KeyValue
   entry; keep `step_index` and all other attributes unchanged. The
   `camel_tracer` log field `step_id` (tracer.rs:146) stays.
5. Update the doc comments listing step-span attributes:
   `crates/camel-core/src/lib.rs:37` and
   `crates/camel-core/src/shared/observability/domain/config.rs:84` — remove
   `step_id` from the span-attribute list (4 attrs: messaging.system,
   route_id, correlation_id, step_index) and keep any log-field mentions.

**Tests:**
- name: `tracing_processor_labeled_span_name`
  setup: a boxed no-op inner processor; `TracingProcessor::new(inner,
  "r".to_string(), 0, DetailLevel::Minimal, None, Some("log".into()))`
  action: call the processor with an exchange; collect spans via the
  `span_test_util` harness.
  assert: exactly one span named `r:log`.
  command: `cargo test -p camel-core --lib tracing_processor_labeled_span_name`
  expected: fails before the change (param does not exist), passes after.
- name: `tracing_processor_fallback_span_name`
  setup: same but `label: None`.
  action: call; collect spans.
  assert: span named `r:step-0` (fallback preserved).
  command: `cargo test -p camel-core --lib tracing_processor_fallback_span_name`
  expected: fails before, passes after.
- name: update the existing step-attribute test(s) in tracer.rs that assert
  the attribute set: remove `step_id` from expectations, assert `step_index`
  present and `step_id` absent.
  command: `cargo test -p camel-core --lib`
  expected: all tracer.rs tests pass after updates.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core --tests -- -D warnings` exits 0.
- `cargo clippy -p camel-otel --tests -- -D warnings` exits 0 (integration
  test target compiles with the new param).
- `rg -n '"step_id"' crates/camel-core/src/shared/observability/adapters/tracer.rs`
  shows zero hits inside `step_span_attributes` (the log-field write at ~146
  may remain).
- `cargo fmt --all -- --check` exits 0.

- [x] 1.1

### Task 1.2: BuilderStep span_label mapping, CompiledStep label field, registry stamping

**Files:**
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/endpoints.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/routing.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/transforms.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/control_flow.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_helpers.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified — 1 site ~837)
- `crates/camel-builder/src/lib.rs` (modified — 5 sites ~2066, ~2075, ~2489,
  ~2522, ~2559)
- `crates/camel-test/src/harness.rs` (modified — ~3 sites ~435, ~570, ~575)
- `crates/components/camel-http/src/lib.rs` (modified — ~2 sites ~7100, ~7108,
  cfg(test) region)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_tests.rs` and
  `route_compiler_span_tests.rs` (modified — test constructions
  `label: None`)
- `crates/camel-core/tests/disposition_metrics.rs`, `tests/continued_e2e.rs`,
  `tests/arc_snapshot_concurrency.rs`, `tests/allocation_count.rs`,
  `benches/tracing_overhead.rs`, `benches/pipeline_throughput.rs`
  (modified — `label: None` insertions)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs`
  (modified — ~3 sites ~1544, ~1689, ~1696)
- `crates/camel-bench/benches/*` (modified — any `CompiledStep::Process`/
  `Segment` constructions, `label: None`)

NOTE: `CompiledStep` is exported and constructed across the workspace. The
inventories above were verified by `rg 'CompiledStep::(Process|Segment) {'
crates/ --type rust | grep -v step_compilers` — after editing, re-run it and
diff against the touched files; ANY remaining untouched constructor is a
missed site.

**Steps:**
1. In `route_definition.rs`, add `impl BuilderStep { pub(crate) fn
   span_label(&self) -> Option<String> }`: one match over ALL variants
   (the enum has 49 — cover every arm, grouped where labels repeat).
   `To(uri)` → `Some(format!("to:{}", component))` where `component` is the
   URI scheme before the first `:` of the authored URI (string split; do NOT
   resolve `SkipTo` interception targets). EIP variants → kebab-case name of
   the EIP (`Log` → `log`, `Split { config: SplitterConfig, .. }` → `split`,
   `DeclarativeSplit` → `split`, filter → `filter`, …; use the existing
   kebab-case convention of the DSL). `DeclarativeStreamSplit` → `split`.
   Anonymous variants
   (`Processor(OpaqueProcessor(..))`, identity/no-op helpers) → `None`.
2. In `step_compilers/mod.rs`, add `label: Option<Arc<str>>` to BOTH
   `CompiledStep::Process` and `CompiledStep::Segment` variant payloads (keep
   field order: after `processor`/`segment`, before existing fields or
   trailing — consistent in both). Add `impl CompiledStep` with
   `pub(crate) fn set_label(&mut self, label: Option<String>)` that overwrites
   `Process`/`Segment` labels and no-ops on `Stop`.
3. In `StepCompilerRegistry::compile_step` (mod.rs:223), capture
   `let label = step.span_label();` BEFORE the dispatch loop moves `step`;
   on `CompileOutcome::Matched(mut s)` call `s.set_label(label)` before
   returning.
4. Update EVERY `CompiledStep::Process` / `CompiledStep::Segment`
   construction site to compile: origin sites in step_compilers/*,
   camel-dsl/compile.rs, and the synthetic resequencer append in
   route_compiler_ext.rs (~626 — ORIGIN, `label: None`; it creates a new
   step, not a rebuild) insert `label: None`; the 6 rebuild constructions
   across the 3 compose functions in route_compiler.rs (~157/~172,
   ~201/~207, ~243/~249), route_compiler_ext.rs rebuild sites, and
   route_helpers.rs destructure the incoming label and
   pass it through unchanged. Test-module constructions (cfg(test) in these
   files and route_compiler_span_tests.rs / route_compiler_tests.rs) insert
   `label: None`.
5. Do NOT change any span-name consumption in this task — labels are stamped
   but not yet read by the tracer (Task 1.3 wires that).

**Tests:**
- name: `builder_step_span_label_mapping`
  setup: the new `span_label` method.
  action: assert representative arms: `To("direct:tree-sub".into())` →
  `Some("to:direct")`, `To("http://api.example/x".into())` →
  `Some("to:http")`, the `Log` variant → `Some("log")`, the splitter variant
  → `Some("split")`, `Processor(OpaqueProcessor(..))` → `None`.
  assert: exact matches; plus the match is exhaustive (compiler enforces via
  the exhaustive `span_label` match — no `_` catch-all returning a label).
  command: `cargo test -p camel-core --lib builder_step_span_label_mapping`
  expected: fails before (method absent), passes after.
- name: `set_label_noop_on_stop`
  setup: `let mut s = CompiledStep::Stop;`
  action: `s.set_label(Some("x".into()))`.
  assert: still matches `CompiledStep::Stop` (no panic, no change).
  command: `cargo test -p camel-core --lib set_label_noop_on_stop`
  expected: fails before, passes after.
- name: `compile_step_stamps_label`
  setup: a `StepCompilerRegistry` with the endpoint compiler registered.
  action: `compile_step(BuilderStep::To("direct:x".into()), 0, &ctx, &registry)`.
  assert: returned `Some(CompiledStep::Process { label: Some(a) })` where
  `a == "to:direct"`.
  command: `cargo test -p camel-core --lib compile_step_stamps_label`
  expected: fails before, passes after.

**Acceptance:**
- `cargo check --workspace --all-targets` exits 0 (proves no construction
  site was missed anywhere in the workspace).
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core --tests -- -D warnings` exits 0.
- `rg -c 'label: None' crates/camel-core/src/lifecycle/adapters/step_compilers/`
  reports hits in core, endpoints, routing, splitting, transforms,
  control_flow, mod.
- `rg -n 'label:' crates/camel-core/src/lifecycle/adapters/route_compiler.rs`
  shows passthrough sites (not `None`) in the coercion/wrap rebuilds.

- [x] 1.2

## camel-core wiring + end-to-end

### Task 1.3: Thread labels into span names (compose + segment) and update span tests

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_span_tests.rs` (modified)
- `crates/camel-test/tests/otel_trace_tree_test.rs` (modified)

**Steps:**
1. In `compose_traced_pipeline` (route_compiler.rs:139), destructure each
   `CompiledStep::Process { processor, label, .. }` and pass
   `label.clone()` into `TracingProcessor::new` (replacing the `None`
   from Task 1.1). Same for the coercion path in
   `compose_traced_pipeline_with_contracts` if it wraps separately (after
   the Task 1.1-era delegation it funnels through
   `compose_traced_pipeline` — verify and wire once).
2. Give `TracedSegmentStep` (route_compiler.rs:653) a new `label:
   Option<Arc<str>>` field; add a `label: Option<Arc<str>>` param to
   `segment_span` (route_compiler.rs:623); update the `TracedSegmentStep`
   construction (~:463) to pass the segment step's label through. The
   segment span name becomes `format!("{route_id}:{}", label.as_deref().
   unwrap_or(&step_id_for(index)))` with the same fallback as steps.
3. Update name assertions in `route_compiler_span_tests.rs`: any test
   asserting `"{route}:step-N"` for a labeled step now asserts the label
   form; unlabeled (opaque-processor) assertions stay `step-N`. AUDIT every
   span-selection filter, not just equality asserts — e.g. the
   `starts_with("rt:step-")` prefix selector at ~:81 silently misses labeled
   spans (`rt:to:direct`); selectors must cover both forms or filter by
   parent/attrs instead.
4. Update `crates/camel-test/tests/otel_trace_tree_test.rs`: `tree-main`
   step 1 (`.to("direct:tree-sub")`) becomes `tree-main:to:direct`;
   steps 0 and 2 (closures) stay `tree-main:step-0/2`; the `tree-split`
   segment span becomes `tree-split:split`; `tree-sub` steps stay
   `tree-sub:step-0/1`. Update the module doc comment (lines 8-19)
   accordingly. Also update the `{route_id}:step-{index}` doc comments
   inside route_compiler.rs (segment_span ~620, TracedSegmentStep ~645,
   compose_traced_pipeline ~132) to document the label form and fallback —
   same doc hygiene as lib.rs:37/config.rs:84 in Task 1.1.

**Tests:**
- name: `compose_threads_label_to_span_name` (in
  route_compiler_span_tests.rs)
  setup: a traced route compiled from `BuilderStep::To("direct:y".into())`.
  action: run the pipeline; collect spans via the harness.
  assert: a span named `{route_id}:to:direct` exists and is a child of the
  route root.
  command: `cargo test -p camel-core --lib compose_threads_label_to_span_name`
  expected: fails before Task 1.3 wiring, passes after.
- name: `segment_span_uses_label`
  setup: a traced route with a splitter segment step.
  action: run; collect spans.
  assert: segment span named `{route_id}:split`, child of the route root.
  command: `cargo test -p camel-core --lib segment_span_uses_label`
  expected: fails before, passes after.
- name: `otel_trace_tree_test` (existing e2e, updated)
  setup: unchanged routes (tree-main, tree-sub, tree-split).
  action: run both tree scenarios.
  assert: updated names (`tree-main:to:direct`, `tree-split:split`,
  fallbacks elsewhere) AND all existing shape invariants (parenting,
  containment, single trace) still hold.
  command: `cargo test -p camel-test --test otel_trace_tree_test`
  expected: fails with old names before wiring, passes after.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo test -p camel-test --test otel_trace_tree_test` exits 0.
- `cargo clippy -p camel-core -p camel-test --tests -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.
- Spec scenarios covered: endpoint label (1.3 e2e + compose test), EIP label
  (segment_span_uses_label + e2e), fallback (1.1 fallback test + e2e steps
  0/2), attr-set (1.1 attribute test).

- [x] 1.3
