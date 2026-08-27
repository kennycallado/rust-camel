# Proposal: span-kind-hint

## Why

After `span-name-enrichment`, step spans carry meaningful names but every
span is still `SpanKind::Internal` (hardcoded at tracer.rs and segment_span).
A backend cannot distinguish "in-memory transform" from "outbound Kafka
produce" or "outbound HTTP call" — the single most useful signal a trace
gives an operator. This is the deferred P2 "SpanKindHint" item (bd rc-fwl7),
ruled on in the trace-model-tree ruling Q7: "worth it, and cheap — the
compiler knows the endpoint kind at compile time."

## What Changes

- New `#[non_exhaustive]` enum `SpanKindHint { Internal, Producer, Consumer,
  Client, Server }` in camel-api (`span_kind.rs`, new module), `Default =
  Internal`, per ADR-0049.
- `BuilderStep::span_kind_hint()` — central exhaustive mapping in
  camel-core (route_definition.rs, next to `span_label`): `To(uri)` derives
  the hint from the authored URI scheme — messaging brokers (kafka, jms,
  activemq, artemis, mqtt) → Producer; request/response and database
  endpoints (http, https, grpc, grpcs, ws, redis, opensearch, sql,
  surrealdb, cxf, llm, mcp) → Client; local-execution schemes (direct,
  seda, timer, cron, mock, log, exec, file, wasm, template, validator,
  xslt, xj, controlbus, master), other schemes, and scheme-less URIs →
  Internal. All non-endpoint EIP variants → Internal. NOTE: `sql` → Client
  (not the ruling Q7 parenthetical's Producer) is a conscious deviation —
  OTel database semconv makes the step span the DB client span, and Client
  is its mandated kind.
- `CompiledStep::Process` gains a `kind_hint: SpanKindHint` field (Process
  only — segments are structural and stay `Internal`); the compiler
  registry stamps it at the same dispatch point as the label.
- `TracingProcessor::new` gains a `kind_hint: SpanKindHint` param; the step
  span opens with the mapped OTel `SpanKind` instead of hardcoded
  `Internal`.
- Route root span and segment spans keep `Internal` (unchanged; root-kind
  from the `from:` endpoint is out of scope).

Excluded: root-span kind from route entry endpoints, per-item Consumer kinds
on splitter fragments, component-metadata-driven kind resolution (the
scheme match is the pragmatic first cut per the ruling).

## Acceptance criteria

- A traced route with `.to("http://...")` produces that step span with
  `SpanKind::Client`; `.to("kafka:...")` → `SpanKind::Producer`.
- EIP steps and `to:direct` steps keep `SpanKind::Internal`.
- Segment spans and route root spans remain `Internal` (no regression).
- All quality gates green; workspace construction sites updated.

## Risk budget

Acceptable: one more mechanical field insertion at `CompiledStep::Process`
construction sites (~90 sites, same churn pattern as span-name-enrichment);
a new public camel-api enum (additive, `#[non_exhaustive]` guards
compatibility). Out of bounds: changing root/segment span kinds, sampler or
tree shape (all blessed in trace-model-tree), any camel-api breaking change.
