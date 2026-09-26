# Proposal: splittrace

## Why

The trace-model-tree work (rc-ikdx, landed 4c69619a) made splitter children
inherit the live route context. That is correct for small fan-outs, but a
large split (thousands of items) produces one giant trace with thousands of
sibling child spans — it overwhelms trace UIs and blows per-trace span
budgets in OTEL collectors (bd rc-29gd motivation).

## What Changes

Add a splitter tracing threshold policy: when the number of split fragments
exceeds a configurable item threshold, each fragment starts a NEW trace
(per-item root span) instead of nesting under the live route span. Each new
root carries an OTEL span Link back to the originating split segment span,
and the parent's sampling decision is carried forward so Link'd traces
sample consistently with their origin (sampled origin → sampled item roots;
dropped origin → dropped item roots).

Affected crates:

- `camel-api` — `SplitterConfig` gains `trace_item_threshold: usize`
  (default 100, 0 disables; builder method mirrors `max_fragments`).
- `camel-processor` — `SplitSegment` stamps `CAMEL_SPLIT_INDEX` /
  `CAMEL_SPLIT_SIZE` / `CAMEL_SPLIT_COMPLETE` on fragments (parity with
  `SplitterService`; no OTEL dependency added).
- `camel-core` — split step compilers wrap the per-fragment body with a
  trace-restart segment: above threshold it mints a per-item root span with
  one Link to the split segment span, runs the inner body under it, and
  ends it with the outcome.
- `camel-otel` — root sampler becomes link-aware: a parentless span whose
  builder carries links inherits the first link's sampled flag (OTEL
  sampler-links guidance); existing spans (no links) are unaffected.
- `camel-dsl` — YAML `split:` step gains `trace_item_threshold`
  (`Option<usize>`; absent → default 100, 0 → off); route schema regenerated.

Included: threshold boundary semantics (`count > threshold` restarts),
nested-mode regression safety, unit + shape tests per house convention
(real-collector integration stays CI's domain).

Excluded: streaming splits (lazy, total unknown upfront — stays nested,
tracked as deferral); `SplitterService` (Tower-layer eager splitter, not the
compiled-route path); DSL flavors beyond YAML route definitions.

## Acceptance criteria

- Config surface exists: `SplitterConfig.trace_item_threshold` + YAML knob.
- Above threshold, each fragment's body runs under a new trace root span
  carrying exactly one Link to the split segment span; no parent span id.
- Sampling flag propagates: item roots' `is_sampled` equals the origin's.
- At or below threshold (and when threshold is 0/omitted-default below 100),
  nested single-trace behavior is byte-identical to today (existing
  `split_fragments_nest_under_segment_span_one_trace` stays green).
- Default threshold 100 is chosen and justified in the spec scenario.

## Risk budget

Acceptable: additive fragment metadata properties (parity gap with
`SplitterService`); sampler behavior change limited to parentless spans
WITH links (no existing span carries links). Out of bounds: any change to
nested-mode span shape below threshold; new hard dependencies in
camel-processor; behavior change for routes that never exceed threshold.

Bd: rc-29gd (feature, P3).
