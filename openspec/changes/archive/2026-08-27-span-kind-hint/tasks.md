# Tasks: span-kind-hint

## camel-api + camel-core tracer

### Task 1.1: SpanKindHint enum and TracingProcessor kind param

**Files:**
- `crates/camel-api/src/span_kind.rs` (new)
- `crates/camel-api/src/lib.rs` (modified — `pub mod span_kind;` +
  re-export)
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified —
  production caller)
- `crates/services/camel-otel/tests/integration.rs` (modified — 5 call
  sites)

**Steps:**
1. Create `crates/camel-api/src/span_kind.rs`:
   `#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
   pub enum SpanKindHint { #[default] Internal, Producer, Consumer, Client,
   Server }` with a doc comment stating the contract (compile-time hint for
   step span kinds; unknown future variants must degrade to Internal at the
   consumption site). Wire `pub mod span_kind;` + `pub use span_kind::SpanKindHint;`
   in camel-api lib.rs.
2. `TracingProcessor::new` (tracer.rs ~71) gains `kind_hint:
   SpanKindHint` param after `label` (label is currently the LAST param —
   append at the end). Convert once at construction to
   `opentelemetry::trace::SpanKind` stored as a precomputed field: a match
   with 5 explicit arms (Internal/Producer/Consumer/Client/Server) plus a
   REQUIRED `_ => SpanKind::Internal` wildcard arm (the enum is
   `#[non_exhaustive]`; the wildcard is the promised forward-compat
   behavior).
3. The step span builder (tracer.rs ~135 `.with_kind(SpanKind::Internal)`)
   uses the precomputed kind field instead of the hardcoded Internal.
4. Update every `TracingProcessor::new` caller to pass
   `SpanKindHint::Internal`: the single production wrap site in
   `compose_traced_pipeline` (route_compiler.rs, rg-verify with
   `rg -n 'TracingProcessor::new' crates/`), ALL in-file tracer.rs test
   sites, and the 5 sites in crates/services/camel-otel/tests/integration.rs.

**Tests:** (write EXACTLY these first, verify they FAIL, then implement)
- name: `tracing_processor_kind_client`
  setup: TracingProcessor::new(inner, "r".to_string(), 0,
  DetailLevel::Minimal, None, None, SpanKindHint::Client); span_test_util
  harness.
  action: call with an exchange; collect spans.
  assert: the span `r:step-0` has span_kind == Client.
  command: `cargo test -p camel-core --lib tracing_processor_kind_client`
  expected: fails before (param absent), passes after.
- name: `tracing_processor_kind_producer`
  setup: same with SpanKindHint::Producer.
  assert: span_kind == Producer.
  command: `cargo test -p camel-core --lib tracing_processor_kind_producer`
  expected: fails before, passes after.
- name: `tracing_processor_kind_default_internal`
  setup: same with SpanKindHint::default().
  assert: span_kind == Internal.
  command: `cargo test -p camel-core --lib tracing_processor_kind_default_internal`
  expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core --tests -- -D warnings` exits 0.
- `cargo clippy -p camel-otel --tests -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.

- [x] 1.1

## camel-core mapping + stamping

### Task 1.2: BuilderStep span_kind_hint mapping, Process field, registry stamping

**Files:**
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/{core,endpoints,routing,splitting,transforms}.rs` (modified — Process sites only; control_flow.rs has ZERO Process sites — exclude unless rg shows otherwise)
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified —
  rebuild passthrough)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs` (modified —
  resequencer origin)
