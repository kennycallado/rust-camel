# Design: splittrace

## Approach

The compiled-route split path funnels through `camel_processor::SplitSegment`
(both `BuilderStep::Split` and `BuilderStep::DeclarativeSplit` compile to
it, in `step_compilers/splitting.rs`). Four cooperating pieces:

1. **Config** (`camel-api/src/splitter.rs`): `SplitterConfig` gains
   `trace_item_threshold: usize`. `SplitterConfig::new` defaults it to 100;
   builder method `.trace_item_threshold(n)` mirrors `max_fragments`.
   Semantics: `0` = off (legacy always-nested); `n >= 1` = restart traces
   when fragment count `> n` (boundary: count == n stays nested,
   count == n+1 restarts).

2. **Fragment metadata** (`camel-processor/src/split_segment.rs`):
   `SplitSegment::run` stamps `CAMEL_SPLIT_INDEX` / `CAMEL_SPLIT_SIZE` /
   `CAMEL_SPLIT_COMPLETE` on the fragment vec before processing — the same
   metadata `SplitterService` already stamps (splitter.rs). This is the
   parity gap fix that exposes the total to the per-fragment body without
   any OTEL dependency in camel-processor.

3. **Trace-restart body wrapper** (`camel-core`, route_compiler zone):
   when the split step compiler sees `trace_item_threshold >= 1`, it wraps
   the composed body segment in a `TraceRestartBody` (camel-core, alongside
   `segment_span`/`finish_span_outcome`). Per fragment: read
   `CAMEL_SPLIT_SIZE`; if `total > threshold`, mint a per-item root span —
   name `{route_id}:split-item`, `SpanKind::Internal`, one attribute pair
   (`split.item.index`, `split.item.total`) capped by the Minimal-level
   discipline — started on an EMPTY `OtelContext` (new trace id, no parent
   span id) with `SpanBuilder::with_links(vec![Link::new(origin_sc, vec![])])`
   where `origin_sc` is the live context's span context (the
   `{route_id}:split` segment span). Swap `frag.otel_context`, run the
   inner body, then finish the item span with the outcome
   (`finish_span_outcome`) and end it — span lifetime is owned by the
   wrapper, so no leak in stop/failure paths. Parallel mode is safe: each
   fragment future owns its exchange and context; no ambient context is
   mutated.

4. **Link-aware root sampler** (`camel-otel/src/service.rs`): a sampler
   wrapper `LinkAwareSampler(inner)` — in `should_sample`, when the
   parent context is absent and `links` is non-empty, return
   `Sampled`/`Drop` per the first link's `is_sampled()`; otherwise delegate
   to `inner`. `to_sdk_sampler` becomes
   `Sampler::ParentBased(Box::new(LinkAwareSampler(inner)))`. Rationale:
   the OTEL SDK minting a parentless span consults only the root sampler —
   plain `ParentBased` ignores links, so a dropped origin would still mint
   sampled item roots. No existing span carries links, so observable
   sampler behavior is unchanged for everything else.

Sampling-flag note: the flag correctness is a property of the configured
provider; with no provider (noop global tracer) item roots are no-ops with
zero overhead, identical to step spans today.

## Affected crates

- `camel-api`: `SplitterConfig` field + default + builder method.
- `camel-processor`: `SplitSegment` metadata stamping (+ unit tests).
- `camel-core`: `TraceRestartBody` wrapper + compiler wiring (+ span-shape
  tests using the existing `span_test_util` harness contract).
- `camel-otel`: `LinkAwareSampler` + `to_sdk_sampler` wiring (+ sampler
  unit tests).
- `camel-dsl`: `SplitData.trace_item_threshold: Option<usize>` (schemars),
  YAML→compile mapping, `schemas/dsl/route-schema.json` regeneration.
- `camel-test`: end-to-end trace-tree tests in `otel_trace_tree_test.rs`
  (forest + link shape; small-split single-trace already covered).

## Architecture boundaries

DSL stays declarative (knob only, no tracing logic). camel-processor stays
OTEL-free (metadata only). Tracing logic stays in camel-core's route
compiler adapters where `segment_span`/`finish_span_outcome` already live.
Sampler policy stays in the camel-otel service. hexagonal boundaries test
untouched (no query-plane tokens). `trace-model-tree` ruling semantics
(rc-ikdx P0-b) are preserved below threshold.

## Phases

Single coherent slice; no phase grouping.

## Deferrals

- Streaming splits (`DeclarativeStreamSplit` / `StreamingSplitter`): lazy,
  total unknown upfront — stay nested. Follow-up bd candidate at park.
- `SplitterService` (Tower eager splitter, non-route path): no threshold
  wiring; routes are the only traced entry today.
