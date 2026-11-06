# Design: builder-layer-middleware

## Approach

Three granularities, one mechanism: every hook wraps the next compiled unit with a
type-erased tower `Layer` at step-compile time. The design was locked AFTER a compile
probe (2026-11-06, rustc 1.99.1, tower 0.5.3); probe findings below.

### Probe findings (hazard clause)

1. **`tower::limit::RateLimit` is NOT `Clone`** (`RateLimit<BoxProcessor>: Clone` fails,
   E0277): its state lives inline in the service, so it cannot be erased into
   `BoxProcessor` (`BoxCloneSyncService`). `tower::buffer::Buffer` is REQUIRED for
   stateful layers; `Buffer` clones share one worker (probe: 5 clones at rate 2/s took
   2.01s — state shared). `Buffer::new` needs a tokio runtime at construction; route-add
   always runs inside the runtime (verified by the probe battery).
2. **`Timeout`/`ConcurrencyLimit` wrap `BoxProcessor` cleanly**; `Timeout` errors are
   `BoxError` wrapping `tower::timeout::error::Elapsed` (downcast verified; Display
   "request timed out"). `ConcurrencyLimit` cap held at ratio 1.000 (20 concurrent
   tasks, cap 2).
3. **Readiness contract is load-bearing**: an adapter that clones the layered service
   inside `call()` and re-`ready()`s the clone DEADLOCKS permit-style layers under
   concurrency (probe-instrumented: every concurrent handle holds an unconsumed
   `poll_ready` permit forever). The adapter must let `poll_ready` reserve on the
   handle and consume on the SAME handle in `call()`. `run_steps`' per-invocation
   `processor.clone()` is safe: cloning resets permit state and each clone
   acquires+consumes on itself.
4. **mdbook doctests cannot resolve workspace crates** (standalone rustdoc), so the
   guide follows the repo convention: snippets `{{#include}}`d from a runnable example
   crate and fenced as `rust,ignore` (mdbook skips them; the example crate itself is
   compiled separately via `cargo check -p middleware-layer` in the gate battery). The
   "doctests that compile and run" requirement is carried by rustdoc doctests on the
   new APIs (`cargo test -p camel-api --doc`, `cargo test -p camel-builder --doc`;
   already in CI docs.yml).
5. **Probe precision contract** (locked into the spec scenarios): the timeout probe
   asserts the timeout-classified error AND elapsed ≥ t (no early firing) with the
   clock at the exchange entering the layered processor; the rate probe defines its
   clock origin (first request arriving at the layered service) and its denominator
   (`floor((k-1)/r) * p`, the required delay for the `k-r` requests beyond the first
   burst); the concurrency probe keeps STRICT cap enforcement (≤ n) alongside the
   ≤1.15 envelope ratio. Ratios measured in the probe: concurrency 1.000, timeout 1.04,
   rate 1.006 (2.01s vs required 2.0s). **Retrievable evidence**: the probe crate is
   committed at `openspec/changes/builder-layer-middleware/probe/` (Cargo.toml with the
   exact `tower 0.5` feature list `["util", "timeout", "limit", "buffer"]`, src/main.rs,
   PROBE.md with commands, toolchain rustc 1.99.1 / tower 0.5.3, and the evidence
   table). Reproduce with `cargo check --tests && cargo test` inside that directory
   (path dep points at the checkout's `crates/camel-api`).

### API shape (camel-api)

- `trait ProcessorLayer: Send + Sync + 'static { fn wrap(&self, inner: BoxProcessor) ->
  Result<BoxProcessor, CamelError>; }` — object-safe factory. Blanket impl over
  `L: tower::Layer<BoxProcessor>` where `L: Send + Sync + 'static` and
  `L::Service: Service<Exchange, Response = Exchange> + Clone + Send + Sync + 'static`,
  `L::Service::Error: Into<BoxError>`, `L::Service::Future: Send`. Non-qualifying
  layers fail to compile at the call site (static guarantee, no runtime surprise).
- `CamelErrorAdapter<S>` — the map-err shim; pass-through `poll_ready`, same-handle
  `call` (finding 3).
- `impl From<BoxError> for CamelError` — first unwraps a boxed `CamelError` unchanged
  (passthrough; tower layers whose inner error IS a `CamelError` must not get
  double-wrapped, or native exception matching and fault classification change), then
  walks the full source chain for `tower::timeout::error::Elapsed` into
  `ProcessorErrorWithSource("middleware timeout elapsed", …)`, everything else into
  `ProcessorErrorWithSource("middleware error", …)`; the original error stays reachable
  via `source()`/`downcast_ref` at every level.
- `LayerFactory(Arc<dyn ProcessorLayer>)` — `Clone + Debug` (redacted), stored in
  `BuilderStep`.
- `SegmentProcessor` — runs an `OutcomeSegment` as `Service<Exchange, Error =
  CamelError>`; `Stopped(ex)` maps to `Ok(ex)` after ALWAYS setting the `CamelStop`
  property to true (unconditional overwrite — an exchange carrying an explicit
  `CamelStop=false` from an earlier step must not defeat the re-derivation), so
  `run_steps`' `Process` arm re-derives `Stopped` (ADR-0023 precedent:
  IdempotentConsumer must stay Segment-mode for the same reason).