- `crates/camel-core/src/lifecycle/adapters/{route_compiler_tests,route_compiler_span_tests,route_controller_tests}.rs` (modified — test sites)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-builder/src/lib.rs` (modified — 5 test sites)
- `crates/camel-test/src/harness.rs` (modified)
- `crates/components/camel-http/src/lib.rs` (modified — cfg(test) sites)
- `crates/camel-core/tests/{disposition_metrics,continued_e2e,arc_snapshot_concurrency,allocation_count}.rs`, `benches/{tracing_overhead,pipeline_throughput}.rs` (modified — Process sites)
- `crates/camel-bench/benches/*` (modified — Process sites)

**Steps:**
1. route_definition.rs: `impl BuilderStep { pub(crate) fn
   span_kind_hint(&self) -> SpanKindHint }` — exhaustive match. The ONLY
   non-Internal arm is `To(uri)`: scheme = authored URI before first ':';
   compare schemes with `eq_ignore_ascii_case` (there is NO scheme
   normalization on the To path — components register and resolve
   case-sensitively, so a mixed-case authored URI is legal and must still
   map correctly); kafka|jms|activemq|artemis|mqtt → Producer;
   http|https|grpc|grpcs|ws|redis|opensearch|sql|surrealdb|cxf|llm|mcp →
   Client; other/unknown/scheme-less → Internal. Every non-`To` variant →
   Internal (grouped arm; NO wildcard that swallows `To`).
2. step_compilers/mod.rs: `CompiledStep::Process` gains `kind_hint:
   SpanKindHint` (Process ONLY — Segment/Stop unchanged). Add
   `pub(crate) fn set_kind_hint(&mut self, hint: SpanKindHint)` — sets on
   Process, no-op on Stop/Segment (mirrors set_label).
3. `StepCompilerRegistry::compile_step` (~mod.rs:250): capture
   `let kind_hint = step.span_kind_hint();` beside the existing label
   capture (BEFORE the loop moves `step`); stamp on `Matched` beside the
   existing `set_label` call.
4. Update EVERY `CompiledStep::Process` construction site workspace-wide:
   origin sites (step_compilers/*, camel-dsl compile.rs, ext resequencer)
   insert `kind_hint: SpanKindHint::Internal`; rebuild sites in
   route_compiler.rs destructure and pass through; test sites insert
   `kind_hint: SpanKindHint::Internal`. Segment construction sites are
   UNTOUCHED (no field). Note: route_helpers.rs pattern sites use `..` and
   need no change but will surface in the verify rg — expected.
   VERIFY COMPLETENESS: `rg -n 'CompiledStep::Process\s*\{'
   crates/ --type rust` diffed against touched files.
5. NO consumption changes (compose still passes Internal — task 1.3 wires).

**Tests:** (write EXACTLY these first, verify FAIL, then implement)
- name: `builder_step_span_kind_hint_mapping`
  setup: the new method.
  action: assert To("kafka:orders")→Producer, To("jms:q")→Producer,
  To("activemq:q")→Producer, To("artemis:q")→Producer,
  To("mqtt:t")→Producer, To("http://x")→Client, To("https://x")→Client,
  To("grpc://x")→Client, To("grpcs://x")→Client, To("ws://x")→Client,
  To("redis://x")→Client, To("opensearch://x")→Client, To("sql:db")→Client,
  To("surrealdb://x")→Client, To("cxf://x")→Client, To("llm://x")→Client,
  To("mcp://x")→Client, To("KAFKA:orders")→Producer (case-insensitive),
  To("direct:y")→Internal, To("timer:z")→Internal,
  To("garbage")→Internal (scheme-less), Log variant→Internal,
  Filter variant→Internal, Split variant→Internal.
  assert: exact enum matches.
  command: `cargo test -p camel-core --lib builder_step_span_kind_hint_mapping`
  expected: fails before, passes after.
- name: `set_kind_hint_noop_on_stop_and_segment`
  setup: `let mut s = CompiledStep::Stop;` and a Segment construction.
  action: `s.set_kind_hint(SpanKindHint::Client);`
  assert: Stop unchanged; Segment field unchanged.
  command: `cargo test -p camel-core --lib set_kind_hint_noop_on_stop_and_segment`
  expected: fails before, passes after.
- name: `compile_step_stamps_kind_hint`
  setup: registry with the ToProcessCompiler double (mirrors the existing
  `compile_step_stamps_label` test in step_compilers/mod.rs).
  action: compile_step(BuilderStep::To("kafka:orders".into()), 0, &ctx, &registry).
  assert: returned Process has kind_hint == Producer AND label ==
  Some("to:kafka") (both stamps present).
  command: `cargo test -p camel-core --lib compile_step_stamps_kind_hint`
  expected: fails before, passes after.

**Acceptance:**
- `cargo check --workspace --all-targets` exits 0.
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core --tests -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.
- `rg -c 'kind_hint: SpanKindHint::Internal' crates/` reports hits across
  the file inventory; `rg -n 'kind_hint:' crates/camel-core/src/lifecycle/adapters/route_compiler.rs`
  shows passthrough at rebuild sites.

- [x] 1.2

## camel-core wiring + tests

### Task 1.3: Thread kind hints into step spans and assert kinds

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_span_tests.rs` (modified)
- `crates/camel-test/tests/otel_trace_tree_test.rs` (modified)

**Steps:**
1. `compose_traced_pipeline` wrap site: destructure `kind_hint` beside
   `label` from `CompiledStep::Process` and pass into
   `TracingProcessor::new(..., label.clone(), kind_hint)` replacing the
   task-1.1 `SpanKindHint::Internal`.
2. Pipeline-level kind assertions (see Tests): compile `To("http://...")`/
   `To("kafka:...")` through the REAL registry path using the stub pattern
   proven in step_compilers/endpoints.rs tests (~:239-276, ~:444-470 — the
   `ComponentContext` stub + `make_ctx` CompilationContext builder that
   compiles `To("stateful:dest")` in a `--lib` test). NOTE:
   route_compiler_span_tests.rs has NO compile-through-registry precedent
   (its tests hand-construct CompiledStep) — add a small helper there
   (stub ComponentContext + CompilationContext, register a compiler
   producing the stub component) rather than expecting pure reuse.
   Do NOT use the `register_component` integration-test pattern (full
   runtime, wrong level).
3. e2e regression guard: in `otel_trace_tree_test.rs`, add assertions that
   the route root span, all step spans (direct route), and the split
   segment span have `SpanKind::Internal` (the direct:-only routes map
   Internal — this locks the root/segment-stay-Internal scenario).
4. Update the route_compiler.rs doc comments that describe step span kinds
   if any now become stale (compose_traced_pipeline doc, tracer.rs
   TracingProcessor doc mention the kind param).

**Tests:** (write EXACTLY these first, verify FAIL, then implement)
- name: `compose_threads_kind_client_http` (route_compiler_span_tests.rs)
  setup: traced route compiled through the registry with a stub component
  under scheme `http` and a `To("http://stub/x")` step.
  action: run pipeline; collect spans.
  assert: the `{route_id}:to:http` span has span_kind == Client.
  command: `cargo test -p camel-core --lib compose_threads_kind_client_http`
  expected: fails before wiring (Internal), passes after.
- name: `compose_threads_kind_producer_kafka`
  setup: same with scheme `kafka`, `To("kafka:stub-topic")`.
  assert: the `{route_id}:to:kafka` span has span_kind == Producer.
  command: `cargo test -p camel-core --lib compose_threads_kind_producer_kafka`
  expected: fails before, passes after.
- name: `root_and_segment_stay_internal` (extend the e2e)
  setup: existing tree-main/tree-split scenarios.
  action: run both trees.
  assert: route root spans, step spans, and the split segment span all
  report SpanKind::Internal.
  command: `cargo test -p camel-test --test otel_trace_tree_test`
  expected: the kind assertions fail before wiring ONLY if kinds changed —
  they assert unchanged behavior; they must pass after wiring.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo test -p camel-test --test otel_trace_tree_test` exits 0.
- `cargo clippy -p camel-core -p camel-test --tests -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.
- Spec scenarios covered: http→Client (1.3 stub test), kafka→Producer
  (1.3 stub test), in-memory→Internal (1.2 mapping test + 1.1 default +
  e2e), root/segment stay Internal (e2e regression guard).

- [x] 1.3
