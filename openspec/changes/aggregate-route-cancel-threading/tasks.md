# Tasks: aggregate-route-cancel-threading

## camel-processor

### Task 1.1: AggregatorService internal sweep-cancel cell + StepLifecycle hooks

**Files:**
- `crates/camel-processor/src/aggregator.rs` (modified)

**Steps:**
1. Add a new field `sweep_cancel: Arc<Mutex<CancellationToken>>` to `AggregatorService`. Remove the plain `route_cancel: CancellationToken` field — its value moves into the cell.
2. In `AggregatorService::new`, seed `sweep_cancel` with `Arc::new(Mutex::new(route_cancel))`. The constructor signature stays `(config, late_tx, language_registry, route_cancel: CancellationToken)` — backward-compatible.
3. In `poll_ready` (line ~283), change `let cancel = self.route_cancel.clone();` to `let cancel = self.sweep_cancel.lock().unwrap_or_else(|e| e.into_inner()).clone();` so the sweep binds to the current cell token.
4. In `call()` (line ~308), delete `let route_cancel = self.route_cancel.clone();` and drop the `route_cancel` argument from the `spawn_timeout_task` call (line ~413). Delete the unused `_route_cancel` parameter from `spawn_timeout_task` (line ~631). This is required because step 1 removed the `route_cancel` field — the timeout-task path does not use the sweep token.
5. Override `StepLifecycle::start` (currently default no-op, trait line 49). Body: lock `sweep_cancel`, replace with `CancellationToken::new()`; lock `sweep_handle`, abort any existing handle and set to `None`. This resets the sweep so the next `poll_ready` respawns it bound to the fresh token.
6. Extend `StepLifecycle::shutdown` (line ~250). Before calling `self.shutdown_inner().await`, add: lock `sweep_cancel` and call `.cancel()`; lock `sweep_handle`, take the handle if `Some`, and call `.abort()`. Both `RouteStop` and `HotSwap` flow through this same code path.
7. Update the `Drop` impl: it currently aborts `sweep_handle`. Keep the logic as-is (defense-in-depth) but note that `shutdown` is now the primary path.
8. Update stale doc comments referencing `route_cancel` as a field or lifecycle invariant: the field comment at lines ~66-72, the `Drop` comment at ~84-93, and the `new()` doc at ~101-108. Replace references to `route_cancel` with `sweep_cancel` and explain the swappable-cell + `StepLifecycle` model.

**Tests:** (executable spec — module-internal tests in `aggregator.rs` `#[cfg(test)] mod tests`, with private field access)
- `sweep_shutdown_cancels_task`: `#[tokio::test]`. setup = build `AggregatorService` with `bucket_ttl: Some(100ms)` (plus any mandatory bounds per `AggregatorConfig::validate()`), call `poll_ready` to spawn the sweep, assert `sweep_handle` lock is `Some`. action = call `shutdown(StepShutdownReason::RouteStop).await`. assert = `sweep_handle` lock is `None` (taken + aborted), `sweep_cancel` lock `.is_cancelled()` is `true` (token cancelled, sweep's `select!` cancel branch fires). Then `tokio::time::sleep(50ms).await` to let the aborted task unwind, confirming no panic.
- `sweep_start_respawns_after_shutdown`: `#[tokio::test]`. setup = same as above, then `shutdown(RouteStop)`. action = call `start().await`, then `poll_ready`. assert = `sweep_handle` is `Some` again, `sweep_cancel` lock `.is_cancelled()` is `false` (fresh token).
- `sweep_shutdown_hotswap_cancels_task`: `#[tokio::test]`. same as `sweep_shutdown_cancels_task` but with `StepShutdownReason::HotSwap`.

**Acceptance:**
- `cargo clippy -p camel-processor -- -D warnings` exits 0
- `cargo test -p camel-processor --lib aggregator` passes all tests
- `AggregatorService::new` signature unchanged (4 params, same types)
- No new `unwrap()` calls (use `unwrap_or_else(|e| e.into_inner())` per existing pattern)

- [ ] 1.1

## camel-core

### Task 2.1: DSL Aggregate step-compiler registers lifecycle handle

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified)

**Steps:**
1. Add `use camel_api::StepLifecycle;` to the imports at the top of `splitting.rs` if not already present. Use the unqualified `StepLifecycle` in the code below.
2. In the `BuilderStep::Aggregate` arm (line ~322-334), after constructing `svc`, clone the lifecycle handle BEFORE moving `svc` into `BoxProcessor`. The exact code:
```rust
let svc = camel_processor::AggregatorService::new(config, late_tx, registry, cancel);
let lifecycle: Arc<dyn StepLifecycle> = Arc::new(svc.clone());
Ok(CompileOutcome::Matched(CompiledStep::Process {
    processor: BoxProcessor::new(svc),
    body_contract: None,
    lifecycle: Some(lifecycle),
}))
```

**Tests:** (executable spec — in `splitting.rs` or `step_compilers/mod.rs` test module)
- `aggregate_step_registers_lifecycle_handle`: setup = build a `CompilationContext` test fixture (following existing patterns in `mod.rs` test module), construct a `BuilderStep::Aggregate` with a config that passes `AggregatorConfig::validate()` (e.g. `bucket_ttl: Some(Duration::from_millis(100))` plus any mandatory bounds), compile via the registry. action = inspect the returned `CompiledStep::Process`. assert = `.lifecycle` is `Some(...)`, not `None`.
- `aggregate_dsl_shutdown_drives_lifecycle`: `#[tokio::test]`. setup = compile a `BuilderStep::Aggregate` via the DSL step-compiler, extract the `Arc<dyn StepLifecycle>` from the compiled step's `lifecycle` field (unwrap the `Some`). action = call `shutdown(StepShutdownReason::RouteStop).await` through the trait handle. assert = `shutdown` returns `Ok(())`. This proves the DSL-compiled aggregator's lifecycle handle is registered and drivable through the runtime's shutdown path. (Sweep-termination itself is proven by the module-internal `sweep_shutdown_cancels_task` in Task 1.1, which has private-field access — `Arc<dyn StepLifecycle>` does not support downcast, so the integration test proves wiring, the unit test proves termination.)

**Acceptance:**
- `cargo clippy -p camel-core -- -D warnings` exits 0
- `cargo test -p camel-core --lib splitting` passes all tests
- The `svc` is cloned BEFORE the move into `BoxProcessor::new(svc)` (compile-time check)
- The `lifecycle` field on the compiled `Aggregate` step is `Some` (runtime test)

- [ ] 2.1
