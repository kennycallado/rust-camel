# Proposal: builder-layer-middleware

## Why

`e_opus` (2026-10-27 tower review) identified custom Tower middleware as the only real
user-facing gap left in the pipeline. Today a custom processor can be wrapped
(`ServiceBuilder` + `BoxProcessor` + `process_fn`), but the pattern is undocumented and
there is no builder hook to wrap a `.to(uri)` producer, a route section, or a whole route
with a `tower::Layer`. Users who need a timeout, a concurrency cap, or a rate limit around
an outbound call must hand-compose services outside the DSL. (bd rc-s5ezy)

## What Changes

- **Builder hook** (`camel-builder`): three additive methods —
  `to_layered(uri, layer)` (producer granularity), `layered(layer, |route| …)`
  (section granularity), `route_layer(layer)` (whole-route granularity) — all accepting
  any `tower::Layer` through a new type-erased `camel_api::ProcessorLayer` /
  `LayerFactory` pair.
- **Compiler support** (`camel-core`): two new `BuilderStep` variants
  (`LayeredTo`, `Layered`) with compile arms in the step-compiler registry. The producer
  arm wraps the resolved producer; the section arm wraps the composed child segment.
  `run_steps`, `OutcomePipeline`, `CompiledStep`, and the route lifecycle are NOT
  modified.
- **`From<BoxError> for CamelError`** (`camel-api`): tower middleware errors (`Timeout`,
  `Buffer`) map to a typed `CamelError` with the original error preserved as `source`,
  so timeouts are discriminable instead of arriving as an opaque box.
- **Segment adapter** (`camel-api`): run an `OutcomeSegment` as a Tower service with
  `PipelineOutcome::Stopped` re-marked on the exchange, so Stop semantics survive the
  Tower boundary.
- **Probe evidence** (`camel-builder` tests): `Timeout`, `ConcurrencyLimit`, and
  `RateLimit + Buffer` around mock producers; timeout fires as `CamelError`; the
  concurrency cap holds under 100 concurrent exchanges; growth ratios stay inside the
  envelope (≤1.15 concurrency, ≤1.25 timeout).
- **Docs**: a "Custom middleware" guide plus a runnable example crate whose snippets the
  guide includes; rustdoc doctests on every new public API item.

Explicitly excluded: changes to `run_steps`, `OutcomePipeline`, `CompiledStep`, the route
lifecycle, or any existing builder method signature. No new dependencies — only new
`tower` features (`timeout`, `limit`, `buffer`) of the already-pinned `tower 0.5`.

## Acceptance criteria

- `.to_layered(...)`, `.layered(...)`, `.route_layer(...)` compile and run at all three
  granularities; existing builder signatures are unchanged.
- A producer wrapped with `TimeoutLayer` surfaces `CamelError` with the tower `Elapsed`
  error reachable via `source()`.
- Concurrency cap holds under 100 concurrent exchanges; measured ratios ≤1.15
  (concurrency) and ≤1.25 (timeout) against configuration.
- `RateLimit` state is shared across clones only through `Buffer` (compile-probed).
- Stop inside a layered section stops the route without running downstream steps.
- `cargo test -p camel-builder --doc`, `mdbook test`, and the probe battery pass.

## Risk budget

- `tower::limit::RateLimit` is not `Clone` (compile-probed 2026-11-06, tower 0.5.3);
  stateful middleware therefore requires `Buffer`. Buffer construction needs a tokio
  runtime at route-add time — routes are always added inside the runtime, and the probe
  battery locks this in.
- Canonical spec: layered steps are rejected with an explicit `RouteError` (precedent:
  custom split aggregation). Canonical representation of middleware is out of scope and
  tracked as a follow-up if needed.
- Stop re-marking unconditionally sets the `CamelStop` property to true when a stopped
  outcome crosses the Tower boundary inside a layered section; behavior outside layered
  sections is untouched.

Affected crates: `camel-api`, `camel-core`, `camel-builder`, `examples/middleware-layer`
(new), `docs`. Bd: rc-s5ezy.
