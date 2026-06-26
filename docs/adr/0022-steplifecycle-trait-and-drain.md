# ADR-0022: StepLifecycle Trait and Drain Policy for Stateful Pipeline Steps

**Date:** 2026-06-26
**Status:** Accepted (Phase 0)
**Amends:** ADR-0004, ADR-0018, ADR-0024, ADR-0025

## Decision

### Separate `StepLifecycle` trait (not on `Processor`)

Stateful pipeline steps (aggregators, idempotent repositories, resequencers, gap-detectors) own background work — timers, buckets, queues, timeout tasks — that outlives a single `process()` call. They need a shutdown hook. This hook is a **separate trait** (`StepLifecycle`), NOT a method on `Service<Exchange>`. Motivation: type erasure. `BoxProcessor = BoxCloneService<Exchange, Exchange, CamelError>` erases the concrete type; adding a trait method to `Service` would require a custom wrapper or vtable extension. A standalone trait collected at compile time avoids this.

File: `crates/camel-api/src/step_lifecycle.rs:31`

```rust
#[async_trait]
pub trait StepLifecycle: std::fmt::Debug + Send + Sync + 'static {
    fn name(&self) -> &'static str;
    async fn shutdown(&self, reason: StepShutdownReason) -> Result<(), CamelError>;
}
```

### `&self` receiver (NOT `&mut self`)

`StepLifecycle` uses `&self`, not `&mut self`. The runtime dispatches shutdown through `Arc<dyn StepLifecycle>` carried inside `ArcSwap<PipelineAssembly>` snapshots. Shared-reference dispatch means `Arc` cloning and concurrent snapshot reads work without mutation races. Implementations requiring mutable state use interior mutability (e.g. `Mutex`, `AtomicBool`).

Documented in doc comment at `crates/camel-api/src/step_lifecycle.rs:20-24`.

### Supertrait: `Debug + Send + Sync + 'static`

`#[async_trait]` provides `Future`-returning method support. `Debug` is required because `CompiledStep` derives `Debug` (`#[derive(Debug)]` on `CompiledStep` at `step_compilers/mod.rs:42`). `Send + Sync + 'static` are the standard `Arc` bounds for trait-object storage.

### `StepShutdownReason`

Two variants:

```rust
pub enum StepShutdownReason {
    RouteStop,   // stop_route is draining the pipeline
    HotSwap,     // pipeline being replaced via hot reload (Restart path)
}
```

File: `crates/camel-api/src/step_lifecycle.rs:6-11`.

### Storage: collected at compile time

Each stateful processor registers its `Arc<dyn StepLifecycle>` at compile time:

- **`CompiledStep::Process.lifecycle`**: `Option<Arc<dyn StepLifecycle>>` — single lifecycle handle for a stateful processor (`step_compilers/mod.rs:49`).
- **`CompiledStep::Segment.lifecycle`**: `Option<Vec<Arc<dyn StepLifecycle>>>` — multiple lifecycle handles for a structural EIP's nested stateful children (`step_compilers/mod.rs:64`). Uses `Vec` (not flattening into a single `Arc`) so multiple stateful children inside a structural EIP each register independently.
- **`PipelineAssembly.lifecycle`**: `Vec<Arc<dyn StepLifecycle>>` — flat aggregated list in the runtime pipeline snapshot (`pipeline_runtime.rs:22`).

Why `Option<Vec<...>>` for Segment, not `Option<Arc<dyn StepLifecycle>>`? A segment wraps an entire sub-pipeline (Filter, Choice, Loop, etc.) that may contain zero, one, or multiple stateful children. Each child has its own lifecycle. The Vec preserves independent identity — important for ADR-0025's outcome-aware structural EIPs where children can be nested segments themselves.

### `compile_children_segments` aggregates child lifecycle

`CompilationContext::compile_children_segments()` (`step_compilers/mod.rs:123`) recursively compiles child steps and accumulates lifecycle handles via `extend`:

- `CompiledStep::Process { lifecycle: Some(lc) }` → pushes `lc` into the accumulating vec (lines 144-146).
- `CompiledStep::Segment { lifecycle: Some(lcs) }` → extends the vec with `lcs` (lines 170-171).
- Stateless steps (`lifecycle: None`) contribute nothing.

The returned `(Vec<Box<dyn OutcomePipeline>>, Vec<Arc<dyn StepLifecycle>>)` pair is then stored in the parent `CompiledStep::Segment.lifecycle`. This recursive flattening ensures nested stateful children (e.g. Idempotent+Resequencer inside Filter) are discovered at compile time and reachable at drain time.

### Drain in `stop_route_internal`: POST-join, PRE-token-reset

Drain placement is precise. `stop_route_internal` (`consumer_management.rs:135`) follows this ordering:

1. Cancel consumer intake token (line 154).
2. `force_complete_all` on aggregator (line 160) — completes open buckets; emits exchanges into pipeline.
3. Cancel pipeline token (line 166).
4. Take handles and join both consumer + pipeline tasks (lines 171-193). Zero `process()` in flight after join.
5. **Drain stateful steps**: iterate `assembly.lifecycle` from ArcSwap snapshot, call `shutdown(RouteStop)` in route order (lines 202-220).
6. **Drain aggregator** via `StepLifecycle::shutdown` on `ManagedRoute.agg_service` (lines 225-240).
7. Reset cancellation tokens for future restarts (lines 245-246).

