# Tasks: fix-pipeline-syncbox-mutex-convoy

## camel-api

### Task 1.1: Swap BoxProcessor to BoxCloneSyncService and make SyncBoxProcessor lock-free

**Files:**
- `crates/camel-api/src/processor.rs` (modified)

**Steps:**
1. Change the type alias at `crates/camel-api/src/processor.rs:47` from
   `pub type BoxProcessor = tower::util::BoxCloneService<Exchange, Exchange, CamelError>;`
   to
   `pub type BoxProcessor = tower::util::BoxCloneSyncService<Exchange, Exchange, CamelError>;`.
2. Rewrite `SyncBoxProcessor` (lines 69-92): drop the `Arc<Mutex<BoxProcessor>>` field; new
   field is the plain `BoxProcessor`. `new(processor: BoxProcessor) -> Self` becomes
   `Self(processor)`; `clone_inner(&self) -> BoxProcessor` becomes `self.0.clone()`
   (lock-free virtual `clone_box`); `Clone` impl becomes `Self(self.0.clone())`.
3. Update the doc comments on `SyncBoxProcessor` (lines 69-75) and on `BoxProcessor`
   (lines 44-47): delete the "not Sync / Arc<Mutex> wrapper" rationale; state that the
   erased type is `Send + Sync` and cloning is a lock-free virtual `clone_box()` call;
   keep the vestigial-wrapper note (collapsing the newtype is a separate follow-up, not
   this change).
4. Replace `tower::util::BoxCloneService::new` constructor calls in this file's own
   unit tests (lines 245 and 255) with `BoxProcessor::new`, and their annotations
   where applicable — post-swap the non-Sync `BoxCloneService` value cannot feed the
   Sync-requiring `BoxProcessor` alias (compiler-directed, one-line each).
5. Remove the now-unused `use std::sync::Mutex;` import (keep `Arc` if still referenced
   elsewhere in the file; otherwise remove it too). Verify `BoxProcessorExt::from_fn` and
   `ProcessorFn` (holds `Arc<F>`, already `Send + Sync`) compile unchanged.
6. Add a compile-time regression lock next to the `SyncBoxProcessor` definition:
   `const _: () = { fn is_sync<T: Sync>() {} fn _check() { is_sync::<BoxProcessor>(); } };`
   (mirrors the existing `assert_send` guard pattern in
   `crates/camel-core/src/lifecycle/adapters/route_compiler.rs:104-106`).

**Tests:** (executable spec — name, arrange, act, assert)
- `syncbox_processor_is_send_sync_static`: static const-assert block (step 5) → `cargo build` → compiles only if `BoxProcessor: Send + Sync`; no runtime body needed.
- `clone_inner_returns_independent_processor`: existing unit tests in `crates/camel-api/src/processor.rs` (`test_box_processor_from_identity`, `test_box_processor_from_processor_fn`, `test_box_processor_ext_from_fn`) still pass after the step-4 constructor swap → `cargo test -p camel-api --lib` → all green (proves behavior preservation of the alias swap).
- `identity_processor_still_satisfies_processor_blanket`: no new test needed; the blanket `Processor` bound is exercised by the rest of the workspace build (task 1.3) — expected pass.

**Acceptance:**
- `rg -n "std::sync::Mutex|Arc<Mutex" crates/camel-api/src/processor.rs` returns no hits.
- `cargo build -p camel-api` exits 0.
- `cargo test -p camel-api --lib` passes.
- `cargo fmt --check` and `cargo clippy -p camel-api -- -D warnings` exit 0.

- [x] 1.1

## camel-processor

### Task 1.2: Resequencer mechanical swap to the BoxProcessor alias

**Files:**
- `crates/camel-processor/src/resequencer/mod.rs` (modified)
- `crates/camel-processor/src/wire_tap.rs` (modified)

**Steps:**
1. Replace the direct import at `crates/camel-processor/src/resequencer/mod.rs:18`
   (`use tower::util::BoxCloneService;`) with `use camel_api::BoxProcessor;`.
