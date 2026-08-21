# Design: fix-pipeline-syncbox-mutex-convoy

## Approach

The route pipeline is stored per-route as `Arc<ArcSwap<PipelineAssembly>>` (hot reload via
atomic snapshot swap, ADR-0004). `PipelineAssembly.processor` is a `SyncBoxProcessor`, which
today wraps `Arc<std::sync::Mutex<BoxProcessor>>` solely because
`BoxProcessor = tower::util::BoxCloneService` is `Send` but NOT `Sync` (its erased inner
trait object lacks a `Sync` bound), while `PipelineAssembly` must be `Sync` to live in an
`ArcSwap`. Every exchange calls `clone_inner()` (Concurrent topology: once per spawned task;
Sequential: once per envelope) — so the whole route serializes on that mutex. Measured as a
futex convoy under load (bd rc-vdy2).

Tower 0.5 (the workspace-wide version, `Cargo.toml:143`; only tower-lsp drags a 0.4
transitively) ships `tower::util::BoxCloneSyncService` — same erasure, inner trait bounded
`+ Send + Sync`, `clone()` = one virtual `clone_box()` call. The blanket `Processor` trait
already requires `Clone + Send + Sync + 'static` on every concrete service, so every boxing
site in the workspace already satisfies the bound; the `Sync` loss was purely an artifact of
the erasure type choice.

Change:

1. `camel-api/src/processor.rs`
   - `pub type BoxProcessor = tower::util::BoxCloneSyncService<Exchange, Exchange, CamelError>;`
   - `pub struct SyncBoxProcessor(BoxProcessor);`
     - `new(processor)` = `Self(processor)`
     - `clone_inner(&self) -> BoxProcessor` = `self.0.clone()` (lock-free)
     - `Clone` impl = `Self(self.0.clone())`
   - Update doc comments (drop the "not Sync / Mutex" rationale; note the type is now
     arguably vestigial — collapsing it is a separate mechanical follow-up, NOT this change).
   - Keep `BoxProcessorExt::from_fn` / `ProcessorFn` (holds `Arc<F>`, already `Send + Sync`).
2. Call sites (`route_controller_trait.rs:249/:296`, `route_controller.rs:991-995`
   hot-reload re-wrap, `error_handler.rs`) compile unchanged — signatures are stable.
   Two compiler-directed touch-ups:
   - camel-processor resequencer (`crates/camel-processor/src/resequencer/mod.rs`) uses
     `tower::util::BoxCloneService` directly (import :18; params/fields :142/:162;
     constructions :540-541, :595-596, :635-636, :662-663) — swap those to the
     `BoxProcessor` alias (mechanical).
   - camel-core `route_compiler.rs:69-98`: `unsafe impl Send/Sync for SharedSnapshot`
     and its SAFETY docs rest on `BoxCloneService: !Sync` (ADR-0042); the `run_steps`
     index-loop comment (~:415) rests on `CompiledStep: !Sync`. Post-swap `CompiledStep`
     is `Send + Sync` by construction: update the SAFETY docs, add
     `assert_sync::<CompiledStep>()` next to the existing `assert_send` guard, and drop
     the now-compiler-provable unsafe impls (keep the loop shape; fix its comment).
   Any other compile failure reveals a genuinely non-`Sync` service (latent bug; fix at
   its source).
3. Tests (all in-repo, deterministic):
   - Static: `const fn` assert `BoxProcessor: Sync` in camel-api (regression lock).
   - Contention repro: extend `camel-core/tests/arc_snapshot_concurrency.rs` — 64 tasks x
     N iterations of `load().processor.clone_inner()` with sleep-injected hold work, assert
     wall-clock under a ceiling the serialized path would exceed.
   - Hot-reload-under-load: concurrent `clone_inner()` while a writer stores new
     `PipelineAssembly` snapshots; assert every clone yields a coherent pipeline and all
     exchanges complete (ADR-0004). Reuse the
     `syncbox_processor_concurrent_clone_inner_via_arcswap` test skeleton.
   - Clone-cost tripwire: timing assert on `clone_inner()` for a representative multi-step
     stack (generous ceiling — tripwire, not benchmark).

## Affected crates

- camel-api: `BoxProcessor` alias swap; `SyncBoxProcessor` internals; doc comments; static
  Sync assertion; unit tests unchanged in behavior.
- camel-core: `route_compiler.rs` — SharedSnapshot SAFETY-doc refresh,
  `assert_sync::<CompiledStep>()` guard, unsafe-impl removal (now compiler-provable);
  no other production-source change expected. New integration tests
  (arc_snapshot_concurrency.rs extension).
- camel-processor: resequencer `mod.rs` — mechanical swap of direct
  `BoxCloneService` usages to the `BoxProcessor` alias; `error_handler.rs` unchanged;
  covered by existing test suites.

## Architecture boundaries

Data-plane-only fix inside the Runtime's pipeline execution. The control plane (RuntimeBus,
lifecycle commands), DSL, Components, Services, Languages, Functions are untouched.
Semantics preserved: ADR-0004 atomic snapshot swap (in-flight exchanges run to completion on
their snapshot; per-exchange clone keeps that true), ADR-0022 StepLifecycle drain (lives in
`PipelineAssembly.lifecycle`, orthogonal to the processor field), Sequential-topology
hot-reload freshness (clone-per-envelope retained — hoisting would pin stale pipelines).

## Alternatives considered

- Clone-frequency reduction (pool/hoist, tower ServicePool-style): rejected — in the
  Concurrent topology "per task" == "per exchange", so pooling buys nothing without
  redesigning the spawn model; hoisting the Sequential clone trades hot-reload freshness for
  a lock that no longer exists. (Recorded in bd rc-vdy2, e_opus consultation.)
- Custom hand-rolled `Send + Sync` erased box: rejected — tower ships the canonical type;
  upstream beats hand-rolled.
- Shallow-clone pipeline representation (Arc-per-step): rejected as first move —
  `clone_box` is O(depth) pointer/Arc clones, not O(work); escalate only if the clone-cost
  tripwire regresses post-fix.
- ArcSwap inside `SyncBoxProcessor`: rejected — ArcSwap already sits one level up
  (`SharedPipeline`); the constraint was the non-`Sync` box type, not swap mechanics.
