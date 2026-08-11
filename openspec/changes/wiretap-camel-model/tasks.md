# Tasks: wiretap-camel-model

## camel-core

### Task 1: Add `CompositeStepLifecycle` combinator in camel-core

**Files:**
- `crates/camel-core/src/lifecycle/adapters/composite_step_lifecycle.rs` (new)
- `crates/camel-core/src/lifecycle/adapters/mod.rs` (modified — declare `pub(crate) mod composite_step_lifecycle;` and re-export `CompositeStepLifecycle`)

**Steps:**
1. Create `composite_step_lifecycle.rs`. Define a `#[derive(Debug)] pub(crate) struct CompositeStepLifecycle { children: Vec<Arc<dyn camel_api::StepLifecycle>> }` storing child handles in the ORDER they are passed (callers pass `[endpoint, wiretap]`).
2. Provide a constructor `pub(crate) fn new(children: Vec<Arc<dyn camel_api::StepLifecycle>>) -> Self` that asserts `!children.is_empty()` (panic with a fail-closed message — an empty composite is a programmer error).
3. Implement `#[async_trait] impl StepLifecycle for CompositeStepLifecycle`:
   - `fn name(&self) -> &'static str` returns `"composite"`.
   - `async fn start(&self)`: run `start()` on children in FORWARD order (index 0..n). If a child returns `Err`, roll back: call `shutdown(StepShutdownReason::RouteStop)` on the already-started children in REVERSE order (only those whose `start` returned `Ok`), then return the error.
   - `async fn shutdown(&self, reason)`: run `shutdown(reason)` on children in REVERSE order (index n-1..0). This is BEST-EFFORT: call every child even if an earlier one errors. Collect ALL errors into a `Vec<CamelError>` (do not stop on the first). If the vec is non-empty, return a single `Err(CamelError::ProcessorError(..))` whose message lists every child name + error (aggregation preserves all failures, per spec lines requiring aggregation). Test that multiple child failures all surface.
4. Register the module in `mod.rs`: add `pub(crate) mod composite_step_lifecycle;` and `pub(crate) use composite_step_lifecycle::CompositeStepLifecycle;`.

**Tests:** (in the same file under `#[cfg(test)] mod tests`)
- `test_composite_start_runs_forward`: setup two `FakeStep` fakes (from a test helper struct recording call order into a shared `Arc<Mutex<Vec<&'static str>>>`); act `CompositeStepLifecycle::new(vec![a, b]).start()`; assert the recorded order is `[a.name(), b.name()]`.
- `test_composite_shutdown_runs_reverse`: same setup; act `shutdown(RouteStop)`; assert recorded order is `[b.name(), a.name()]`.
- `test_composite_start_failure_rollbacks_started`: setup 3 fakes where child index 1 `start()` returns `Err`; act `start()`; assert child 0 (started ok) gets `shutdown` called (rollback), child 2 never starts, and the composite returns `Err`.
- `test_composite_shutdown_best_effort_all_called`: setup 3 fakes where child 1 `shutdown()` returns `Err`; act `shutdown(RouteStop)`; assert ALL three children had `shutdown` called, and the composite returns `Err`.
- `test_composite_shutdown_aggregates_multiple_errors`: setup 3 fakes where children 0 AND 2 both `shutdown()` return `Err` with distinct messages; act `shutdown(RouteStop)`; assert the returned `Err` message contains BOTH error messages (not just the first), proving aggregation.

