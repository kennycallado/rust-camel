# ADR-0031: WASM Source World

**Date:** 2026-07-01
**Status:** Accepted (spike validated e2e — 5/5 integration tests pass)

## Context

The three existing WIT worlds (`plugin`, `bean`, `authorization-policy`) are all guest-receives-exchange patterns. There is no WIT world for inbound sources — 3rd-party WASM components cannot act as Consumers. Half of the connectors that matter are sources.

## Decision

Add a 4th WIT world `source` using the resource negotiation pattern (Approach 3):

- The guest IS the source — it owns the consumption loop via `run(listener)`.
- The host provides raw capabilities (HTTP listener as a WIT resource).
- The guest calls `accept-http(listener)` to receive events and `submit-exchange(exchange)` to push them to the pipeline.
- Cancellation via channel close (blocking host functions) + epoch deadline (CPU-bound loops).
- Crash recovery via Consumer trait contract: trap → route Failed → restart recreates instance.

## Consequences

- 3rd-party WASM components can now be sources, not just processors/beans/security/sinks.
- The guest is limited to host-known transports (spike: HTTP only). Arbitrary socket access is NOT supported.
- Backpressure is host-controlled via bounded tokio channels.
- `stop()` is idempotent and safe to call on any exit path.

## Binary answers (spike outcome)

1. **Can WIT model guest-as-source cleanly?** YES.
   - Resource negotiation (`configure → source-plan → run(listener)`) works end-to-end.
   - Backpressure via bounded channel (capacity 1) is visible to the guest: `submit-exchange` blocks until the pipeline accepts.
   - Cancellation via channel close wakes `blocking_recv` in `accept-http`; guest exits `run()` cleanly.
   - Integration tests: lifecycle start/stop, e2e webhook, backpressure sequential — all pass.

2. **Is package distribution practical?** YES.
   - Guest dependencies are minimal: `wit-bindgen` only (no WASI SDK, no extra crates).
   - Debug .wasm is 3.5MB; release size not yet measured but expected <2MB with `opt-level = "z"`.
   - No signing/versioning in spike scope; existing `wasm:` URI scheme and path validation reused.

3. **Are crash/lifecycle semantics acceptable?** YES.
   - Guest trap → `call_run` returns `Err(wasmtime::Error)` → `spawn_blocking` task exits with `CamelError::ProcessorError` → runtime detects via `background_task_handle()` → route enters Failed state → restart recreates consumer.
   - `stop()` cancels token + `increment_epoch()` + graceful join with timeout — does NOT own `run_task` (runtime owns it via `background_task_handle()`).
   - Integration test: crash recovery — guest that traps on 3rd request → consumer reports error → test verifies error propagation.

## Spike findings (implementation notes)

### Critical lessons

- **Epoch deadline must be set before any guest call.** With `epoch_interruption(true)`, the store's default deadline is 0 (already expired). Without `store.set_epoch_deadline(N)` before `call_configure`, the guest traps at the first epoch check — which occurs inside the component model's lift/lower machinery (`cabi_realloc`), producing a misleading error that looks like a WIT/bindgen bug.
- **Sync bindings, not async.** The `exports: { default: async }` option forces `handle.block_on()` in `spawn_blocking`, which puts the blocking thread into a tokio runtime context. Host functions using `blocking_recv`/`blocking_send` then panic. Solution: use sync bindings and call `call_run` directly on the blocking-pool thread.
- **`with:` mapping for resources.** Wasmtime bindgen generates empty (uninhabited) enums for imported resources. Use `with: { "camel:plugin/source-host.http-listener": HttpListenerHandle }` to map to a concrete type, following the wasmtime-wasi pattern.
- **Stale build artifacts.** `wit_bindgen::generate!` does not emit `rerun-if-changed` for its `path:` wit dir. Integration tests must resolve the guest .wasm via `CARGO_TARGET_DIR` to avoid testing stale binaries.

### Known tech debt

- `to_plugin_wasm_exchange`: field-by-field converter between two `bindgen!` outputs (source vs plugin worlds). Eliminates when WIT-001 unifies type definitions.
- `path_filter` wired but minimally tested (axum routes on it, no filter-specific integration test).
- Guest crash variant uses config toggle (`crash=run`), not a separate .wasm artifact.

## References

- bd `rc-g2kr` — spike ticket
- bd `rc-9484` — cabi_realloc trap (closed; root cause: epoch deadline)
- `crates/camel-wit/wit/camel-source.wit` — WIT definition
- `crates/components/camel-component-wasm/src/source_consumer.rs` — host consumer
- `examples/wasm-source-webhook/` — guest example
- `crates/components/camel-component-wasm/tests/source_integration.rs` — 5 integration tests