Invariant: intake cancelled + pipeline task joined BEFORE `shutdown()` is called. This ensures no concurrent `process()` when the lifecycle hook fires.

Shutdown `Err` is best-effort: `tracing::warn!` and continue (does NOT fail `stop_route`). Precedent: `CamelContext::stop` service handling. See `consumer_management.rs:212-218`.

### Hot-swap policy: REJECT lifecycle-bearing routes

`DefaultRouteController::swap_pipeline` (`route_controller.rs:659`) checks:

```rust
let assembly = managed.pipeline.load();
let has_lifecycle = !assembly.lifecycle.is_empty();
if has_lifecycle || managed.agg_service.is_some() {
    return Err(CamelError::RouteError(
        "Route '...' contains stateful steps (lifecycle-bearing). Hot-swap not supported — use restart."
    ));
}
```

Rationale: Atomic `ArcSwap` swap cannot safe-drain lifecycle handles. An in-flight request may hold the old Arc while the new pipeline is swapped in. If that request holds the last reference, the old pipeline (and its lifecycle) is dropped concurrently with the new pipeline starting — races with background timers/buckets from the old pipeline. The safe protocol is **stop → raw swap → start**:

1. `stop_route` — drains lifecycle via `StepLifecycle::shutdown`.
2. `swap_pipeline_raw` — bypasses the lifecycle check (route is stopped, no in-flight).
3. `start_route` — recreates consumer + pipeline.

`swap_pipeline_raw` lives at `route_controller.rs:702` and `pipeline_runtime.rs:70`.

### Aggregator: side-channel + unified drain

The aggregator is a **side-channel**: `ManagedRoute.agg_service` (`route_helpers.rs:120`), NOT a `CompiledStep`. It implements `StepLifecycle` directly (`aggregator.rs:181-191`) for unified drain.

Dual-phase shutdown:
1. **Pre-join**: `force_complete_all` (line 160 of `consumer_management.rs`) — completes open buckets, emits exchanges into the pipeline. The join then processes those emits.
2. **Post-join**: `shutdown(RouteStop)` on the `Arc<AggregatorService>` (lines 225-240) — drains remaining timeout tasks that the pipeline task's select loop may have spawned.

`aggregator.rs:186-190`:
```rust
async fn shutdown(&self, reason: StepShutdownReason) -> Result<(), CamelError> {
    tracing::debug!(reason = ?reason, "Aggregator shutdown via StepLifecycle");
    self.shutdown_inner().await;
    Ok(())
}
```

### Segment lifecycle is `Option<Vec<...>>` — multiple stateful children

`CompiledStep::Segment.lifecycle` (`step_compilers/mod.rs:64`) is `Option<Vec<Arc<dyn StepLifecycle>>>` rather than `Option<Arc<dyn StepLifecycle>>`. This is deliberate:

- A structural EIP (Filter, Choice, Loop, doTry, Split) may contain zero, one, or multiple stateful processors in its sub-pipeline.
- Each child registers its own `Arc<dyn StepLifecycle>`.
- The `Vec` accumulates via `extend` in `compile_children_segments` (lines 135-177).
- At drain time, `PipelineAssembly.lifecycle` is also a flat `Vec` — the `Option<Vec<...>>` from each `CompiledStep` is flattened into `PipelineAssembly.new()`.

This design does NOT preclude ADR-0024/0025 propagation of `PipelineOutcome::Stopped` through `Segment`: the `Segment`'s `OutcomePipeline::run` returns `PipelineOutcome` independently of its `lifecycle` Vec. The two are orthogonal — lifecycle for shutdown, `PipelineOutcome` for data-plane control flow.

## Context

### Problem

Before Phase 0, stateful pipeline steps had no shutdown hook. `stop_route` cancelled the pipeline task and reset tokens, but never notified background state (aggregator buckets, idempotent caches, resequencer queues, timeout tasks) that the route was stopping. This allowed:

- Orphaned timeout tasks firing after token reset.
- Aggregator buckets leaking when the route was stopped mid-completion.
- No mechanism for hot-swap to safely transfer or drain stateful resources.

ADR-0004 (hot-reload atomic pipeline swap) assumed stateless pipelines. ADR-0018 (two-phase route lifecycle) established `stop_route` as the canonical drain site. ADR-0024/0025 introduced `PipelineOutcome` and outcome-aware segments — these expanded the set of possible stateful steps (segments with child lifecycle handles). Phase 0 fills the gap.

### Quiescence policy

Old pipeline assembly retires via Arc refcounting. After `ArcSwap::store()`, the old `Arc<PipelineAssembly>` survives as long as any in-flight request holds it. No explicit quiescence queue is needed because the Restart path drains via `stop_route` (which joins before draining) before the new pipeline starts.

