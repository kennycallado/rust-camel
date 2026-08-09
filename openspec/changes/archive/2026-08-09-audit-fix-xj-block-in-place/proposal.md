# Proposal: audit-fix-xj-block-in-place

## Why

`XjComponent::block_on_result` (camel-xj `component.rs:362-381`) uses
`tokio::task::block_in_place` to bridge the sync `create_endpoint` trait
method into async bridge-startup and stylesheet-compile calls. This has two
defects:

1. **Panic on current-thread runtime.** `block_in_place` panics when the
   ambient runtime is `RuntimeFlavor::CurrentThread`. Any embedder or test
   that runs on a single-threaded Tokio runtime hits an unrecoverable panic
   during route compilation.

2. **Dead Channel in the no-runtime path.** The `else` branch builds an
   ephemeral `current_thread` runtime, runs `ensure_bridge_started` on it,
   then drops the runtime at end of scope. The tonic `Channel` produced by
   `BridgeProcess::start_and_connect` spawns an internal dispatch task on
   that ephemeral runtime. When the runtime is dropped, the dispatch task
   dies, and the `Channel` stored in `BridgeState::Ready` becomes a dead
   channel. The first `transform` call on the route runtime receives
   `DispatchGone` / `Unavailable`. The project already documented this
   pattern in `camel-test/tests/support/mod.rs` (`bridge_bg_rt`).

Both defects share one root cause: the object produced inside the blocking
shim (a tonic `Channel`) escapes the function and captures runtime affinity,
but neither `block_in_place` nor the ephemeral runtime guarantees a stable
host for that affinity.

Bd issue: `rc-gbrh`.

## What Changes

- **Add `OffloadRuntime`** to camel-xj: a owned multi-thread Tokio runtime
  (1 worker) stored in `XjComponent` via `Arc`. The runtime lives for the
  Component's lifetime, so the tonic Channel's dispatch task has a stable
  host.
- **Rewrite `block_on_result`** to use the offload runtime for current-thread
  and no-ambient-runtime cases. Multi-thread runtime callers keep using
  `block_in_place` (unchanged, already correct).
- **Eliminate the dead-channel path**: the `else` branch (ephemeral runtime)
  is removed entirely.
- **Add tests** covering all three runtime flavors.

**Excluded:** making `create_endpoint` async (requires upstream trait change,
tracked as TODO(XJ-014)). The other `block_in_place` sites in camel-core and
camel-mock will be filed as separate `discovered-from: rc-gbrh` issues.

## Acceptance criteria

- `create_endpoint` does NOT panic on a current-thread Tokio runtime.
- `create_endpoint` does NOT produce a dead Channel in the no-runtime path.
- Multi-thread runtime behavior is unchanged.
- The offload runtime is cleaned up when the Component is dropped.
- `cargo clippy -p camel-xj -- -D warnings` passes.

## Risk budget

- **Accepted:** 2 OS thread spawns per endpoint creation (one-off setup cost,
  negligible). Hybrid approach (multi-thread uses `block_in_place`, others use
  offload) minimises blast radius on the production path.
- **Out of bounds:** trait-level async `create_endpoint`, camel-core/camel-mock
  `block_in_place` sites, performance optimisation of the offload path.