**Acceptance:**
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-core --lib composite_step_lifecycle` passes all four tests.
- No new `unwrap()` introduced (verify `cargo xtask lint-unwrap` shows no new sites in this file).

- [x] 1

## camel-processor

### Task 2: Evolve `WireTapConfig` — add `shutdown_grace`, default bound 20, validation

**Files:**
- `crates/camel-processor/src/wire_tap.rs` (modified — `WireTapConfig` struct + `Default` + `bounded`)

**Steps:**
1. Add a field `pub shutdown_grace: std::time::Duration` to `WireTapConfig`. Update the doc comment on `WireTapConfig` to state that the default concurrency bound is now 20 (Camel-faithful flat-semaphore) and `shutdown_grace` defaults to 5 seconds. **Remove `Default` from the derive list** (the struct currently is `#[derive(Clone, Default)]`; change to `#[derive(Clone)]`) — a manual `impl Default` is added in step 2, so keeping the derived one would be E0119 (conflicting implementations).
2. Add a manual `impl Default for WireTapConfig` returning `Self { max_concurrent: Some(20), shutdown_grace: std::time::Duration::from_secs(5) }` (the bound is no longer `None`/unlimited — this is the deliberate Camel-faithful behavior change recorded in the proposal risk budget).
3. Update `WireTapConfig::bounded(max_concurrent: usize)` to assert `max_concurrent > 0` (it already does) and to set `shutdown_grace: Duration::from_secs(5)`.
4. Add a public method `pub fn validate(&self)` on `WireTapConfig` that asserts `max_concurrent` is not `Some(0)`: if `self.max_concurrent == Some(0)`, panic with `"max_concurrent must be > 0 when set"`. (`None` is no longer reachable via `Default` but a struct literal could still set `Some(0)`; `WireTapService::with_config` in Task 3 calls this.) Also document that `shutdown_grace` of zero is valid (means "skip drain, cancel immediately" — handled in Task 4).

**Tests:**
- `test_wiretap_config_default_is_bounded_20`: setup none; act `WireTapConfig::default()`; assert `max_concurrent == Some(20)` and `shutdown_grace == Duration::from_secs(5)`.
- `test_wiretap_config_bounded_zero_panics`: setup none; act `std::panic::catch_unwind(|| WireTapConfig::bounded(0))`; assert it panicked with a message containing "max_concurrent".
- `test_wiretap_config_validate_rejects_zero_bound`: setup a literal `WireTapConfig { max_concurrent: Some(0), shutdown_grace: Duration::from_secs(5) }`; act `std::panic::catch_unwind(|| cfg.validate())`; assert it panicked with a message containing "max_concurrent".

**Acceptance:**
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `cargo test -p camel-processor --lib wire_tap::tests::test_wiretap_config` passes all three.
- `WireTapConfig` derives `Clone` (not `Default`); the manual `impl Default` coexists without E0119.

- [x] 2

### Task 3: Rewrite `WireTapService` data-plane — admission-before-spawn, detached permits, TaskTracker, cancellation

**Files:**
- `crates/camel-processor/src/wire_tap.rs` (modified — `WireTapService` struct, `Clone`, `new`, `with_config`, `poll_ready`, `call`; remove `in_flight: Arc<Mutex<JoinSet<()>>>`)

