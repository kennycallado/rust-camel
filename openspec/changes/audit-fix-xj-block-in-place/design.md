# Design: audit-fix-xj-block-in-place

## Approach

Introduce an `OffloadRuntime` — an owned multi-thread Tokio runtime (1 worker)
stored in `XjComponent` — and rewrite `block_on_result` to route the
current-thread and no-runtime cases through it.

### Why not pure error-on-current-thread?

Returning `Err` on current-thread runtimes trades a rare panic for a
guaranteed startup failure for any embedder that uses `new_current_thread()`.
For v1.0 the component should be runtime-agnostic.

### Why not lazy initialisation (like camel-xslt)?

camel-xslt defers bridge startup and stylesheet compilation to `poll_ready`
(first message), avoiding `block_on_result` entirely. camel-xj deliberately
eager-compiles the stylesheet during `create_endpoint` to surface
configuration errors at route setup time, before the first message. Changing
this to lazy is a semantic change with broader blast radius — out of scope for
this fix.

### The hybrid `block_on_result`

```
fn block_on_result(fut):
    if ambient runtime is MultiThread:
        block_in_place(|| handle.block_on(fut))   // unchanged
    else:
        offload.block_on(fut)                       // new path
```

The offload path uses `std::thread::scope` to spawn a scoped thread that
enters the offload runtime context and calls `runtime.block_on(fut)`. The
calling thread blocks on `scope.join()`. The scoped thread has no ambient
runtime, so `Runtime::block_on` does not panic.

The tonic `Channel` produced by `ensure_bridge_started` spawns its dispatch
task on the offload runtime. Because the offload runtime lives in the
`XjComponent` (via `Arc`), the dispatch task outlives `create_endpoint` and
survives until the Component is dropped.

### Runtime cleanup

`OffloadRuntime` holds a `Option<tokio::runtime::Runtime>`. Dropping a
`tokio::runtime::Runtime` inside an async context panics ("Cannot drop a
runtime in a context where blocking is not allowed"). Since `XjComponent` is
registered as `Arc<dyn Component>` in the context registry and the context
drops at the end of `async fn run`, production shutdown drops the component
inside an async context.

To avoid this panic, `OffloadRuntime` implements `Drop` by moving the
`Runtime` onto a scoped OS thread before dropping it:

```rust
impl Drop for OffloadRuntime {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            std::thread::scope(|s| { s.spawn(|| drop(rt)); });
        }
    }
}
```

When the last `Arc<OffloadRuntime>` is released (Component drop), this `Drop`
impl runs. The bridge process is stopped first via
`XjBridgeRuntime::shutdown()`, so the dispatch task is cancelled after the
bridge is already down.

### Terminology

`OffloadRuntime` wraps a `tokio::runtime::Runtime` (a Tokio async executor),
not the camel-core `Runtime` (the integration runtime that owns Routes and
the context). The name `OffloadRuntime` refers exclusively to the Tokio
executor.

### Scope choice: per-Component

The offload runtime is owned by `XjComponent` (1 worker thread, per-Component
instance). This is a deliberate v1.0 scope choice: `camel-xslt` uses lazy
bridge startup and needs no offload runtime, so sharing has no correctness
payoff. A shared offload runtime across bridge-backed components can be
evaluated post-v1.0 if thread-count consolidation becomes a concern.

### Doc comment update

The `block_on_result` doc comment (currently lines 352-361) still describes
the removed ephemeral-runtime branch. It must be rewritten to describe the
hybrid approach: `block_in_place` on multi-thread, offload runtime on other
flavours.

## Affected crates

- **camel-xj**: add `OffloadRuntime` struct, add field to `XjComponent`,
  rewrite `block_on_result`, remove ephemeral-runtime `else` branch, add
  tests.

## Architecture boundaries

This change is entirely within the Components layer. No Runtime, DSL, or
contract-API changes. The `Component::create_endpoint` sync signature is
unchanged. The fix is internal to camel-xj's bridge-startup shim.

The `OffloadRuntime` follows the same pattern as `camel-test/support/mod.rs`
`bridge_bg_rt()`: a long-lived multi-thread runtime that hosts tonic Channel
dispatch tasks. The difference is lifecycle: `bridge_bg_rt` intentionally
leaks its runtime (test-process lifetime); `OffloadRuntime` is owned by the
Component and dropped on Component teardown.

## Alternatives considered

- **Error on current-thread runtime**: rejected — narrows the runtime contract
  without deliberate sign-off, trades rare panic for broader startup failure.
- **Lazy bridge startup (xslt pattern)**: rejected — changes eager-compile
  semantics, larger blast radius for v1.0.
- **Spawn + std channel** (instead of scoped thread): rejected — requires
  `F: Send + 'static`, breaking the current borrow-based API.
- **Persistent offload thread** (channel-driven loop): rejected — over-engineered
  for a one-time setup operation; `std::thread::scope` is simpler and sufficient.