`swap_pipeline_raw` exists specifically for the Restart path — it bypasses the lifecycle check because the route is already stopped and drained.

### Shutdown `Err` handling: best-effort

`Err` from `StepLifecycle::shutdown` is logged and skipped — it does NOT fail `stop_route`. This mirrors `CamelContext::stop`'s handling of service shutdown errors. Rationale: a failing step should not prevent subsequent steps from draining, nor should it leave the route in a half-stopped state.

Implementation at `consumer_management.rs:208-218`:

```rust
if let Err(e) = step.shutdown(StepShutdownReason::RouteStop).await {
    tracing::warn!(step = step.name(), error = %e,
        "StepLifecycle shutdown failed during stop_route for route {}", route_id);
}
```

## Consequences

### Trait location

`StepLifecycle` lives in `camel-api` (`step_lifecycle.rs`), NOT in `camel-processor` or `camel-core`. This allows any crate (including component crates) to implement it without depending on core lifecycle internals. Imported by `camel-core`'s step_compilers (`step_compilers/mod.rs:11`).

### Interface stability

`StepLifecycle` is `pub` in `camel-api`. It is not gated behind an `internal-adapters` feature flag — any stateful processor or component may implement it. The trait is simple (one method + one accessor) and unlikely to change except to add additional reason variants.

### PipelineAssembly growth

`PipelineAssembly` gains a `Vec<Arc<dyn StepLifecycle>>` field. Cost: one allocation per stateful step (amortized by `Vec::extend`). Stateless routes carry an empty `Vec` (zero cost at runtime, 24 bytes in the struct). See `pipeline_runtime.rs:22`.

### No quiescence queue

The Restart path (stop → drain → swap → start) avoids the need for an explicit quiescence or deferred-drain mechanism. `swap_pipeline_raw` (`route_controller.rs:702`, `pipeline_runtime.rs:70`) is the non-checking variant for this path.

### ADR-0024/0025 interaction

`Segment.lifecycle` being `Option<Vec<Arc<dyn StepLifecycle>>>` (not a single `Option`) ensures multiple stateful children inside a structural EIP each register independently. ADR-0025's `OutcomePipeline` trait and `PipelineOutcome::Stopped` propagation through `Segment` are orthogonal to lifecycle drain — they operate on the data plane, not the control plane.

`PipelineOutcome::Stopped` propagation through `Segment` is a Phase 1/3 concern; Phase 0 does NOT implement it but does NOT preclude it.

### Phase 0 boundary

Phase 0 Tasks 1-7 implement:
- `StepLifecycle` trait + `StepShutdownReason` (`camel-api`).
- `CompiledStep` field additions (`Process.lifecycle`, `Segment.lifecycle`).
- `PipelineAssembly` + `new_shared_pipeline_with_lifecycle`.
- Drain loop in `stop_route_internal`.
- `swap_pipeline` lifecycle check + `swap_pipeline_raw`.
- `AggregatorService::StepLifecycle` impl.
- `compile_children_segments` lifecycle aggregation.

What Phase 0 does NOT do:
- Implement `StepLifecycle` on every stateful processor (Phase 1+).
- Wire `PipelineOutcome::Stopped` through Segment data plane (ADR-0025, already done in Phase 4 — but lifecycle collection predates it and is compatible).

## Load-bearing citations

| File:line | Element |
|---|---|
| `camel-api/src/step_lifecycle.rs:31` | `pub trait StepLifecycle` |
| `camel-api/src/step_lifecycle.rs:6` | `StepShutdownReason` enum |
| `step_compilers/mod.rs:43` | `pub enum CompiledStep` |
| `step_compilers/mod.rs:49` | `Process { lifecycle: Option<Arc<dyn StepLifecycle>> }` |
| `step_compilers/mod.rs:64` | `Segment { lifecycle: Option<Vec<Arc<dyn StepLifecycle>>> }` |
| `step_compilers/mod.rs:123` | `fn compile_children_segments` |
| `step_compilers/mod.rs:135-177` | Lifecycle aggregation via push/extend |
| `pipeline_runtime.rs:20` | `struct PipelineAssembly` |
| `pipeline_runtime.rs:22` | `lifecycle: Vec<Arc<dyn StepLifecycle>>` |
| `pipeline_runtime.rs:50` | `fn new_shared_pipeline_with_lifecycle` |
| `pipeline_runtime.rs:70` | `fn swap_pipeline_raw` (no lifecycle bypass) |
| `consumer_management.rs:135` | `fn stop_route_internal` |
| `consumer_management.rs:160` | `force_complete_all` (pre-join) |
| `consumer_management.rs:202-220` | Drain loop (post-join, pre-token-reset) |
| `consumer_management.rs:225-240` | Aggregator shutdown via StepLifecycle |
| `route_controller.rs:659` | `fn swap_pipeline` (lifecycle check) |
| `route_controller.rs:702` | `fn swap_pipeline_raw` (bypass) |
| `route_helpers.rs:120` | `ManagedRoute.agg_service` |
| `aggregator.rs:181` | `impl StepLifecycle for AggregatorService` |