**Steps:**
1. Add `tokio_util::task::TaskTracker` and `tokio_util::sync::CancellationToken` to the imports (the crate already depends on `tokio-util` workspace with the `rt` feature that provides `TaskTracker`).
2. Define a private shared-state struct `#[derive(Debug)] struct WireTapShared { inner: Mutex<WireTapSharedInner> }` where `WireTapSharedInner { open: bool, tracker: TaskTracker, cancel: CancellationToken, semaphore: Option<Arc<Semaphore>>, shutdown_grace: Duration }`. Note: `tokio_util::task::TaskTracker` is NON-generic (`pub struct TaskTracker`, not `TaskTracker<()>`). The `Mutex` spans "check open → register task" so shutdown cannot close the tracker while a `call()` is mid-registration (the admission-race fix). Wrap it as `Arc<WireTapShared>`.
3. Change `WireTapService` to hold `tap_endpoint: camel_api::BoxProcessor` and `shared: Arc<WireTapShared>`. Update `impl Clone for WireTapService` to clone `tap_endpoint` and `Arc::clone(&self.shared)` (the shared state — admission gate, tracker, token — is shared across clones; per-request clone drop does NOT abort or close anything).
4. `WireTapService::new(tap_endpoint)` keeps its signature: builds `WireTapConfig::default()` and delegates to `with_config`. `WireTapService::with_config(tap_endpoint, config)` keeps its signature: calls `config.validate()` (panics on `Some(0)`), constructs `WireTapShared` with `open: true`, `tracker: TaskTracker::new()`, `cancel: CancellationToken::new()`, `semaphore: Some(Arc::new(Semaphore::new(n)))` when `max_concurrent` is `Some(n)` (or `None` for an explicit unbounded literal — note `Default` now sets `Some(20)`), `shutdown_grace: config.shutdown_grace`.
5. Change `poll_ready` to always return `Poll::Ready(Ok(()))` (WireTap main route never blocks on tap readiness — ADR-0019; this resolves the `pending-fix` row in CONTEXT.md).
6. Rewrite `call(&mut self, exchange)`:
   - Clone `tap_endpoint` and `exchange` for the tap.
   - Lock `shared.inner`. If `!open`, log `warn!("WireTap admission closed, dropping tap")` and return `Ok(exchange)` immediately (calls-after-shutdown are rejected). Release the lock only after the task is registered (see below) — hold it across the admit-or-inline decision.
   - BOUNDED case (`Some(sem)`): non-blockingly acquire an OWNED permit via `Arc::clone(&sem).try_acquire_owned()` (tokio's `Arc<Semaphore>::try_acquire_owned` returns `Result<OwnedSemaphorePermit, TryAcquireError>`). On `Ok(permit)`: register a detached task via `tracker.spawn(async move { let _permit = permit; run_tap(tap_endpoint_clone, tap_exchange_clone, cancel_clone).await; })` — the `OwnedSemaphorePermit` is MOVED into the spawned task and lives for the task's lifetime, releasing on completion (this is the leak-free admission gate). Drop the lock, return `Box::pin(async move { Ok(exchange) })`. On `Err(TryAcquireError::NoPermits)` (saturated): run the tap INLINE (see CallerRuns below). On `Err(TryAcquireError::Closed)` (semaphore closed): log `warn!("WireTap semaphore closed, dropping tap")` and return `Ok(exchange)`.
   - UNBOUNDED case (`None`): register a detached task via `tracker.spawn(async move { run_tap(tap_endpoint_clone, tap_exchange_clone, cancel_clone).await; })` (no permit to manage), drop the lock, return `Ok(exchange)`.
   - CallerRuns inline path: run `run_tap(tap_endpoint_clone, tap_exchange_clone, cancel_clone)` directly on the calling future WITHOUT acquiring or holding a permit — so total concurrent execution transiently reaches `bound + 1` (the inline task), matching the spec invariant. Return `Box::pin(async move { run_tap(tap_endpoint_clone, tap_exchange_clone, cancel_clone).await; Ok(exchange) })` — the caller is back-pressured until the inline tap finishes. The inline path selects on `cancel` via `run_tap` exactly like the detached path.
   - The detached task body and the inline body share a single private async helper `async fn run_tap(tap_endpoint: camel_api::BoxProcessor, tap_exchange: Exchange, cancel: CancellationToken)`. The helper runs `select!`: one branch drives the tap endpoint to readiness then calls it (logging `warn!` on any `Err` from readiness or `call`, then returning); the other branch awaits `cancel.cancelled()` (returning immediately on cancellation so shutdown aborts promptly). Both the detached path and the inline path call this helper so cancellation coverage is uniform.
   - Tap readiness and processing errors are logged at `warn!` (category handler-owned per ADR-0012) and suppressed — the main exchange proceeds unchanged.
7. Remove the `JoinSet` field and its `Mutex<JoinSet<()>>` import. The `in_flight` accumulation path is gone.
8. Add `impl Drop for WireTapShared` that calls `self.inner.lock().expect("WireTapShared mutex poisoned").cancel.cancel()` — this satisfies the spec's "last-reference drop of the canonical service SHALL abort" clause (when the last `Arc<WireTapShared>` ref drops, in-flight taps are cancelled). This is defense-in-depth alongside `StepLifecycle::shutdown`; the runtime calls `shutdown` before drop per ADR-0022, but Drop guarantees cleanup if it does not.
9. Explicitly handle the EXISTING `test_wire_tap_drop_aborts_spawned_tasks` test (line ~335 in the current file): KEEP it, retargeted to the new model. Under the new model the canonical `WireTapService` holds an `Arc<WireTapShared>`; dropping the only/canonical service drops the last `Arc` ref, triggering `WireTapShared::drop` which cancels the token, aborting the 10s tap. Confirm the test still asserts the tap does not complete after drop. (If the test constructs additional clones that keep the `Arc` alive, adjust it to drop ALL refs so the last-ref-drop fires — the test's intent is "canonical-service teardown aborts taps".)

**Tests:**
- `test_wiretap_bounded_detached_count_never_exceeds_bound`: setup a `WireTapService::with_config(.., WireTapConfig::bounded(2))` with a tap sleeping 50ms; expose a test-only accessor `pub(crate) fn in_flight_count(&self) -> usize { self.shared.inner.lock().expect("mutex").tracker.len() }` which reads the `TaskTracker::len()` (counts ONLY detached tracked tasks, not inline CallerRuns work — so it cannot observe the inline `bound+1`). Act: fire 5 `call()` futures from concurrent tasks (spawned callers) so the admission gate is exercised under contention. Assert: every sample of `in_flight_count()` taken during the run is `<= 2`.
- `test_wiretap_caller_backpressured_when_saturated`: setup `WireTapConfig::bounded(1)`, a tap that blocks on a `tokio::sync::Notify`, and one tap already holding the sole permit (spawned + awaiting the Notify). Act: build `let mut fut2 = Box::pin(svc.ready().await.unwrap().call(ex2));` then poll it WITHOUT notifying — use a non-consuming poll (e.g. `futures::poll!(&mut fut2)` inside a `tokio::time::timeout(Duration::from_millis(50), async { futures::poll!(&mut fut2) }).await`, or a `select!` that races `&mut fut2` against a 50ms sleep). Assert the second call is `Pending` after 50ms (it is running the tap inline, engaging `CallerRuns` back-pressure — the leaky spawn-then-acquire version would resolve immediately regardless of the Notify). Then `notify.notify_one().await` and await `fut2` to completion — assert it resolves `Ok`.
- `test_wiretap_unbounded_none_path_detaches_without_permit`: setup `WireTapService::with_config(.., WireTapConfig { max_concurrent: None, shutdown_grace: Duration::from_secs(5) })` and a fast tap. Act: fire 50 `call()`s; assert each returns `Ok` promptly and `in_flight_count()` rose above 0 then drained to 0 (tasks detached without permits, tracked). Confirms the explicit-unbounded path registers tracked tasks.
- `test_wiretap_no_unbounded_task_growth_across_bursts`: setup `WireTapConfig::default()` (bound 20), fast taps (no sleep). Act: fire a burst of 1000 `call()`s, poll until `in_flight_count() == 0` within a 2s timeout; fire a SECOND burst of 1000; poll again. Assert both bursts drain to 0 (the leaky JoinSet version's count is monotonically non-decreasing → second-burst poll times out).
- `test_wiretap_tap_readiness_error_suppressed_with_log`: setup a tap endpoint whose `poll_ready`/`ready()` returns `Err(CamelError::ProcessorError("ready-boom"))`. Act `svc.call(ex).await`. Assert result is `Ok`, original exchange returned. Additionally capture the `warn!` log (use the crate's existing tracing test capture, or `tracing_subscriber` test layer) and assert a `warn!` record mentioning the readiness error was emitted.
- `test_wiretap_tap_processing_error_suppressed_with_log`: setup a tap endpoint whose `call` returns `Err(CamelError::ProcessorError("call-boom"))`. Act `svc.call(ex).await`. Assert `Ok(ex)` returned AND a `warn!` record mentioning the processing error was emitted.
- `test_wiretap_poll_ready_always_ready`: setup any tap (even one whose readiness would fail). Act `svc.poll_ready(&mut cx)` (or `svc.ready().await`). Assert it is immediately `Ready(Ok(()))` regardless of tap endpoint state.
- `test_wiretap_admission_shutdown_no_orphan_task`: stress/iteration test for the admission-race invariant. The open-check and `tracker.spawn` happen under ONE `Mutex` acquisition, so they are atomic by construction (there is no await/yield point to inject a barrier between them — the lock serializes `call()` and `shutdown()`). The test therefore verifies the OUTCOME invariant across many randomized interleavings rather than one deterministic injection. setup `WireTapConfig::bounded(2)` with a tap sleeping 1-5ms (randomized per call). Act: in a loop of ~200 iterations, concurrently fire a handful of `call()`s from spawned callers AND invoke `lifecycle.shutdown(RouteStop)` on a separate spawned task (rebuild the service each iteration since shutdown closes admission), racing them. After each iteration's `shutdown` completes, assert `in_flight_count() == 0` (no task escaped the tracker — every admitted task was either completed-and-drained or cancelled-and-awaited). Across 200 iterations with randomized timing, a race that let a task register after `tracker.close()` would manifest as `in_flight_count() > 0` after shutdown at least once. Assert it is 0 every iteration.
- `test_wire_tap_survives_per_request_clone_drop` (EXISTING — keep, it is the rc-vq91 regression): confirm it still passes with the new shared-state model. No rewrite needed; just ensure it is not deleted and still exercises clone→call→drop ×3 with slow taps completing.

**Acceptance:**
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `cargo test -p camel-processor --lib wire_tap` passes ALL tests (existing + new).
- No `JoinSet` symbol remains in `wire_tap.rs` (`rg -n "JoinSet" crates/camel-processor/src/wire_tap.rs` returns nothing).
- `poll_ready` returns `Ready(Ok(()))` unconditionally.

- [x] 3

### Task 4: Add `WireTapLifecycle` handle + `lifecycle()` accessor + shutdown sequence

**Files:**
- `crates/camel-processor/src/wire_tap.rs` (modified — add `WireTapLifecycle` struct + impl + accessor)

**Steps:**
1. Add `use camel_api::{StepLifecycle, StepShutdownReason};` and `use async_trait::async_trait;` to imports. Confirm `async_trait` is a dependency of `camel-processor` (it is — used by other StepLifecycle impls).
2. Define `#[derive(Debug)] pub struct WireTapLifecycle { shared: Arc<WireTapShared>, shutdown_called: AtomicBool }`. It holds the SAME `Arc<WireTapShared>` the service uses, so `shutdown` observes the live admission gate and tracker.
3. Implement `#[async_trait] impl StepLifecycle for WireTapLifecycle`:
   - `fn name(&self) -> &'static str` returns `"wiretap"`.
   - `async fn shutdown(&self, _reason)`: if `shutdown_called.swap(true, Ordering::SeqCst)` was already true, return `Ok(())` immediately (idempotent). Otherwise: (a) lock `shared.inner`, set `open = false`, call `tracker.close()`, then CLONE the `TaskTracker` handle out of the lock (`let tracker = guard.tracker.clone();` — `TaskTracker` is `Clone`, a shared-counter handle) and clone the `cancel` token; capture `shutdown_grace`; drop the guard. (b) If `shutdown_grace` is zero, skip the drain wait. (c) `let _ = tokio::time::timeout(shutdown_grace, tracker.wait()).await;` — drain taps that complete naturally. NO `MutexGuard` is held across this `.await` (a `std::sync::MutexGuard` is `!Send` and would fail to compile across the `async_trait` Send-boxed future; the cloned `tracker` handle avoids this). (d) `cancel.cancel();` — tasks selecting on the token abort. (e) `let _ = tracker.wait().await;` — await remaining abort completions (bounded; same no-guard-held rule). Return `Ok(())` (best-effort per the trait contract; shutdown errors are logged by the runtime, not surfaced as route failures).
4. Add a method on `WireTapService`: `pub fn lifecycle(&self) -> Arc<dyn StepLifecycle> { Arc::new(WireTapLifecycle { shared: Arc::clone(&self.shared), shutdown_called: AtomicBool::new(false) }) }`. This is the additive accessor that preserves the stable constructor signatures — callers obtain the lifecycle handle separately.
5. Re-export `WireTapLifecycle` from `crates/camel-processor/src/lib.rs` alongside the existing wire-tap exports (`pub use wire_tap::{WireTapConfig, WireTapLayer, WireTapService, WireTapLifecycle};`).

**Tests:**
- `test_wiretap_shutdown_drains_fast_aborts_slow`: setup a `WireTapService` built from a test config with `shutdown_grace: Duration::from_millis(200)`, one fast tap (10ms) and one slow tap (10s) in flight via two `call()`s. Act `lifecycle.shutdown(RouteStop).await`. Assert: the fast tap's completion counter incremented (drained), the slow tap's completion counter did NOT increment (aborted after grace), and `shutdown` returned within ~400ms.
- `test_wiretap_shutdown_idempotent`: setup as above. Act `shutdown` twice. Assert the second call returns `Ok(())` promptly and the cancellation token was only cancelled once (the slow tap's abort counter increments only once).
- `test_wiretap_calls_after_close_rejected`: setup a service. Act `lifecycle.shutdown(RouteStop).await` first, then `svc.call(ex).await`. Assert the `call` returns `Ok(original exchange)` immediately (tap not spawned) — verify by asserting the tap's invocation counter is 0.
- `test_wiretap_cancellation_while_pending_readiness`: setup a tap endpoint whose `ready()` is pending (blocked on a Notify). Fire one `call()` (spawned detached). Act `lifecycle.shutdown(RouteStop).await`. Assert: no panic, the pending tap aborts cleanly, and the main route is unaffected.
- `test_wiretap_zero_grace_immediate_cancel`: setup `shutdown_grace: Duration::ZERO` and one slow tap. Act `shutdown(RouteStop).await`. Assert the slow tap is cancelled without the drain wait and `shutdown` returns within a small bound (~100ms, not the multi-second tap).

**Acceptance:**
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `cargo test -p camel-processor --lib wire_tap` passes all new lifecycle tests + existing.
- `WireTapLifecycle` is exported from `camel-processor` crate root.

- [x] 4

## camel-core

### Task 5: Wire the WireTap compiler arm to compose `[endpoint, WireTap]` lifecycle handles

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/endpoints.rs` (modified — the `BuilderStep::WireTap { uri }` arm, lines ~50-64)

**Steps:**
1. In the `BuilderStep::WireTap { uri }` arm: keep the existing `resolve_producer_with_lifecycle(ctx, &uri)?` call that yields `(producer, endpoint_lifecycle: Option<Arc<dyn StepLifecycle>>)`.
2. Build the service: `let svc = camel_processor::WireTapService::new(producer);`. Obtain the WireTap lifecycle handle via the additive accessor: `let wiretap_lifecycle: Arc<dyn camel_api::StepLifecycle> = svc.lifecycle();`.
3. Compose the two handles into a `CompositeStepLifecycle` in child order `[endpoint, wiretap]`: if `endpoint_lifecycle` is `Some(ep)`, build `Some(Arc::new(CompositeStepLifecycle::new(vec![ep, wiretap_lifecycle])) as Arc<dyn StepLifecycle>)`; if `endpoint_lifecycle` is `None`, use `Some(wiretap_lifecycle)` directly (no endpoint handle to compose). Store this as the `lifecycle` field of `CompiledStep::Process`.
4. Wrap the service: `processor: BoxProcessor::new(svc)`. The rest of the arm (`body_contract: None`) is unchanged.

**Tests:** (extend the existing `mod tests` in `endpoints.rs`, or add new tests alongside the existing WireTap lifecycle tests)
- `test_wiretap_compiler_composes_endpoint_and_wiretap_lifecycles`: setup a minimal Component + Endpoint whose `lifecycle()` returns a `StepLifecycle` fake (as the existing tests already do), and a WireTap URI resolving to it. Act: compile the WireTap step. Assert: the resulting `CompiledStep::Process.lifecycle` is `Some`, and invoking `shutdown(RouteStop)` on it shuts down BOTH the endpoint fake and the WireTap handle (observe the endpoint fake's shutdown counter incremented, and the WireTap's tracker drained via a tap if one is in flight).
- `test_wiretap_compiler_compose_when_no_endpoint_lifecycle`: setup an endpoint whose `lifecycle()` returns `None`. Act: compile the WireTap step. Assert: `lifecycle` is `Some` (the WireTap-only handle), and shutdown drains the WireTap handle.

**Acceptance:**
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-core --lib endpoints` passes (existing + 2 new).
- The WireTap arm composes via `CompositeStepLifecycle`; the `To` arm is unchanged.

- [x] 5

## camel-processor

### Task 6: Document divergences + update CONTEXT.md poll_ready/catalog rows

**Files:**
- `crates/camel-processor/CONTEXT.md` (modified)

**Steps:**
1. In the EIP catalog table row for `wire_tap`, update the description/source to reflect the Camel-faithful bounded-admission model.
2. In the "poll_ready contract" table, change the `WireTapService` row from `pending-fix` / "delegates to tap endpoint" to `migrated` / "`Ready(Ok(()))` unconditional" with rationale "Fire-and-forget tap readiness is checked inside the tap task; the main route never blocks on the tap (ADR-0019)."
3. Add a new section `## WireTap EIP divergences from Apache Camel (ADR-0046 protocol)` (mirror the existing `## Aggregator EIP divergences from Apache Camel` section shape). Document four divergences, each with the divergence statement, forcing rationale, and observable consequence:
   - `D-W1 flat-semaphore admission collapse`: Camel's two-tier `maxPoolSize=20` + `maxQueueSize=1000` collapsed to a single flat concurrency cap (semaphore) with `CallerRuns` at the bound. Forcing rationale: Camel's own virtual-thread executor is documented as exactly this semaphore-based flat cap. Consequence: operators configure one bound, not pool+queue.
   - `D-W2 CallerRuns transient exceed`: under saturation the inline task makes total concurrent execution reach `bound + 1` transiently. Forcing rationale: `CallerRuns` runs the tap on the caller's thread rather than queueing. Consequence: peak concurrency is `bound + number_of_saturated_callers`, not exactly `bound`.
   - `D-W3 route-level teardown, not CamelContext-level`: WireTap taps are drained/aborted at route `shutdown` (ADR-0022 `StepLifecycle`), not at CamelContext shutdown. Forcing rationale: rust-camel has no global `CamelContext` shutdown hook; the route lifecycle owns the drain. Consequence: stopping a route drains its taps; there is no cross-route pool.
   - `D-W4 absent pool-profile knobs`: Camel exposes `poolSize`, `maxPoolSize`, `maxQueueSize`, `rejectedPolicy`, executor service refs. rust-camel exposes only `max_concurrent` and `shutdown_grace`. Forcing rationale: the flat-semaphore model (D-W1) makes the pool/queue knobs redundant. Consequence: operators cannot tune the queue depth or choose a rejection policy other than CallerRuns.
4. Do NOT touch other sections of CONTEXT.md.

**Tests:** (documentation — no `#[test]`; verify via the build)
- `ctx_verify`: the `wire_tap` row in the EIP catalog and the poll_ready table read as specified; the new `## WireTap EIP divergences` section has four `### D-W` entries each naming (a) divergence, (b) rationale, (c) consequence. (Manual/eyeball check; no automated test.)

**Acceptance:**
- `rg -n "D-W1|D-W2|D-W3|D-W4" crates/camel-processor/CONTEXT.md` returns four matches.
- `rg -n "pending-fix" crates/camel-processor/CONTEXT.md` no longer matches a WireTapService row.
- The mdbook docs build is not affected (CONTEXT.md is not in `docs/src/`).

- [x] 6
