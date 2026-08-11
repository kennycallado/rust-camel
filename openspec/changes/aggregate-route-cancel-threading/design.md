# Design: aggregate-route-cancel-threading

## Approach

The aggregator becomes self-contained: it owns its sweep lifecycle internally
through a swappable token cell, driven by `StepLifecycle::start()`/`shutdown()`.
No external token threading is needed.

### Token cell (interior mutability)

Replace the plain `route_cancel: CancellationToken` field with a swappable
token cell:

```rust
sweep_cancel: Arc<Mutex<CancellationToken>>,
```

The constructor signature is **preserved** — the provided `route_cancel` seeds
the cell at construction. This keeps all existing call sites
backward-compatible. `poll_ready` clones the current token under the lock when
spawning the sweep. `start()` swaps in a new token; `shutdown()` cancels the
current one.

The `call()` path (line ~308) previously cloned `route_cancel` for
`spawn_timeout_task`, but that parameter was unused (`_route_cancel`). Deleting
the clone, the argument, and the unused parameter is part of this task.

### StepLifecycle hooks

The aggregator already implements `StepLifecycle` (`aggregator.rs:247`) with a
`shutdown()` that calls `shutdown_inner()` (cancels per-bucket timeout tasks).
The changes:

- **`start()` override** (currently default no-op): lock `sweep_cancel`, replace
  with a fresh `CancellationToken::new()`. Lock `sweep_handle`, abort any stale
  handle and set to `None`. The next `poll_ready` respawns the sweep bound to
  the new token.

- **`shutdown()` extension**: before calling `shutdown_inner()`, cancel
  `sweep_cancel` and abort+clear `sweep_handle`. Both `RouteStop` and `HotSwap`
  trigger this — the sweep must not survive a route stop or a hot-swap.

### DSL step-compiler wiring

`splitting.rs:327-334` changes:

```rust
// Before:
let cancel = CancellationToken::new();
let svc = AggregatorService::new(config, late_tx, registry, cancel);
// ...
    processor: BoxProcessor::new(svc),
    lifecycle: None,

// After (clone BEFORE move):
let cancel = CancellationToken::new();
let svc = AggregatorService::new(config, late_tx, registry, cancel);
let lifecycle: Arc<dyn StepLifecycle> = Arc::new(svc.clone());
Ok(CompileOutcome::Matched(CompiledStep::Process {
    processor: BoxProcessor::new(svc),
    body_contract: None,
    lifecycle: Some(lifecycle),
}))
```

The `AggregatorService::new` signature is unchanged. The `svc.clone()` is cheap
(`#[derive(Clone)]`, all fields are `Arc` or cloneable channels). The lifecycle
handle is cloned BEFORE the service is moved into `BoxProcessor`. The
`Arc<dyn StepLifecycle>` is registered so the runtime's `collect_lifecycle` →
`assembly.lifecycle` machinery picks it up, and `start_route`/`stop_route` drive
`start()`/`shutdown()` automatically.

### Why not thread route_cancel through CompilationContext

The compiled pipeline is immutable, but `pipeline_cancel_token` is cancelled and
replaced on each `stop_route` (`consumer_management.rs:367-371,441-446`). A
token bound at compile time stays cancelled after stop→restart — the sweep never
respawns. The runtime already solved this pattern for between-step cancellation
with `task_local! CANCEL_TOKEN` re-scoped per start
(`route_controller_trait.rs:304-308`). The `StepLifecycle` anchor applies the
same principle: `start()` re-seeds the token on each route start.

## Affected crates

- `camel-processor` — `aggregator.rs`: field replacement (`route_cancel` → `sweep_cancel` cell), `call()` cleanup, `start()` override, `shutdown()` extension. Constructor signature unchanged.
- `camel-core` — `splitting.rs`: drop local token, wire lifecycle handle.

## Architecture boundaries

The change reuses the `StepLifecycle` idiom (ADR-0022) that WireTap
(rc-wmuc) already uses. The sweep token is **independent** of
`pipeline_cancel_token` — sweep lifecycle is orthogonal to the drain-grace
cancellation of ADR-0043. No public API change beyond the `AggregatorService::new`
signature (which is internal to the step compiler and the route controller's
aggregate-split path).

## Restart safety

| Event | Sweep token | Sweep handle | poll_ready behavior |
|---|---|---|---|
| Construction | fresh | `None` | spawns sweep on first call |
| `start()` (route start) | fresh (reset) | `None` (cleared) | respawns sweep |
| `shutdown(RouteStop)` | cancelled | aborted+cleared | sweep terminated |
| `shutdown(HotSwap)` | cancelled | aborted+cleared | sweep terminated |

## Test observability

Two complementary tests, neither requiring production instrumentation:

1. **Compiler test** (camel-core, `splitting.rs` test module): compile an
   `<aggregate>` step and assert the compiled step's `lifecycle` field is
   `Some(...)` — not `None`. This proves the wiring.

2. **Aggregator module-internal test** (camel-processor, `aggregator.rs`
   `#[cfg(test)] mod tests`): has access to private fields (`sweep_handle`,
   `sweep_cancel`). Uses paused Tokio time to:
   - Drive `poll_ready` (spawns sweep), insert an expired bucket, advance time,
     assert it was swept.
   - Call `shutdown(RouteStop)`, assert `sweep_handle` is cleared and the task
     terminated.
   - Call `start()`, `poll_ready` again, assert sweep respawned with a fresh
     token.

## Alternatives considered

- **Thread `route_cancel` through `CompilationContext`** — rejected: reintroduces
  the compile-once/token-per-start bug. The token stays cancelled after
  stop→restart; the sweep never respawns. Additionally requires touching ~35
  call sites for zero benefit.
- **Token container swap without lifecycle** — rejected: no lifecycle event to
  drive the swap. `StepLifecycle::start()` is the correct anchor.
- **Recompile on restart** — rejected: violates the compile-once invariant, large
  blast radius.
- **Don't cancel `pipeline_cancel_token` on stop** — rejected: stop cancels it
  deliberately to kill stragglers past the drain grace (ADR-0043 amend).
