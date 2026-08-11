# Proposal: aggregate-route-cancel-threading

## Why

The Aggregate step-compiler (`splitting.rs:327-334`) creates a local
`CancellationToken::new()` and sets `lifecycle: None` on the compiled step. As a
result:

1. The aggregator's background TTL-sweep task is bound to a disconnected token
   that is never cancelled — the sweep leaks past route stop/shutdown.
2. The aggregator's `StepLifecycle` handle is not registered, so the runtime's
   start/stop drain never drives the aggregator's sweep lifecycle.

The runtime already solved the compile-once/token-per-start mismatch for
between-step cancellation with a `task_local! CANCEL_TOKEN` re-scoped on each
start (`route_controller_trait.rs:304-308`). Threading `route_cancel` through
`CompilationContext` would reintroduce the exact bug that design avoids.

Discovered during the rc-wmuc architect review
(`docs/reviews/wiretap-rc-wmuc-architect-guidance.md` §c). WireTap does not
depend on this change (it uses its own `StepLifecycle` anchor).

## What Changes

- Replace `AggregatorService.route_cancel` field with an internal swappable token
  cell (`Arc<Mutex<CancellationToken>>`). The constructor signature is preserved
  — the provided `route_cancel` seeds the cell.
- Override `StepLifecycle::start()` to reset the cell with a fresh token and
  clear the sweep handle, so `poll_ready` respawns the sweep on route restart.
- Extend `StepLifecycle::shutdown()` to cancel the sweep token and abort the
  sweep task before running existing `shutdown_inner()`.
- Change `splitting.rs:333` from `lifecycle: None` to
  `lifecycle: Some(...)`, cloning the service BEFORE moving it into
  `BoxProcessor`, wiring the aggregator into the runtime's lifecycle drain.
- Zero changes to `AggregatorService::new` signature, `CompilationContext`,
  `resolve_steps`, or any call site.

## Acceptance criteria

- `AggregatorService` owns its sweep lifecycle internally through a swappable
  token cell; constructor signature is preserved (backward-compatible).
- `StepLifecycle::start()` resets the sweep token + handle so the sweep
  respawns on route restart.
- `StepLifecycle::shutdown()` cancels the sweep on `RouteStop` and `HotSwap`.
- The DSL step-compiler registers the lifecycle handle (`lifecycle: Some(...)`).
- Regression test: stop → sweep terminates; start → sweep respawns.
- `cargo fmt`, `cargo clippy`, and `cargo test` green for affected crates.
