# Proposal: trace-model-tree

## Why

Two captured OTLP traces from the demo environment show the trace model is broken:

- A 337-span trace where **336/336 child spans start after their parent ended**. Steps chain (each step's parent is the previous step's span), so no span temporally contains its children. Backends render a depth-7 comb instead of a tree.
- A splitter fan-out put **49 correlation IDs into one traceId**, all fragments chained off a 0.76 ms root.
- `duration_ms` is recorded as truncated `as_millis()` on 17% of spans it contradicts (sub-ms spans report 0).
- Errors emit a non-standard `"error"` event (`error.type`/`error.message`) instead of the `exception` semantic convention.
- `to_sdk_sampler` maps to raw `TraceIdRatioBased`, which re-samples children independently and drops parts of traces even after the tree fix.

Root cause (per ruling `docs/rulings/trace-model-tree-ruling-2026-08-26.md`): there is no enclosing route root span, `TracingProcessor` overwrites `exchange.otel_context` with the step span for the next step, and the sampler lacks `ParentBased`.

## What Changes

1. **Route root span (P0-a).** `TracedPipeline` opens a `{route_id}` span at pipeline entry. Fresh entry starts a new trace; an exchange arriving with a live context (e.g. `direct:` sub-route) nests the root under it. Root ends with the pipeline result status.
2. **Nested steps (P0-a).** The step span stays active only while its inner processor runs (so split fragments and `direct:` hops nest under the step). After the step completes, `TracingProcessor` restores the route root context instead of chaining the next step under it.
3. **Splitter fan-out (P0-b).** Structural segments (split, streaming split, multicast) compile as `CompiledStep::Segment` and today emit no span. `run_steps` (which already carries the trace flag) opens a step span for each segment too, so fragment exchanges inheriting the live context become children of the splitter step span in one trace. No new-trace-per-item (deferred, bd rc-29gd).
4. **No `duration_ms` span attribute (P1-a).** Timestamps already encode duration; the tracing-log field and route metrics stay.
5. **`exception` event (P1-b).** Error branch records event name `"exception"` with `exception.type`/`exception.message` plus `set_status(error)`.
6. **Parent-based sampling (P0 co-requisite).** `to_sdk_sampler` wraps the configured root sampler in `Sampler::ParentBased` so children respect the parent sampling decision.

## Acceptance Criteria

- All step spans of a route execution share the route root span as parent and fall inside its time window.
- A `direct:` sub-route root is a child of the caller's step span.
- Split fragments and their route spans stay in one trace, nested under the splitter step span.
- No span carries a `duration_ms` attribute; the `camel_tracer` log field remains.
- Failed steps emit one `exception` event with `exception.type` and `exception.message`.
- A child span of an unsampled parent is not recorded (ParentBased); sampled parents keep children.

## Risk Budget

Low. Pre-1.0, no external trace contract. In-repo breakage limited to tests asserting trace shape and demo docs. Semantics of `exchange.otel_context` change for downstream processors — covered by tests in tracer, route_compiler, splitter, and camel-test (otel direct-hop regression).

## Out of Scope (bd follow-ups)

rc-29gd split threshold + Link, rc-fwl7 SpanKindHint, rc-hxx1 scope version, rc-k6dx name enrichment.
