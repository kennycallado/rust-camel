# Design: span-name-enrichment

## Approach

The processor identity problem: `Processor` is a blanket Tower trait and
`BoxProcessor` is type-erased — runtime identity does not exist. But the DSL
does: the compiler registry sees the `BuilderStep` variant before dispatch,
and that variant fully determines a human-meaningful label. So labels are
stamped **at the registry dispatch point**, not per compiler arm.

1. `BuilderStep::span_label(&self) -> Option<String>` (route_definition.rs) —
   one central match over the ~49 variants. `To(uri)` is the only endpoint
   step variant (route entry `from:` lives on `RouteBuilder`, never a step)
   and maps to `to:{component}` using the scheme before the first `:` of the
   authored URI — the label reflects the DSL URI, not any `SkipTo`
   interception-rewritten target (endpoints.rs rewrites the send URI after
   compilation; `span_label` is pure over the authored step). EIP variants
   map to their kebab-case name (`Log` → `log`, `SplitterConfig` arm →
   `split`, …). `Processor(OpaqueProcessor)` and other anonymous variants →
   `None`. `set_label` no-ops on `Stop`.
2. `CompiledStep::{Process, Segment}` gain `label: Option<Arc<str>>`
   (step_compilers/mod.rs). `Arc<str>` is cheap to clone through the
   coercion/tracing wrap passes.
3. `StepCompilerRegistry::compile_step` (mod.rs:223) captures
   `let label = step.span_label();` before dispatch and stamps
   `s.set_label(label)` on `CompileOutcome::Matched`. Single injection point;
   compiler arms never know about labels.
4. Compiler arms construct `label: None` (registry overwrites); the
   rebuild sites in `route_compiler.rs` / `route_compiler_ext.rs` /
   `route_helpers.rs` (coercion, tracing wrap, segment re-wrapping) pass the
   destructured label through unchanged. Pattern sites already use `..`.
5. `TracingProcessor::new` gains a `label: Option<Arc<str>>` param; span name
   becomes `format!("{route_id}:{}", label.as_deref().unwrap_or(&step_id))`
   where `step_id = step_id_for(index)`. `compose_traced_pipeline` threads the
   step's label in.
6. Segment spans: `segment_span` in route_compiler.rs uses the segment step's
   label with the same fallback.
7. `step_span_attributes` (tracer.rs:280) drops the `step_id` KeyValue;
   `step_index` stays. The `camel_tracer` log field `step_id` (tracer.rs:146)
   is a separate surface and stays. Update the stale doc comments that list
   `step_id` as a span attribute: `crates/camel-core/src/lib.rs:37`,
   `crates/camel-core/src/shared/observability/domain/config.rs:84`.
8. Tests: tracer.rs unit tests assert the new name format (labeled +
   fallback); route_compiler_span_tests.rs updated where names are asserted;
   camel-test otel_trace_tree_test.rs — `tree-main` step 1 becomes
   `tree-main:to:direct` (its `.to("direct:tree-sub")`), steps 0/2 stay
   `tree-main:step-0/2` (closures); the split segment becomes
   `tree-split:split`.

## Affected crates

- camel-core: mechanism and consumption (DSL types, compiler registry,
  tracer, route compiler, tests) — the bulk of the change.
- camel-dsl: 1 production `CompiledStep` construction site (compile.rs:837,
  builds from raw `BoxProcessor`s → `label: None` correct).
- camel-otel: test-only caller updates (integration.rs
  `TracingProcessor::new` sites).
- camel-test (harness), camel-http (`cfg(test)` sites), camel-builder
  (test sites), camel-bench: mechanical `label: None` insertions. No
  production component behavior change.
- `CompiledStep` is exported (`camel_core::route::CompiledStep`) and
  constructed in 6 crates — that export is why the field addition churns
  workspace-wide despite the mechanism being camel-core-internal.

## Architecture boundaries

The label mechanism is DSL-internal: `BuilderStep`/registry are camel-core
lifecycle internals, and the tracer adapter consumes a label string, nothing
else. The only cross-crate surface touched is the already-exported
`CompiledStep` enum (field addition — a compile-breaking but mechanical
change for constructors; pattern sites use `..` and ride free). No camel-api
change, no component behavior change, no Services/Languages API change.
Labels are static DSL metadata captured at compile time, so hot-reload
semantics are unaffected (labels travel with the compiled step snapshot;
recompile re-stamps).

## Phases

Single-phase: the mechanism (field + stamping + name format) and its
consumption are one coherent slice — a label field without the name change
is dead code, and the name change without the field has no source. Task
granularity (1.1 tracer name format + attr drop, 1.2 label field + central
mapping + stamping + workspace sites, 1.3 wiring + test updates) provides
the review checkpoints within the phase.