2. Replace every `BoxCloneService<Exchange, Exchange, CamelError>` type annotation with the
   `BoxProcessor` alias at lines 142, 162, 540, 595, 635, 662 (per blessed design: params,
   fields, and local constructions).
3. Replace each `BoxCloneService::new(capture)` / `BoxCloneService::new(CapturePost { tx })`
   constructor call (lines 540-541, 595-596, 635-636, 662-663) with the same expression
   on `BoxProcessor::new` — the constructor exists on `BoxCloneSyncService` with identical
   `Clone + Send + Sync + 'static` bounds.
4. No behavioral change: `post_continuation` handling, actor task wiring, and
   `sync_post` (`SyncBoxProcessor::new(post_continuation)`) semantics are untouched.
5. In `crates/camel-processor/src/wire_tap.rs` test module, replace the three
   `tower::util::BoxCloneService::new` constructions annotated as
   `camel_api::BoxProcessor` (lines 967, 1031, 1233) with `camel_api::BoxProcessor::new`
   — same compiler-directed break as task 1.1 step 4 (the cfg(test) tree compiles the
   whole crate, so these break this task's test gate if left).

**Tests:** (executable spec — name, arrange, act, assert)
- `resequencer_existing_suite_still_passes`: existing resequencer tests in `crates/camel-processor/src/resequencer/mod.rs` (the `#[cfg(test)]` module) and `crates/camel-processor/src/resequencer/` → `cargo test -p camel-processor resequencer` → all green, unchanged behavior post-swap.
- `resequencer_no_direct_boxcloneservice_refs_left`: arrange repo state after step 2 → run `rg -n "BoxCloneService" crates/camel-processor/src/resequencer/mod.rs` → zero hits.
- `wire_tap_suite_still_passes`: `cargo test -p camel-processor wire_tap` → all green after the step-5 swap.

**Acceptance:**
- `rg -n "BoxCloneService" crates/camel-processor/src/resequencer/mod.rs` returns no hits; `rg -n "BoxCloneService" crates/camel-processor/src/wire_tap.rs` returns at most the :118 prose comment (owned by task 1.4 step 5).
- `cargo test -p camel-processor resequencer` passes.
- `cargo fmt --check` and `cargo clippy -p camel-processor -- -D warnings` exit 0.

- [x] 1.2

## camel-core

### Task 1.3: SharedSnapshot SAFETY refresh + concurrency tests

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)
- `crates/camel-core/tests/arc_snapshot_concurrency.rs` (modified)
- `crates/camel-api/src/outcome_pipeline.rs` (modified — amended: Sync cause missed in design)
- `crates/components/camel-direct/src/lib.rs` (modified — amended)
- `crates/components/camel-opensearch/src/producer/mod.rs` (modified — amended)

**Steps:**
1. In `route_compiler.rs` (lines 69-98): delete `unsafe impl Send for SharedSnapshot {}`
   and `unsafe impl Sync for SharedSnapshot {}`. Rewrite the `SharedSnapshot` doc comment:
   the `!Sync` rationale is gone — state that `CompiledStep` is now `Send + Sync` by
   construction (`BoxProcessor = BoxCloneSyncService`) and the snapshot is shareable
   without unsafe impls.
2. Extend the compile-time guard block (lines 104-107): alongside the existing
   `assert_send::<CompiledStep>()`, add `fn assert_sync<T: Sync>() {}` and
   `assert_sync::<CompiledStep>()` inside the same existing `const _` guard block.
3. Fix the stale `run_steps` index-loop comment (~line 413): the loop shape is kept, but
   the rationale "`CompiledStep: !Sync` so `&[CompiledStep]: !Send`" no longer holds;
   rewrite the comment to reflect the modern rationale (index loop retained to avoid
   borrow-across-await even though `CompiledStep: Sync` now; no behavior change).