### Compiler support (camel-core, additive)

- `BuilderStep::LayeredTo { uri, factory }` — endpoints compiler resolves the producer
  exactly like `To` (interception aware), then `factory.wrap(producer)`; produces a
  `CompiledStep::Process` keeping the resolved lifecycle, body contract, and `to_uri`.
- `BuilderStep::Layered { factory, steps }` — compiles children via
  `compile_children_segments` → `compose_outcome_segment` → `SegmentProcessor` →
  `factory.wrap` → `BoxProcessorSegment` → `CompiledStep::Segment`. Segment-mode is
  mandatory (ADR-0023/0025): a `Stop` inside the section must propagate as `Stopped`,
  which a `Process`-mode wrap would flatten.
- Exhaustive-match maintenance: `span_label` (`to:{scheme}` / `layered`),
  `to_uri_metadata` (`Some` / `None`), `span_kind_hint` (scheme-classified / `Internal`),
  `canonical_step_name` (`to_layered` / `layered`), and the canonical rejection lists
  (`CANONICAL_CONTRACT_RUST_ONLY_STEPS`).

### Builder hooks (camel-builder, additive only)

- `StepAccumulator::to_layered(uri, layer)` — producer granularity; explicit, no
  pending-state magic, mirrors `to`.
- `StepAccumulator::layered(layer, |section| …)` — section granularity; the closure
  receives a dedicated `SectionBuilder` (a minimal step accumulator: inherits the
  step methods, exposes NO `.parameters()`/`.route_layer()`/`build()`, so section
  misuse is impossible by construction) and its steps become one `Layered` step.
  Matches the builder's consuming style; available on every `StepAccumulator`
  implementor (route, filter bodies, split bodies, do-try bodies, …).
- `RouteBuilder::route_layer(layer)` — whole-route granularity; collects factories,
  `build()` nests the entire step list into `Layered` (repeated calls nest: the last
  call is the outermost layer). Declarative and position-independent, consistent with
  `error_handler`/`route_id` route metadata. Documented semantics: the route error
  handler retries the whole wrapped unit (whole route) rather than per-step — the
  user opted into route-wide middleware.
- Ergonomics argument: three named hooks beat one generic "pending layer" flag because
  the pending-flag design cannot express producer wrapping at all (producer services do
  not exist until endpoint resolution at compile time) and silently changes meaning
  depending on what step follows. The section closure's dedicated `SectionBuilder`
  keeps the step-method surface (to/process/set_header/…, plus nested
  `to_layered`/`layered`) while making route-level misuse (.parameters/.route_layer)
  unrepresentable.

## Affected crates

- `camel-api`: `ProcessorLayer`, `LayerFactory`, `CamelErrorAdapter`, `SegmentProcessor`,
  `From<BoxError> for CamelError`, canonical rejection-list entries.
- `camel-core`: two `BuilderStep` variants + arms in the step-compiler registry
  (`endpoints.rs`, new `middleware.rs`), exhaustive-match maintenance. No control-plane
  changes.
- `camel-builder`: three hook methods, `tower` feature additions
  (`timeout`, `limit`, `buffer`), doctests, probe-evidence test battery.
- `examples/middleware-layer` (new): runnable example included by the guide.
- `docs`: `src/extending/custom-middleware.md` + SUMMARY entry.

## Architecture boundaries

- Runtime/control plane: untouched — `run_steps`, `OutcomePipeline`, `CompiledStep`, and
  the route lifecycle compile the new steps through the EXISTING registry dispatch.
- DSL: purely additive builder methods; no signature changes.
- Components: producers are wrapped post-resolution, component code unaware.
- Services/Languages/Functions: not touched.

## Risk budget

- `Buffer` requires a tokio runtime at route-add time — a documented runtime
  precondition of `tower::buffer`, not an error path this change invents; routes are
  always added inside the runtime (probe battery locks this in).
- Canonical v2 rejects layered steps explicitly (precedent: custom split aggregation);
  canonical round-trip is a documented follow-up.
- Stop re-marking overwrites the `CamelStop` property to true only at the
  layered-section boundary when a `Stopped` outcome crosses into Tower-result world;
  behavior outside layered sections is untouched.
- Single-phase delivery: API + compiler arms + probe evidence + docs land as one
  coherent slice.
