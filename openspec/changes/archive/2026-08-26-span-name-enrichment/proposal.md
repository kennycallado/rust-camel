# Proposal: span-name-enrichment

## Why

After `trace-model-tree` fixed the trace shape, step spans are still named
`{route_id}:step-{index}` — positionally correct but semantically opaque. An
operator reading a trace with 15 sibling spans under a route root cannot tell
which span is the HTTP call, which is the splitter, which is a log. The
`step_id` attribute (`"step-3"`) duplicates information already in the span
name. This is the deferred P2 "name enrichment" item (bd rc-k6dx) from the
trace-model-tree ruling.

## What Changes

- Step and segment spans SHALL be named `{route_id}:{label}` where the label
  identifies the DSL step: EIP name (`log`, `filter`, `split`, …) or endpoint
  component (`to:http`, `to:direct`, …).
- When no label is derivable (opaque closures via `.process(...)`), the name
  falls back to the current `{route_id}:step-{index}`.
- The `step_id` span attribute is removed (redundant with name + step_index);
  `step_index` remains the positional attribute. The `camel_tracer` LOG field
  `step_id` is a separate surface and stays.
- Labels derive from the DSL step kind and endpoint component scheme only —
  never full URIs, headers, or exchange data (cardinality and PII guard).
  For endpoint steps the label reflects the authored DSL URI scheme, not any
  interception-rewritten target (`SkipTo`).
- Mechanism: `BuilderStep::span_label()` central mapping (~49 variants,
  camel-core DSL-internal); `CompiledStep::{Process,Segment}` gain an
  `Option<Arc<str>>` label field stamped by the compiler registry at the
  single dispatch point; `TracingProcessor`/segment span builders consume it.
- Compiler arms construct with `label: None` (registry overwrites); the
  wrap/rebuild sites in `route_compiler.rs` preserve labels by passthrough.

Excluded (stay deferred): `SpanKindHint` (rc-fwl7), splitter new-trace
threshold + links (rc-29gd), root-span entry-endpoint enrichment.

## Acceptance criteria

- A route using `.to("http://...")` produces step spans named
  `{route_id}:to:http`; a `.split(...)` step produces `{route_id}:split`.
- A route using only `.process(closure)` keeps `{route_id}:step-{index}` names.
- Step/segment spans carry `step_index` but no `step_id` attribute.
- Existing tree-shape invariants (parenting, containment, single trace) are
  unchanged — only names and the attribute set change.
- All quality gates green; span tests updated to the new naming.

## Risk budget

Acceptable: mechanical churn at ~96 `CompiledStep` construction sites
(`label: None` insertions) and span-test name updates. Cardinality shift in
span names is bounded by the closed set of DSL kinds + component schemes.
Out of bounds: any change to trace shape/parenting, sampler behavior, or
public `camel-api` surfaces; any label derived from exchange payload data.
