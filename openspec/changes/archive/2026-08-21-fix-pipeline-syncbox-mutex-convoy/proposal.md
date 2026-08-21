# Proposal: fix-pipeline-syncbox-mutex-convoy

## Why

Under real concurrent load (external report, bd rc-vdy2: wrk, 300 connections, pure cache-HIT
route, 22 idle cores), a single route serializes ALL exchanges through one `std::sync::Mutex`
before the pipeline runs: p50 958 ms, p99 1.97 s, ~14% timeouts at 2 s, throughput inversion
(230 req/s at c300 vs 348 req/s at c16). The lock is `SyncBoxProcessor::clone_inner()`
(camel-api/src/processor.rs), invoked on EVERY exchange by both route topologies
(route_controller_trait.rs:249 Concurrent, :296 Sequential). The Mutex exists only to recover
`Sync` for `tower::util::BoxCloneService` (which is `Send` but not `Sync`); it protects no
mutable state. A contended blocking lock inside tokio worker tasks produces a futex convoy —
idle CPU, flat throughput, multimodal latency tail.

Published benchmarks missed it because the bench route is a single trivial step (negligible
clone hold time) and the loadgen has no connection-count knob (bd rc-qv1x tracks that gap).

## What Changes

Swap the erased pipeline type to tower's upstream `BoxCloneSyncService` (tower 0.5, already
the workspace-wide dependency; its erased inner trait carries `+ Send + Sync`):

- camel-api: `pub type BoxProcessor = BoxCloneSyncService<Exchange, Exchange, CamelError>`;
  `SyncBoxProcessor` drops `Arc<Mutex<..>>` (field becomes the plain `BoxProcessor`);
  `clone_inner()` becomes a lock-free virtual `clone_box`. Public surface (`new`,
  `clone_inner`, `Clone`) unchanged — no API break.
- camel-processor: resequencer (`resequencer/mod.rs`) uses `tower::util::BoxCloneService`
  directly (import :18; `post_continuation` params/fields :142/:162; constructions
  :540-541, :595-596, :635-636, :662-663) — swaps those usages to the `BoxProcessor`
  alias (mechanical; compiler-directed). `error_handler.rs` uses `clone_inner()` unchanged.
- camel-core: `route_compiler.rs:69-98` holds `unsafe impl Send/Sync for SharedSnapshot`
  whose SAFETY rationale rests on `BoxCloneService: !Sync` (ADR-0042), and the `run_steps`
  index-loop comment (~:415) rests on `CompiledStep: !Sync`. Post-swap these go stale:
  update the SAFETY docs, add an `assert_sync::<CompiledStep>()` compile-time guard, and
  drop the now-compiler-provable unsafe impls.
- Any other compile failure reveals a genuinely non-`Sync` service, which the blanket
  `Processor` bound (`Clone + Send + Sync + 'static`) already forbids.
- Tests: static `BoxProcessor: Sync` assertion, in-repo contention repro (64 tasks,
  wall-clock ceiling), hot-reload-under-load snapshot coherence (ADR-0004), clone-cost
  tripwire.

Per-exchange clone semantics are KEPT (ADR-0004 snapshot isolation; expert consultation
e_opus recorded in bd rc-vdy2).

Excluded: clone-frequency reduction (pooling/hoisting), shallow-clone pipeline refactor,
loadgen connection knob (rc-qv1x), allocator work (rc-vnm8).

## Acceptance criteria

- No `std::sync::Mutex` acquisition on the per-exchange hot path of a Concurrent route.
- `BoxProcessor: Send + Sync` asserted at compile time (regression lock).
- In-repo contention test: 64 concurrent tasks x N clone_inner() complete under a wall-clock
  ceiling the mutex path would exceed.
- Hot-reload-under-load: every in-flight exchange completes on a coherent snapshot while
  ArcSwap stores occur; Sequential topology still picks up the latest snapshot per envelope.
- Clone-cost tripwire: per-clone cost of a multi-step stack stays under a generous ceiling.
- Workspace gates green: fmt, clippy -D warnings, cargo test -p camel-api -p camel-core
  -p camel-processor, xtask lint-unwrap (net negative: removes a `lock().unwrap_or_else`).

## Risk budget

Acceptable: a boxed service failing the new `Sync` bound at compile time (would be a latent
bug the swap exposes — fix at source). Out of bounds: behavior changes to hot-reload
semantics, drain policy (ADR-0022), Sequential topology freshness, or any public API shape.