4. In `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (in-crate, so
   `pub(crate) PipelineAssembly` is directly accessible — this is the primary location for
   the ArcSwap-based tests; the existing
   `syncbox_processor_concurrent_clone_inner_via_arcswap` at :1456-1476 is the skeleton),
   add three async `#[tokio::test]` tests:
   a. `lockfree_pipeline_acquisition_per_clone_latency_ceiling`: build a
      `SyncBoxProcessor` wrapping a multi-step composed stack (at least 4
      `IdentityProcessor` layers composed via `tower::ServiceBuilder`); store it in an
      `Arc<ArcSwap<PipelineAssembly>>`. Spawn 64 tokio tasks, each doing 200 iterations
      of `load().processor.clone_inner()` in a tight loop (no sleep — the mutex, when it
      existed, was held only for the µs-scale clone, so the discriminating signal is
      per-clone latency, not hold-under-lock), recording each clone's `Instant` duration
      locally; collect durations via JoinHandle returns. Assert the p99 (or max) clone
      duration across all 12,800 clones is < 10 ms — under the old mutex a 64-way futex
      convoy inflates exactly per-clone latency well past this; lock-free `clone_box`
      stays in the µs range with a 1000x margin.
   b. `pipeline_swap_during_concurrent_acquisition_is_coherent`: same shared ArcSwap
      setup; spawn 16 acquisition tasks looping `clone_inner()` + `oneshot(exchange)`
      through the cloned pipeline; concurrently spawn one writer task that stores 20 new
      `PipelineAssembly` snapshots (each a different composed stack); assert all 16 x 20
      acquisitions complete without error and every cloned pipeline processes an exchange
      successfully (`Ok(exchange)` with body intact).
   c. `multi_step_pipeline_clone_cost_tripwire`: build the same 4-layer stack; time 3
      runs of 1000 `clone_inner()` iterations with `std::time::Instant`; take the MINIMUM
      of the three run means (preemption-resistant: one OS stall cannot flake it); assert
      min-of-means < 50 µs (generous tripwire: expected is sub-µs O(depth) pointer/Arc
      clones; the ceiling catches accidental deep-copy reintroduction).
5. In `crates/camel-core/tests/arc_snapshot_concurrency.rs` (integration file, wraps
   `SyncBoxProcessor` directly — no `PipelineAssembly` needed there): extend
   `in_flight_call_completes_on_old_snapshot_after_swap` (:168) only if needed — its
   existing coverage (ADR-0004 isolation with the direct `SyncBoxProcessor` type) must
   stay green unchanged; no new test required in this file.
6. In `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs`, replace the
   four `tower::util::BoxCloneService::new` test constructions (lines 1866, 1986,
   2107, 2133) with `camel_api::BoxProcessor::new` — same compiler-directed break
   class as tasks 1.1 step 4 and 1.2 step 5.
7. Run the full camel-core test surface to confirm the unsafe-impl removal did not break
   Send/Sync inference anywhere (SequentialPipeline/TracedPipeline futures, run_steps).
