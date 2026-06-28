# ADR-0029: Resequencer Continuation Boundary

**Date:** 2026-06-27
**Status:** Accepted (Phase 3)
**References:** ADR-0022 (StepLifecycle), ADR-0024 (PipelineOutcome/CamelStop), ADR-0025 (Outcome-aware structural EIPs)
**Related:** Phase 3 — Tasks 1a, 1b, 2, 3 (Resequencer EIP)

## Context

The Resequencer EIP reorders incoming message streams back into their original sequence. It has two modes:

- **Batch:** buffer a window of messages (by size/timeout per correlation key), sort, then burst-emit in order.
- **Stream:** hold out-of-order messages in a priority queue, emit the contiguous run starting at the next-expected sequence number, with gap detection and capacity management.

The fundamental architectural challenge: a Resequencer receives ONE input exchange via the unary Tower `Service<Exchange>` contract, but may produce ZERO outputs (buffering), ONE output (normal), or MULTIPLE outputs (batch burst / stream gap drain). This conflicts with the unary pipeline model where `call(input) -> Result<Exchange>`.

## Decision

### Continuation-boundary design

The route compiler splits the flat step list at the top-level `Resequence` boundary into three partitions:

```
pre_steps  →  ResequencerService  →  [post-steps compiled as a BoxProcessor continuation]
```

- **`pre`** compiles normally into the main pipeline (before the resequencer).
- **`post`** compiles via `compose_pipeline_with_contracts` into a `BoxProcessor` continuation owned by the `ResequencerService`.
- The resequencer is the **LAST step** of the main pipeline.

`ResequencerService::call(input)` sends the exchange into a bounded actor channel and returns an **ack** (`Body::Empty` + property `CAMEL_RESEQUENCER_ACCEPTED=true`). The actual reordered payloads flow asynchronously through a **post-driver** task that drives the continuation:

```
input → actor channel → policy.accept() → ready exchanges → post-driver → continuation.call(ex)
```

The whole route is **ONE `PipelineAssembly`** (no side-channel) — the resequencer's `CompiledStep.lifecycle` is `Some(Arc<ResequencerService>)`, so the Phase 0 `StepLifecycle` drain mechanism reaches it for stop/hot-swap.

### Why NOT the aggregator split-route

The Aggregator uses `find_top_level_aggregate_requiring_split` + two independent `SharedPipeline`s + `agg_service` side-channel + a warn-and-proceeds hot-swap. The resequencer deliberately avoids this shape:

1. The resequencer's single `PipelineAssembly` swaps atomically **with full lifecycle drain** (unlike the aggregator's two-pipeline shape that cannot drain via `CompiledStep.lifecycle`).
2. No `agg_service` side-channel — the resequencer IS a pipeline step, reachable by the standard lifecycle drain.
3. Hot-swap for lifecycle-bearing routes uses the **Restart path** (stop → drain → swap → start), not a warn-and-proceeds no-op.

### Hot-swap drain semantics

On `HotSwap`: complete in-flight exchanges through the **OLD continuation** (ADR-0004 in-flight-finishes-old semantics — NOT discard), then quiesce. The `StepLifecycle::shutdown` ordering:

1. Set shutdown flag; close input channel (actor sees EOF).
2. Await actor `JoinHandle` (bounded deadline).
3. `policy.flush()` — emit remaining in order via post-driver.
4. Close post-driver channel sender.
5. Await post-driver `JoinHandle` (5s deadline).
6. Drain post-step lifecycles (Phase 3: post-steps with lifecycle are **rejected** at compile time; this step is a structural placeholder for future use).

### Backpressure

The input channel is **bounded** (`tokio::sync::mpsc` with configurable capacity, default 1024). `Service::call` uses `send().await` — backpressure propagates into the consumer when the actor falls behind.

### InOnly/ack semantic consequence

Request-reply final responses are **NOT preserved** through the unary Tower contract — the route is effectively `InOnly` past the resequencer. A runtime InOut guard in `ResequencerService::call` inspects `exchange.pattern`:

- If `InOut` and not `allow_inout: true`:
  - Increment a durable metric counter (`resequencer_inout_warnings_total`).
  - Emit a rate-limited `warn!` (once per 30s per route, NOT per-exchange).
  - Set diagnostic property `CAMEL_RESEQUENCER_INOUT_WARN=true` on the ack.

### CamelStop interaction (Phase 2)

The post-driver checks `camel_api::is_camel_stop(&ex)` **before** calling the continuation — if true, the exchange is skipped (analogous to `route_compiler.rs:345`). This prevents downstream processing of stop-signaled exchanges past the resequencer boundary.

### Compile-time rejection rules

- **N2 (mutual exclusion):** `assert_no_mixed_top_level_splits` rejects any route containing BOTH a top-level aggregate-requiring-split step AND a top-level `Resequence`. The predicate tests EVERY top-level `Aggregate` for `has_timeout_condition || force_completion_on_stop` (NOT `find_top_level_aggregate_requiring_split`, whose first-match-then-break under-detects).
- **N3:** Reject more than one top-level `Resequence`.
- **N4:** Reject any `Resequence` reached via `compile_children` (nested inside Choice/Split/Loop/Filter). The step-compiler registry arm returns `RouteError("resequence must be a top-level step")` unconditionally.

### Post-ack continuation failure taxonomy

A `continuation.call(ex)` failure happens AFTER `call()` returned the ack, so the exchange has left the ADR-0019 pipeline loop — `RouteErrorHandler` is **NEVER consulted**. The post-driver:

1. Logs at `warn!` (ADR-0012 best-effort).
2. Increments `resequencer_post_ack_failures_total{route}` metric.
3. Does NOT count against the route's error budget unless explicitly configured.

### Policy trait

```rust
#[async_trait]
pub trait ResequencePolicy: Send + Sync + 'static {
    async fn accept(&self, input: Exchange) -> Vec<Exchange>;
    async fn flush(&self) -> Vec<Exchange>;
    fn name(&self) -> &'static str;
    fn set_timeout_tx(&self, _tx: mpsc::Sender<Exchange>) { /* default no-op */ }
}
```

`set_timeout_tx` has a default no-op implementation — `BatchPolicy` and `StreamPolicy` override it to store the driver channel for self-spawned timeout tasks. `PassthroughPolicy` inherits the no-op.

### Stream capacity + gap failure policies

The stream policy's failure modes use honest naming (no false promises of dead-letter routing that isn't wired):

- `CapacityPolicy::LogAndDrop` — log `warn!` + drop the incoming exchange (queue full).
- `GapPolicy::DropAndLog` — gap timer fired, drop held exchanges + log.

Future: wire a DLQ sink for both policies.

### Scatter-Gather reconciliation (spec §5 correction)

Spec §5 described Scatter-Gather as "feeding a single aggregator with a correlation key." This is **INCORRECT** — canonical Hohpe/Woolf Scatter-Gather is the **stateless** form: parallel fan-out to N endpoints, combine N responses into ONE exchange. No correlation key. The stateful form (with correlation key) is the separate **Aggregator** EIP.

`scatter_gather` is a pure DSL alias that lowers to `Multicast` with `parallel: true` and the configured aggregation strategy (`LastWins`/`CollectAll`/`Original`). No new processor, no new runtime primitive.

## Rejected Alternatives

- **Pure `Process` mode** (inline reordering in `run_steps`): impossible — the unary Tower contract cannot emit multiple outputs from one `call()`.
- **Couple emit to input** (return the burst from `call()`): breaks unary semantics and cannot flush on shutdown (no input arrives).
- **Reopen the spec** (change Tower contract to multi-output): loses hot-swap drain and breaks every existing processor.
- **Aggregator split-route shape**: two pipelines + side-channel — cannot drain via `CompiledStep.lifecycle`, hot-swap is a warn-and-proceeds no-op.
