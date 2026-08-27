# Design: span-kind-hint

## Approach

Mirror the span-name-enrichment mechanism (blessed pattern): central
DSL-side mapping, registry stamping, tracer-side consumption.

1. `crates/camel-api/src/span_kind.rs` (new module, re-exported from lib.rs):
   `#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
   pub enum SpanKindHint { #[default] Internal, Producer, Consumer, Client,
   Server }`. ADR-0049 compliant (non_exhaustive contract enum).
2. `BuilderStep::span_kind_hint(&self) -> SpanKindHint` in
   route_definition.rs, next to `span_label`: one exhaustive match; the only
   non-Internal arm is `To(uri)` — scheme before first `:` compared with
   `eq_ignore_ascii_case` (the To path performs NO scheme normalization —
   components register and resolve case-sensitively, so mixed-case
   authored URIs are legal and must map correctly):
   kafka|jms|activemq|artemis|mqtt → Producer (messaging brokers);
   http|https|grpc|grpcs|ws|redis|opensearch|sql|surrealdb|cxf|llm|mcp →
   Client (request/response + database; `sql` → Client is a conscious
   deviation from ruling Q7's parenthetical — OTel database semconv mandates
   Client for the DB-call span, and the step span IS the DB client span);
   every remaining in-repo scheme (direct, seda, timer, cron, mock, log,
   exec, file, wasm, template, validator, xslt, xj, controlbus, master) is
   local execution → Internal; unknown schemes and scheme-less URIs →
   Internal. Every non-`To` variant (including `WireTap { uri }`, which
   resolves a producer but keeps its authored EIP identity) → Internal —
   the kind mapping is scoped to `.to` steps exactly as the ruling
   scoped it; single catch-all arm placed AFTER the `To` match (it cannot
   swallow `To`); unlike `span_label`, this mapping deliberately uses a
   wildcard — `Internal` is the enum's `#[default]` contract and the
   correct uniform default for any future non-endpoint variant, so new
   variants degrade silently to today's behavior (a missed-improvement
   ceiling, not a regression; `span_label`'s exhaustive match still forces
   the author to decide every new variant's label one screen above).
3. `CompiledStep::Process` gains `kind_hint: SpanKindHint` (Process ONLY —
   segments are structural, `segment_span` keeps `SpanKind::Internal`).
   `set_kind_hint(&mut self, hint)` mutates Process, no-op on Stop/Segment.
   `StepCompilerRegistry::compile_step` captures `let kind_hint =
   step.span_kind_hint();` beside the label capture and stamps on `Matched`.
4. Site churn: every `CompiledStep::Process` construction site gains
   `kind_hint: SpanKindHint::Internal` (origin + test sites) or passthrough
   (rebuild sites destructure and re-emit). Segment sites untouched.
5. `TracingProcessor::new` gains `kind_hint: SpanKindHint` (after `label`);
   converts once to `opentelemetry::trace::SpanKind` stored as a
   precomputed field. The conversion match carries 5 explicit arms plus a
   `_ => SpanKind::Internal` wildcard arm — REQUIRED because the enum is
   `#[non_exhaustive]` in camel-api (ADR-0049: out-of-crate matches need
   the wildcard), and the wildcard IS the forward-compat behavior the enum
   promises (unknown future hints trace as Internal).
   `span_builder(...).with_kind(...)` uses it (tracer.rs ~135 today
   hardcodes Internal).
6. `compose_traced_pipeline` destructures `kind_hint` beside `label` and
   threads it into `TracingProcessor::new`. After the span-name-enrichment
   delegation there is a single wrap site.
7. Root span (route_compiler.rs ~390) and segment_span (~650) keep
   `SpanKind::Internal` — out of scope by design.

## Affected crates

- camel-api: new `span_kind.rs` module (additive, public enum + re-export).
- camel-core: route_definition mapping, registry stamping, CompiledStep
  field, tracer conversion, compose wiring, tests.
- camel-dsl, camel-builder, camel-test (harness), camel-http (cfg(test)),
  camel-core tests/benches, camel-bench: mechanical
  `kind_hint: SpanKindHint::Internal` insertions at Process construction
  sites only (the span-name-enrichment inventory is the superset — Segment
  sites are untouched this time).

## Architecture boundaries

Same shape as span-name-enrichment: DSL-internal mechanism, single
cross-crate surface is the already-exported `CompiledStep` (field addition
on Process) plus one new additive camel-api type. No component behavior
change, no runtime hexagonal boundary change, hot-reload unaffected
(hint travels with the compiled snapshot; recompile re-stamps). The
scheme→kind table is a closed first cut per the ruling; component-metadata
enrichment stays a future option.

## Phases

Single-phase: enum + mapping + stamping + wiring are one coherent slice
(a hint field without consumption is dead code; consumption without the
field has no source). Tasks: 1.1 camel-api enum + tracer param, 1.2
mapping + stamping + workspace sites, 1.3 compose wiring + tests.

Test strategy for kind scenarios: the tree e2e is `direct:`-only (all
Internal — blind to the mapping). Pipeline-level kind assertions (http →
Client, kafka → Producer) compile `To("http://...")`/`To("kafka:...")`
through the REAL compiler registry using the stub pattern proven in
step_compilers/endpoints.rs tests (~:239-276, ~:444-470: ComponentContext
stub + `make_ctx` CompilationContext builder) — no broker, network, or
full runtime dependency. route_compiler_span_tests.rs gets its first
small compile-through-registry helper. Mapping exhaustiveness gets unit
tests beside the `span_label` tests in route_definition.rs.