8. (Amended) `crates/camel-api/src/outcome_pipeline.rs:24`:
   `pub trait OutcomePipeline: Send + Sync + 'static` + one-line doc note ("Sync required
   so `CompiledStep::Segment` is `Sync` by construction; all in-tree implementors already
   satisfy it"). `CompiledStep::Segment → OutcomeSegment → Box<dyn OutcomePipeline>` is
   `!Sync` until the trait requires Sync — second cause design.md §Change 2 under-enumerated;
   design's "fix at its source" clause governs.
9. (Amended) `crates/components/camel-direct/src/lib.rs:37` and
   `crates/components/camel-opensearch/src/producer/mod.rs:61`: add `+ Sync` to the
   `AcquirePermitFut` box (`Pin<Box<dyn Future<...> + Send + Sync>>`); the underlying
   `acquire_owned` future is Sync (holds `Arc<Semaphore>`; tokio Semaphore is Send + Sync).
   These are camel-core dev-deps — their services must be Sync for camel-core `--lib` to build.

**Tests:** (executable spec — name, arrange, act, assert)
- `lockfree_pipeline_acquisition_per_clone_latency_ceiling`: 64 tasks x 200 tight-loop clone_inner on shared ArcSwap → per-clone durations collected → p99 (or max) < 10 ms.
- `pipeline_swap_during_concurrent_acquisition_is_coherent`: 16 acquirer tasks + 1 writer storing 20 snapshots → all oneshot exchanges Ok, no torn state, no error.
- `multi_step_pipeline_clone_cost_tripwire`: 4-layer stack → 3 runs x 1000 clone_inner → min of run means < 50µs.
- `shared_snapshot_remains_send_sync_after_unsafe_removal`: the extended const-assert block (step 2) → `cargo build -p camel-core` → compiles (statically proves `CompiledStep: Send + Sync` without the unsafe impls).
- `sequential_topology_snapshot_freshness_per_envelope`: preserved by existing suite — `swap_pipeline_and_remove_route_behaviors` (route_controller_tests.rs:246) proves a swapped pipeline is picked up by subsequent processing; run `cargo test -p camel-core --lib route_controller_tests` and confirm green (no regression from this change).
- `arc_snapshot_isolation_suite_still_green`: `cargo test -p camel-core --test arc_snapshot_concurrency` → existing tests at :35 and :168 pass unchanged.

**Acceptance:**
- `rg -n "unsafe impl" crates/camel-core/src/lifecycle/adapters/route_compiler.rs` returns no hits.
- `rg -n "BoxCloneService" crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` returns no hits.
- `cargo test -p camel-core --test arc_snapshot_concurrency` passes unchanged.
- `cargo test -p camel-core --lib route_controller::tests` passes including the three new tests (file is mounted as `mod tests` via `#[path]`; filter `route_controller_tests` matches 0 tests).
- `cargo fmt --check` and `cargo clippy -p camel-core -- -D warnings` exit 0.
- (Amended) `rg -n "unsafe impl" crates/camel-api/src/outcome_pipeline.rs` returns no hits; `cargo test -p camel-component-direct -p camel-opensearch` green.

- [x] 1.3

## Verification

### Task 1.4: Workspace gates + Context documentation

**Files:**
- `crates/camel-api/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `docs/adr/0042-arc-compiled-steps-snapshot.md` (modified)
- `crates/camel-core/tests/hexagonal_architecture_boundaries_test.rs` (modified)
- `crates/camel-builder/src/lib.rs` (modified)
- `crates/camel-processor/src/content_enricher.rs` (modified)
- `crates/camel-builder/CONTEXT.md` (modified)
- `crates/camel-core/CONTEXT.md` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_tests.rs` (modified — amended: stale unsafe-impl doc reference at :1309-1315)
- `crates/components/camel-component-grpc/src/producer/mod.rs` (modified — amended: acquire-box + Sync)
- `crates/components/camel-component-wasm/src/producer.rs` (modified — amended: acquire-box + Sync + stale comment refresh :38, :194)
- `crates/components/camel-cxf/src/producer.rs` (modified — amended: acquire-box + Sync)
- `crates/components/camel-jms/src/producer.rs` (modified — amended: acquire-box + Sync)
- `crates/components/camel-kafka/src/producer.rs` (modified — amended: acquire-box + Sync)

**Steps:**
1. Run workspace-wide verification (conductor gates minus lint-commits, plus conductor
   additions): `cargo build --workspace`, `cargo test --workspace --lib`,
   `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`,
   `cargo fmt --check --all`, clippy per AGENTS.md split (workspace `--all-features
   --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak
   --exclude security-wasm-policy -- -D warnings`, then `-p camel-component-kafka
   --all-targets -- -D warnings`, then `-p camel-cli -- -D warnings`), `cargo xtask
   lint-unwrap`, `cargo xtask lint-secrets`, `cargo xtask lint-non-exhaustive`,
   `cargo xtask lint-log-levels`, `cargo xtask lint-ignore`, `cargo xtask
   lint-publish-cycles`, `cargo xtask lint-component-deps`, `cargo xtask
   lint-context-citations`, `cargo xtask schema --check`.
2. Update `crates/camel-api/CONTEXT.md`: the `BoxProcessor` entry — replace
   "SyncBoxProcessor wraps it for `Sync` contexts" with the new reality
   (`BoxCloneSyncService`, `Send + Sync` by construction, lock-free `clone_box()`;
   SyncBoxProcessor remains as a vestigial newtype pending separate collapse). Keep the
   _Avoid_ list as-is.
3. Update `CONTEXT-MAP.md` only if the BoxProcessor/SyncBoxProcessor glossary mentions
   the Mutex wrapper (check with `rg -n "SyncBoxProcessor|Mutex" CONTEXT-MAP.md`;
   update the sentence, keep the rest).
4. Amend `docs/adr/0042-arc-compiled-steps-snapshot.md`: add a dated amendment note
   stating the `unsafe impl Send/Sync for SharedSnapshot` no longer exists —
   `BoxProcessor` is now `BoxCloneSyncService` (`Send + Sync` by construction), so the
   snapshot shares without unsafe impls; keep the original decision text as history.
5. Refresh stale comments that cite the old type's `!Sync` / clone semantics:
   `crates/camel-core/tests/hexagonal_architecture_boundaries_test.rs:323`
   (`BoxCloneService` type name in the comment — update to the new alias),
   `crates/camel-builder/src/lib.rs:368` (`Arc`/`BoxCloneService` deep-copy comment —
   update type name), `crates/camel-processor/src/content_enricher.rs:40`
   ("BoxProcessor is BoxCloneService — clone for the async block" — update type name),
   `crates/camel-processor/src/wire_tap.rs:118` (comment mentioning `BoxCloneService` —
   update type name). Comment-only edits; no code change.
6. Sweep remaining prose mentions so the final `rg -rn "BoxCloneService" crates/`
   acceptance passes: `crates/camel-builder/CONTEXT.md:115` and
   `crates/camel-core/CONTEXT.md:106` — update the type-name references to the new
   alias (prose-only edits; keep surrounding text intact).
7. (Amended) Refresh the stale unsafe-impl doc reference in
   `crates/camel-core/src/lifecycle/adapters/route_compiler_tests.rs:1309-1315` —
   the doc still cites the removed `unsafe impl Send/Sync for SharedSnapshot`;
   rewrite to reflect compiler-proven Send + Sync.
8. (Amended — extension of the task 1.3 amendment class) The workspace gates compile
   five further component producers whose semaphore acquire-future boxes
   (`Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>`)
   must be `+ Sync` for their producers to stay Sync under the new alias:
   camel-component-grpc/producer/mod.rs, camel-component-wasm/producer.rs,
   camel-cxf/producer.rs, camel-jms/producer.rs, camel-kafka/producer.rs.
   One-line `+ Sync` each; underlying futures are `acquire_owned` (genuinely Sync).
   camel-component-wasm additionally refreshes stale comments at :38 and :194
   (inverted `!Sync` premise). NOTE: `cargo build --workspace` gate dropped by human
   decision (test/clippy gates compile the same code).

**Tests:** (executable spec — name, arrange, act, assert)
- `workspace_gates_green`: all commands in step 1 → all exit 0.
- `context_docs_consistent`: `rg -n "Arc<Mutex|Mutex wrapper" crates/camel-api/CONTEXT.md CONTEXT-MAP.md` → zero hits post-edit.
- `stale_type_refs_swept`: `rg -rn "BoxCloneService" crates/ docs/adr/0042-arc-compiled-steps-snapshot.md` (post-change) → only historical ADR-decision-text mentions and zero live-code mentions outside ADR history.

**Acceptance:**
- Every gate command in step 1 exits 0.
- `cargo xtask lint-context-citations` exits 0 (validates the CONTEXT.md edit).
- `rg -n "Arc<Mutex|Mutex wrapper" crates/camel-api/CONTEXT.md CONTEXT-MAP.md` returns no hits.
- `docs/adr/0042-arc-compiled-steps-snapshot.md` carries a dated amendment; live code under `crates/` has zero `BoxCloneService` mentions (verify `rg -rn "BoxCloneService" crates/` → no hits).

- [x] 1.4
