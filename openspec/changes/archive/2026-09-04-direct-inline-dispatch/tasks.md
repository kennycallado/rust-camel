# Tasks: direct-inline-dispatch

## Phase 0: Guardrails and baseline

### camel-component-direct

#### Task 0.1: Pin current cycle semantics with a regression test

**Files:**
- `crates/components/camel-direct/src/lib.rs` (modified — tests module at the end of the file)

**Steps:**
1. Add test helper `fn cycle_routes_ctx() -> (DirectComponent, ProducerContext)` in the existing `#[cfg(test)]` module that builds two DirectComponent endpoints `direct:a?timeout_ms=500` and `direct:b?timeout_ms=500` sharing one registry, where consumer a forwards to `direct:b` and consumer b forwards to `direct:a` (mirror the wiring pattern of `test_direct_producer_consumer_roundtrip`, lib.rs ~l.744).
2. Add `#[tokio::test] async fn test_direct_cycle_never_succeeds_or_hangs()`: dispatch one exchange into `direct:a` wrapped in a 5-second `tokio::time::timeout` around the producer dispatch and assert the outcome is `Err(_)` (a `CamelError`, whatever its variant) — never `Ok`, never a panic, never the 5s external deadline elapsing silently.

**Tests:** (executable spec)
- `test_direct_cycle_never_succeeds_or_hangs`: two cyclic routes with `timeout_ms=500` registered → producer dispatches one exchange into `direct:a` under a 5s external `tokio::time::timeout` → assert the join returns `Err` (dispatch error) well before the 5s deadline; assert it is NOT `Ok`.
  - command: `cargo test -p camel-component-direct --lib test_direct_cycle_never_succeeds_or_hangs`
  - expected: PASSES on unmodified code (the per-message timeout at lib.rs l.440 breaks the cycle).

**Acceptance:**
- `cargo test -p camel-component-direct --lib` exits 0 (new test plus all existing).
- `cargo clippy -p camel-component-direct -- -D warnings` exits 0.

- [x] 0.1

### camel-bench

#### Task 0.2: Add the direct-hop criterion benchmark and record the baseline

**Files:**
- `crates/camel-bench/benches/direct.rs` (new)
- `crates/camel-bench/Cargo.toml` (modified — add to `[dev-dependencies]`: `camel-component-direct.workspace = true`, `camel-component-api.workspace = true`; add a `[[bench]] name = "direct" harness = false` section)
- `openspec/changes/direct-inline-dispatch/bench/baseline.md` (new)

**Steps:**
1. Read `crates/camel-bench/benches/throughput.rs` and `pipeline.rs` first and copy THEIR async-with-criterion pattern exactly (runtime setup, `criterion_group!`/`criterion_main!`, iteration style) — do not invent a new harness shape.
2. Wire a REAL Sequential consumer route through camel-core's route controller (camel-bench already depends on camel-core): a `DirectComponent` with one endpoint `direct:hop`, its consumer started via the controller harness so the route gets the real pipeline, cancellation, and (from Phase 2 on) the published dispatcher capability — do NOT hand-construct `ConsumerContext` the way camel-direct's unit tests do, or the bench can silently stay on the channel fallback after Phase 3. The consumer pipeline is a minimal no-op processor (measure the HOP, not the processor); the bench owns the producer.
3. Define ONE criterion benchmark id `direct_hop`: a single producer dispatch → consumer no-op pipeline → reply, per iteration. The bench carries NO capability references (the dispatcher API arrives in Phase 2; the Phase-3 inline-path proof lives in Task 3.4).
4. Run from the worktree root: `cargo bench -p camel-bench --bench direct -- --save-baseline direct-inline-baseline` (baseline stored under `target/criterion`).
5. Write `bench/baseline.md`: date, worktree git rev, exact command, criterion's median point estimate for `direct_hop` in ns/iteration, and the protocol note (criterion default sample config, saved baseline name `direct-inline-baseline`).

**Tests:** (executable spec)
- `baseline-recorded`: after steps 4-5 → assert `bench/baseline.md` exists and contains a numeric `direct_hop` median.
  - command: `test -s openspec/changes/direct-inline-dispatch/bench/baseline.md && grep -c 'direct_hop' openspec/changes/direct-inline-dispatch/bench/baseline.md`
  - expected: exit 0 with count ≥ 1.

**Acceptance:**
- `cargo bench -p camel-bench --bench direct` exits 0 (worktree).
- `bench/baseline.md` exists with the median and command.
- No file under `benchmarks/` (the cross-framework harness) was modified.

- [x] 0.2

## Phase 1: Collapse the camel-direct channel (Hook B)

### camel-component-direct

#### Task 1.1: Route DirectProducer through ConsumerContext::send_and_wait

**Files:**
- `crates/components/camel-direct/src/lib.rs` (modified)
- `crates/components/camel-direct/CONTEXT.md` (modified — its channel/"direct sender" description, ~l.3/l.8/l.17, must move with the code)

**Steps:**
1. Replace the registry value type: `type DirectRegistry = Arc<Mutex<HashMap<String, DirectEntry>>>` and add `struct DirectEntry { ctx: ConsumerContext, closed: Arc<AtomicBool> }` next to the old `DirectSender` alias position (lib.rs l.34-35). Delete the `DirectSender` alias and its `mpsc`/`oneshot` imports once unused.
2. Liveness: add `struct CloseGuard(Arc<AtomicBool>)` with `impl Drop` setting `true`; `DirectConsumer` creates one per instance and stores its `Arc<AtomicBool>` in the entry. The mpsc sender's `is_closed()` semantics are replaced by `entry.closed.load(Ordering::Acquire)`: duplicate-consumer rejection (~l.292-293) rejects when an entry exists AND `!closed` (overwriting a crashed consumer's entry stays allowed, as today with `existing.is_closed()`); registry cleanup on consumer exit (~l.354-370) removes the entry AND sets `closed`.
3. `DirectConsumer` (lib.rs ~l.256-380): in the registration path (~l.286-291) insert `DirectEntry { ctx, closed }` instead of cloning an mpsc sender; delete the channel creation (`mpsc::channel(32)`, ~l.286-287), the per-message `oneshot::channel()` (~l.475), and the receive-forward loop that called `send_and_wait`; duplicate rejection and cleanup per step 2.
4. `DirectProducer::call` (lib.rs ~l.384-500): keep the `fail_if_no_consumers` lookup semantics (l.415-432) and the endpoint `Semaphore` acquire (l.244 field); replace the `(exchange, oneshot)` send + reply-await with `entry.ctx.send_and_wait(exchange).await` wrapped in the existing `tokio::time::timeout` (l.440, default 30_000ms).
5. `poll_ready` (~l.415-432): replace the closed-sender arm (l.426) with the `closed` flag check — an entry that exists with `closed == true` reports not-ready exactly as a closed sender does today; live entries report ready without touching the semaphore (spec `direct-startup-handshake`, requirement "poll_ready behavior unchanged", stays intact).
6. Update the module doc claim "no serialization overhead" (~l.1-30) to state the truth after this change: producer submits directly to the route controller via `ConsumerContext::send_and_wait`; no per-message channel or oneshot exists inside camel-direct. Update `CONTEXT.md` to match.

**Tests:** (executable spec — the existing suite is the safety net; it must stay green through the rework)
- `test_direct_producer_consumer_roundtrip` (existing, ~l.744): unchanged code → run → still passes (reply exchange returns to producer).
- `test_direct_producer_no_consumer_registered` (existing, ~l.690): unchanged code → run → still fails with the no-consumer error.
- `test_direct_duplicate_consumer_returns_error` (existing, ~l.704): unchanged code → run → duplicate registration still errors before `mark_ready`.
- `test_direct_propagates_error_when_no_handler` (existing, ~l.795): unchanged code → run → pipeline error still propagates.
- `test_direct_cycle_never_succeeds_or_hangs` (Task 0.1): unchanged code → run → still passes (timeout still bounds the cycle).
  - command: `cargo test -p camel-component-direct --lib`
  - expected: all pass after the rework; compile failure or any regression = task incomplete.
- NEW `test_direct_crashed_consumer_entry_is_overwritable`: in-module test inserts a stale entry directly — `registry.lock().insert(name, DirectEntry { ctx, closed: Arc::new(AtomicBool::new(true)) })` (no production hook; a closed entry IS the crashed-consumer state) → register a second consumer on the same name → assert registration succeeds and replaces the stale entry (mirrors `existing.is_closed()` overwrite today).
- NEW `test_direct_poll_ready_reports_stale_entry_not_ready`: same direct stale-entry insertion → call `poll_ready` → assert not-ready outcome matches today's closed-sender arm (l.426 behavior).
  - command: `cargo test -p camel-component-direct --lib test_direct_crashed_consumer_entry_is_overwritable && cargo test -p camel-component-direct --lib test_direct_poll_ready_reports_stale_entry_not_ready`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-component-direct --lib` exits 0.
- `cargo clippy -p camel-component-direct -- -D warnings` exits 0.
- `rg 'DirectSender|oneshot' crates/components/camel-direct/src/lib.rs` returns zero hits.

- [x] 1.1

### camel-bench

#### Task 1.2: Record the informational Phase 1 bench re-run

**Files:**
- `openspec/changes/direct-inline-dispatch/bench/phase1.md` (new)

**Steps:**
1. Re-run the Phase 0 bench identically from the worktree root: `cargo bench -p camel-bench --bench direct -- --baseline direct-inline-baseline`.
2. Derive the ratio reproducibly from criterion's own artifacts: read `median.point_estimate` from `target/criterion/direct_hop/direct-inline-baseline/estimates.json` and `target/criterion/direct_hop/new/estimates.json`, compute `baseline / current`. Write `bench/phase1.md`: date, git rev, command, both medians, and the ratio. Informational only — NOT a gate (design.md Phase 1 exit criteria; Hook B removes one of two round-trips, so a partial improvement is expected and just recorded).

**Tests:** (executable spec)
- `phase1-recorded`: after step 2 → assert `bench/phase1.md` exists with a numeric median.
  - command: `test -s openspec/changes/direct-inline-dispatch/bench/phase1.md && grep -c 'direct_hop' openspec/changes/direct-inline-dispatch/bench/phase1.md`
  - expected: exit 0 with count ≥ 1.

**Acceptance:**
- `bench/phase1.md` exists with the median and the change ratio.
- No file under `benchmarks/` modified.

- [x] 1.2

## Phase 2: InlineRouteDispatcher seam

### camel-component-api

#### Task 2.1: Declare the InlineRouteDispatcher capability

**Files:**
- `crates/components/camel-component-api/src/dispatch.rs` (new)
- `crates/components/camel-component-api/src/lib.rs` (modified — add `mod dispatch; pub use dispatch::*;`)

**Steps:**
1. Define the object-safe capability trait:
   `pub trait InlineRouteDispatcher: Send + Sync + 'static { fn dispatch(&self, exchange: Exchange) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send + 'static>>; }` (use the `Future`/`Pin` import style already used in this crate; if the crate already re-exports a boxed-future alias, use it).
2. Extend `ConsumerContext` (`crates/components/camel-component-api/src/consumer.rs`, struct at l.175) with an interior-mutability slot for the capability using `std::sync::OnceLock`: `pub fn set_inline_dispatcher(&self, dispatcher: std::sync::Arc<dyn InlineRouteDispatcher>)` — set-once-KEEP-FIRST contract: if `OnceLock::set` returns `Err` (already set), emit `tracing::warn!("inline dispatcher already set; ignoring second set")` and keep the first (no new CamelError variant — the contract-enum taxonomy, ADR-0049, is untouched) — and `pub fn inline_dispatcher(&self) -> Option<std::sync::Arc<dyn InlineRouteDispatcher>>`. Document "set once by the camel-core runtime before the consumer starts".
3. Keep the capability opaque: the trait exposes ONLY `dispatch`; no pipeline/processor accessor leaks (hexagonal boundary).

**Tests:** (executable spec — in `dispatch.rs` `#[cfg(test)]` or the crate's existing test module)
- `inline_dispatcher_defaults_to_none`: fresh `ConsumerContext` (however existing tests construct one) → call `inline_dispatcher()` → assert `None`.
- `inline_dispatcher_set_then_get_roundtrip`: construct `ConsumerContext` + a no-op fake dispatcher struct → `set_inline_dispatcher(Arc::new(fake))` → `inline_dispatcher()` → assert `Some` with matching dispatch behavior (dispatch returns the exchange unchanged).
- `inline_dispatcher_second_set_keeps_first`: set dispatcher A, then set dispatcher B → `inline_dispatcher()` → assert the returned dispatcher still behaves as A (set-once-keep-first; second set is a no-op warning, not an error).
  - command: `cargo test -p camel-component-api --lib dispatch`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-component-api --lib` exits 0.
- `cargo clippy -p camel-component-api -- -D warnings` exits 0.
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` exits 0.

- [x] 2.1

### camel-core

#### Task 2.2: Implement the dispatcher adapter and wire capability publication

**Files:**
- `crates/camel-core/src/lifecycle/adapters/inline_dispatcher.rs` (new)
- `crates/camel-core/src/lifecycle/adapters/mod.rs` (modified — add module)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified — publication site immediately after `consumer_ctx` creation, l.~357, BEFORE the aggregator-v2 early-return branch at l.~361-387)

**Steps:**
1. Implement `pub(crate) struct RouteInlineDispatcher` in `inline_dispatcher.rs` holding: `route_id: String`, a handle to the pipeline swap source as used by `pipeline_runtime.rs` (the same source `ready_with_backoff` polls, route_controller_trait.rs ~l.559), `cancel: CancellationToken` (the route's `pipeline_cancel` child-token scope), `drain_in_flight` Arc handle of the same type used by `DrainGuard` (~l.469-572), `admission: Arc<tokio::sync::Mutex<()>>` (FIFO admission permit), a handle to the startup-cohort barrier used by the envelope drain sites (~l.526-543), and `hop_budget: AtomicU32` (fairness yield counter, owned per endpoint — cumulative across ALL dispatches through this dispatcher, which covers the 100-sequential-dispatch split-aggregate shape and any concurrent producers).
2. `impl InlineRouteDispatcher for RouteInlineDispatcher::dispatch(exchange)` — exact structure (no worker design freedom):
   - Take the `DrainGuard` (increment `drain_in_flight`) BEFORE starting the dispatch operation.
   - Load ONE pipeline snapshot at entry and hold it for the whole call (ADR-0004 pattern; mirror `pipeline_runtime.rs` l.34-40/85-87 snapshot discipline).
   - Build ONE operation future, in order: acquire the `admission` Mutex (FIFO serialization of concurrent producers) → park on the startup-cohort barrier (same mechanism as the envelope drain sites, ~l.526-543 — keeps the canonical `consumer-activation` barrier-covers-every-topology requirement true for the inline topology) → `ready_with_backoff` (reuse the pattern) → `CANCEL_TOKEN.scope(operation_cancel, pipeline.call(exchange))` (scope mechanism per route_controller_trait.rs ~l.571-575 / route_compiler.rs ~l.537).
   - Race the whole operation against the consumer token with a biased select so the consumer arm wins ties:
     `tokio::select! { biased; _ = cancel.cancelled() => Err(CamelError::ConsumerStopping), result = operation => result }`
     Dropping `operation` on the consumer arm immediately drops the admission/barrier/readiness/pipeline futures and releases the admission permit.
   - On success increment `hop_budget` via `fetch_add`; when the previous value + 1 crosses a multiple of 32 → `tokio::task::yield_now().await` before returning (≥1 yield per 32 completed hops on the executing task).
   - Producer cancellation: dropping the entire `dispatch` future cancels it WITHOUT touching the consumer token; `DrainGuard::drop` decrements `drain_in_flight` exactly once on every exit path — producer cancellation, consumer cancellation, success, or error.
3. Publication: in `route_controller_trait.rs`, immediately after `consumer_ctx` is created (l.~357) and BEFORE the aggregator-v2 early-return branch (l.~361-387), gate on the effective concurrency model being non-Concurrent — `if !matches!(model, Concurrent { .. })` — because the `#[non_exhaustive]` wildcard arm (l.~501-506) is Sequential-equivalent and aggregate routes take the early-return branch: construct the dispatcher and call `consumer_ctx.set_inline_dispatcher(Arc::new(dispatcher))` BEFORE `mark_ready`; `Concurrent` models leave the capability `None` (channel path).
4. No producer behavior change in this task — camel-direct still calls `send_and_wait`.

**Tests:** (executable spec — `inline_dispatcher.rs` `#[cfg(test)]`, reuse the route/pipeline test harness from `route_controller_tests.rs`)
- `dispatch_holds_snapshot_through_completion`: dispatcher holds a pipeline snapshot → swap the pipeline source mid-dispatch (park the first dispatch on a test processor) → assert the in-flight dispatch completes against the OLD snapshot and the NEXT dispatch uses the new one.
- `dispatch_decrements_in_flight_exactly_once`: run dispatch to completion → assert `drain_in_flight` returned to its starting value; run a dispatch that fails (pipeline processor returns error) → same assertion.
- `dispatch_serializes_concurrent_callers_fifo`: two tasks call `dispatch` concurrently against a pipeline that records entry order → assert executions do not interleave and complete in call order.
- `dispatch_parks_on_startup_cohort`: context mid-boot with a sibling still Registered → first inline dispatch parks → complete the cohort → assert the dispatch then executes.
- `dispatch_yields_every_32_hops`: run 100 sequential dispatch calls through one dispatcher (no natural await in the test pipeline) → assert `hop_budget` reached ≥ 100 and the yield site fired ≥ 3 times (expose `#[cfg(test)] fn hop_budget_for_test(&self) -> u32` AND a sibling `#[cfg(test)] yields_for_test(&self) -> u32` counter incremented at the `yield_now` site inside `dispatch`).
- `dispatch_consumer_cancel_during_admission_returns_consumer_stopping`: externally lock the dispatcher's shared admission Mutex; start `dispatch`; confirm it stays blocked with pipeline entry count zero; cancel the consumer token → assert `Err(CamelError::ConsumerStopping)` within 1 second, zero pipeline entries, in-flight counter restored to baseline.
- `dispatch_consumer_cancel_during_execution_returns_consumer_stopping`: pipeline signals entry then parks forever, with a drop probe; start `dispatch`, await the entry signal, cancel the token → assert `ConsumerStopping` within 1 second, the drop probe fired (operation future dropped), admission permit released, in-flight baseline restored.
- `sequential_consumer_publishes_capability`: spawn a Sequential consumer route via the controller harness → assert `ctx.inline_dispatcher().is_some()`; `concurrent_consumer_gets_no_capability`: same with `Concurrent { max: None }` → assert `is_none()`.
- `aggregate_route_gets_capability`: a route whose consumer is the aggregator-v2 branch (takes the l.~361-387 early return) → assert `ctx.inline_dispatcher().is_some()` (publication precedes the branch).
  - command: `cargo test -p camel-core --lib lifecycle::adapters::inline_dispatcher`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` exits 0.

- [x] 2.2

### camel-component-direct

#### Task 2.3: Store the capability in DirectRegistry entries

**Files:**
- `crates/components/camel-direct/src/lib.rs` (modified)

**Steps:**
1. Extend `DirectEntry` (Task 1.1) to `struct DirectEntry { ctx: ConsumerContext, closed: Arc<AtomicBool>, dispatcher: Option<Arc<dyn InlineRouteDispatcher>> }`.
2. `DirectConsumer` startup: after obtaining its `ConsumerContext` and BEFORE registry insert + `mark_ready` (order per the `direct-startup-handshake` spec — duplicate-rejection semantics unchanged), read `ctx.inline_dispatcher()` once and store the `Option` in the entry.
3. `DirectProducer::call` still ignores the capability in this task (behavior identical to Phase 1) — only storage and typing change.
4. Duplicate-consumer rejection, cleanup-on-exit, `poll_ready`, and `failIfNoConsumers` paths stay byte-equivalent in behavior.

**Tests:** (executable spec)
- `test_direct_registry_entry_carries_dispatcher_option`: set a fake dispatcher on the consumer ctx before startup → assert the locked registry entry exposes `dispatcher: Some(_)`; without one → `None`.
- `test_direct_duplicate_consumer_returns_error` (existing): unchanged code → still passes (registration order not disturbed).
  - command: `cargo test -p camel-component-direct --lib`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-component-direct --lib` exits 0.
- `cargo clippy -p camel-component-direct -- -D warnings` exits 0.

- [x] 2.3

## Phase 3: Inline fast path

### camel-component-direct

#### Task 3.1: Task-local inline guard (cycle and depth stack)

**Files:**
- `crates/components/camel-direct/src/inline_guard.rs` (new)
- `crates/components/camel-direct/src/lib.rs` (modified — `mod inline_guard;`)

**Steps:**
1. `task_local!` stack: `static INLINE_STACK: RefCell<InlineStack>` where `struct InlineStack { active: Vec<Box<str>> }` (NO hop budget here — hop counting lives on the dispatcher, Task 2.2; task-locals cannot be created lazily, so the stack only works inside an explicit scope, established in step 3).
2. `pub(crate) fn enter(name: &str) -> Result<InlineGuard, CamelError>` (called only inside a scope): if `active` contains `name` → `Err(CamelError::ProcessorError(format!("direct cycle detected re-entering direct:{name}")))`; if `active.len() >= 64` → `Err(CamelError::ProcessorError("direct inline dispatch depth limit (64) exceeded".into()))`; else push and return `InlineGuard`. CONSTRAINT: `InlineGuard` holds only an owned `Box<str>` copy of the name and pops by name equality in `Drop` — it must NOT hold the `RefCell` borrow across the dispatch await (the producer's `Service::Future` must stay `Send`; every task-local access happens in a fresh short-lived scope inside `enter`/`Drop`).
3. Scope establishment (mirror the `CANCEL_TOKEN` pattern, route_compiler.rs l.49/535 — `try_with` to detect, `scope` to create): `pub(crate) async fn with_inline_stack<R>(fut: impl Future<Output = R>) -> R` — if `INLINE_STACK.try_with(|_| ())` succeeds (nested dispatch, outer scope exists) → plain `fut.await`; else → `INLINE_STACK.scope(RefCell::new(InlineStack::default()), fut).await`. A NESTED chain (dispatcher → producer → dispatch) therefore shares one stack; sequential non-nested calls each get a fresh stack (correct — cycles and depth only exist within a nested chain).
4. Error text constants: define `const CYCLE_ERROR_PREFIX: &str = "direct cycle detected"` and `const DEPTH_ERROR: &str = "direct inline dispatch depth limit (64) exceeded"` as `pub(crate)` for reuse in tests and in Task 3.2's producer code.

**Tests:** (executable spec — `inline_guard.rs` `#[cfg(test)]`)
- `guard_rejects_cycle_immediately`: inside `with_inline_stack`, `enter("a")` held → `enter("a")` → assert `Err` whose message starts with `CYCLE_ERROR_PREFIX`.
- `guard_rejects_depth_65`: inside `with_inline_stack`, enter 64 distinct names nesting → 65th `enter` → assert `Err` with `DEPTH_ERROR`.
- `guard_allows_64_and_unwinds`: inside `with_inline_stack`, enter 64 distinct names → all Ok; drop guards in reverse → a fresh `enter("a")` is Ok (stack empty again).
- `nested_calls_share_stack`: a future that calls `with_inline_stack(inner)` from inside `with_inline_stack(outer)` where `inner` enters the name `outer` entered → assert cycle error (stack shared, not reset, by the nested wrapper).
  - command: `cargo test -p camel-component-direct --lib inline_guard`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-component-direct --lib` exits 0.
- `cargo clippy -p camel-component-direct -- -D warnings` exits 0.

- [x] 3.1

#### Task 3.2: Producer inline selection with guards and timeout parity

**Files:**
- `crates/components/camel-direct/src/lib.rs` (modified)

**Steps:**
1. `DirectProducer::call`: keep the `fail_if_no_consumers` fast-fail (l.415-432) — then perform the path selection, registry lookup, AND dispatch INSIDE one `tokio::time::timeout` async block (mirroring the channel path, where lookup sits inside the timed section at l.459-464): if `entry.dispatcher` is `Some(d)` → inline path; else → `entry.ctx.send_and_wait` exactly as Phase 1 (channel path = the consumer-context submission; covers Concurrent consumers and capability-unavailable entries).
2. Inline path inside the timed block: `inline_guard::with_inline_stack(dispatch_future)` wraps the dispatch await (establishes the cycle/depth scope, Task 3.1 step 3); inside it, `enter(&name)?` then awaits `dispatch_future` (`d.dispatch(exchange)`). SKIP the endpoint `Semaphore` on the inline path — the dispatcher's `admission` Mutex owns serialization (the l.449-456 comment's rationale "permit contention must not eat the dispatch timeout budget" stays honored: admission is inside the timed section by design and is the single serializer; the channel path keeps the semaphore untouched). Hop-budget yield happens inside `dispatch` (Task 2.2 step 2) — the producer does NOT track hops. Map cycle/depth errors straight out (NO fallback to the channel path on guard rejection).
3. Timeout construction shared by both paths: extract BOTH the effective-`Duration` construction (`self.config.timeout_ms.unwrap_or(30_000)`, l.440) and the timeout-error text construction (l.440-500 arm) into small private fns used by the channel and inline paths so neither default nor text can drift.
4. Reply semantics: the `Result<Exchange, CamelError>` returned by `dispatch` IS the producer result — same shape as `send_and_wait` (consumer.rs l.300 returns the transformed exchange or error; Completed/Stopped mapping already happened inside the dispatcher — Task 2.2 step 2).

**Tests:** (executable spec — extend the crate test module; fake `InlineRouteDispatcher` per Task 2.1's test pattern; `ConsumerContext` is concrete — use real ones from the existing test harness (`test_producer_ctx`, l.574) and assert channel non-use by checking the context's submission channel received zero envelopes)
- `inline_dispatch_roundtrip_same_task`: registry entry with fake dispatcher that appends a marker header → producer call → assert Ok and marker present (pipeline ran inline, reply returned).
- `inline_falls_back_when_capability_absent`: entry with `dispatcher: None` → producer call → assert the consumer-context submission path ran (real ctx; an envelope arrived on its route submission receiver) and the result propagates.
- `inline_timeout_error_text_matches_channel`: fake dispatcher whose `dispatch` parks forever; endpoint `timeout_ms=Some(200)` → assert producer error text equals the text produced by the channel path with a parking consumer (construct both paths in one test, compare strings).
- `inline_timeout_covers_admission_wait`: fake dispatcher holding the admission Mutex (first dispatch parked inside `dispatch`); endpoint `timeout_ms=Some(200)` → a second concurrent producer call times out with the channel-path timeout error text (admission wait is inside the boundary).
- `inline_default_timeout_is_30s`: endpoint without `timeout_ms` → assert the shared private duration fn from step 3 returns 30_000ms for both paths.
- `inline_cycle_rejected_without_fallback`: fake dispatcher that re-dispatches into `direct:<same name>` through a nested producer → assert outer error starts with `CYCLE_ERROR_PREFIX` and the channel fallback was NOT invoked (real ctx; assert its submission receiver received zero envelopes).
- `concurrent_producers_serialized_fifo`: entry with fake dispatcher recording execution order under an internal delay; spawn 4 tasks calling the producer → assert recorded order equals admission order and no interleaving.
  - command: `cargo test -p camel-component-direct --lib`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-component-direct --lib` exits 0.
- `cargo clippy -p camel-component-direct -- -D warnings` exits 0.
- `cargo fmt --check` clean for the crate.

- [x] 3.2

### camel-core

#### Task 3.3: Real-route integration — eligibility, cancellation, restart

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified — resume_route publication per step 2, rc-y4vk)
- `crates/camel-core/src/lifecycle/adapters/inline_dispatcher.rs` (modified — tests + any dual-domain fix)

**Steps:**
1. Verify end-to-end eligibility through the real controller: a `direct:` route with Sequential model exposes `inline_dispatcher() == Some` at the consumer; switching the same route definition to `Concurrent { max: Some(1) }` leaves it `None`. If the effective-model gate at the publication site (Task 2.2 step 3) is wrong for any route shape, fix it there.
2. Resume-path publication (bd rc-y4vk): `resume_route` (~l.832-834) builds a fresh `ConsumerContext` without publishing — mirror the `start_route` publication block there (same `!matches!(model, Concurrent { .. })` gate, same handle captures, BEFORE the resumed consumer spawn; fresh ctx = fresh OnceLock, keep-first cannot interfere).
3. Dual-domain cancellation through the REAL dispatcher: (a) start an inline dispatch parked in a test processor → stop the consumer route → assert drain grace elapses, then the dispatch fails with `CamelError::ConsumerStopping`; (b) cancel the producer task mid-dispatch → assert the route keeps running and `drain_in_flight` returns to baseline (exactly-once decrement); (c) stop the consumer while an inline dispatch is in flight, then restart the route → assert fresh tokens and zero in-flight.
4. Keep b′ ownership: dispatch/timeout errors surface from the consumer route's error taxonomy exactly as `send_and_wait` errors do today (assert the error variant matches, not a new variant) — ADR-0012 boundaries unchanged.

**Tests:** (executable spec — route_controller harness)
- `inline_consumer_stop_yields_consumer_stopping`: per step 3(a) → assert `CamelError::ConsumerStopping`.
- `inline_producer_cancel_keeps_route_alive`: per step 3(b) → assert route still dispatches after producer drop + counter baseline.
- `inline_restart_fresh_cancellation_state`: per step 3(c) WITH proof of inline selection — the test consumer captures each received `ConsumerContext`; save the pre-stop dispatcher Arc; perform full stop then start; assert the NEW context's `inline_dispatcher()` is `Some` and `Arc::ptr_eq(old, new)` is false; assert the OLD dispatcher now returns `CamelError::ConsumerStopping`; invoke the NEW dispatcher directly against a marker pipeline → assert success; assert in-flight count is zero before and after the new dispatch.
- `inline_error_taxonomy_matches_channel`: run one failing pipeline via inline and one via channel (capability stripped) → assert identical `CamelError` variants for the same processor failure.
- `inline_resume_republishes_capability`: suspend a Sequential `direct:` route (entry closed, pipeline plane alive) → resume it → assert the fresh consumer context exposes `inline_dispatcher() == Some` and the registry entry carries it (no silent channel fallback after resume).
- `inline_stopped_consumer_keeps_no_consumer_semantics`: consumer route running with inline capability → stop the route (registry cleanup path) → producer dispatch with `fail_if_no_consumers` default → assert the existing no-consumer error, identical to a never-registered name.
  - command: `cargo test -p camel-core --lib lifecycle::adapters::inline_dispatcher`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 3.3

### camel-bench

#### Task 3.4: Bench gate re-run and cycle test tightening

**Files:**
- `openspec/changes/direct-inline-dispatch/bench/phase3.md` (new)
- `crates/components/camel-direct/src/lib.rs` (modified — tighten Task 0.1's test)
- `crates/camel-bench/benches/direct.rs` (modified — untimed inline-path proof)

**Steps:**
1. Add to `benches/direct.rs` an UNTIMED Phase-3 proof that runs once before the criterion loop and panics on failure: the consumer no-op pipeline records the `tokio::task::id()` of its execution; the bench records the producer-side task id around one warm dispatch; assert the two are EQUAL (inline dispatch = pipeline executes on the producer's task; channel fallback = different task id — this prevents silently gating on a channel-path measurement). Then re-run from the worktree root: `cargo bench -p camel-bench --bench direct -- --baseline direct-inline-baseline`.
2. Derive the ratio reproducibly from criterion's own artifacts: read `median.point_estimate` from `target/criterion/direct_hop/direct-inline-baseline/estimates.json` and `target/criterion/direct_hop/new/estimates.json`, compute `baseline / current` (record ≥ 1 decimal). Write `bench/phase3.md`: date, git rev, command, both medians, and the ratio.
3. Tighten `test_direct_cycle_never_succeeds_or_hangs`: assert the cycle error arrives within a small window (assert elapsed < 1s with `timeout_ms=500` configured — the guard rejects immediately; the timeout no longer breaks the cycle) and that the error message starts with `CYCLE_ERROR_PREFIX`.
4. Attribution note: if the ratio is < 5x, do NOT tweak the bench; report the number and stop for review (conductor decides: further profiling or human).

**Tests:** (executable spec)
- `bench-gate`: after step 2 → assert `phase3.md` records a ratio ≥ 5.0.
  - command: `python3 -c "import re;t=open('openspec/changes/direct-inline-dispatch/bench/phase3.md').read();r=[float(x) for x in re.findall(r'ratio[: ]+([0-9.]+)',t)];assert r and r[0]>=5.0, r"`
  - expected: exit 0.
- `test_direct_cycle_never_succeeds_or_hangs` (tightened): cycle dispatch with `timeout_ms=500` → assert `Err` with `CYCLE_ERROR_PREFIX` message and elapsed < 1s.
  - command: `cargo test -p camel-component-direct --lib test_direct_cycle_never_succeeds_or_hangs`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-component-direct --lib` exits 0.
- `bench/phase3.md` exists; recorded ratio ≥ 5.0 (or the task is reported blocked per step 4).
- No file under `benchmarks/` modified.

- [x] 3.4

## Phase 4: Aggregator correlation-key trim

### camel-processor

#### Task 4.1: Constant-key fast resolution without per-fragment serde

**Files:**
- `crates/camel-processor/src/aggregator.rs` (modified — `AggregatorService::call`, ~l.404-540)

**Steps:**
1. Add a memoized-key field to `AggregatorService`: `cached_key: Arc<std::sync::Mutex<Option<(serde_json::Value, String)>>>` — a separate field cloned into the `'static` call future exactly like `buckets` (aggregator.rs l.405-411 clones all cross-call state as `Arc`; a bare `std::Mutex` field would be unreachable inside the async block; do NOT widen the buckets `Arc<Mutex<HashMap>>` payload or its lock sites, ~l.211/387/427).
2. In `call` (~l.412-417), after `extract_correlation_key`: apply the fast path to SCALAR keys only — `Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null`: if `cached_key` holds `(v, s)` with `v == key_value` → reuse `s` (zero serde, one scalar equality check); else serialize via `serde_json::to_string` exactly as today, store the pair, use the new string. Object/array keys BYPASS the cache entirely and serialize per fragment (object `Value` equality is key-order-insensitive while `to_string` preserves insertion order — caching them could merge distinct buckets; scalars have no such divergence).
3. Remove the unconditional `key_str.clone()` at bucket insert (~l.465-475): obtain the bucket first — `let bucket = match guard.get_mut(&key_str) { Some(b) => b, None => guard.entry(key_str.clone()).or_insert_with(Bucket::new) };` — then run the EXISTING push + completion checks once on `bucket` (no double push, no dropped first fragment); the clone happens only on first insert for a key (leave the existing cold-path log clones untouched).
4. No behavior change: bucket identity, ordering, completion predicates, `max_buckets`/`max_bucket_size` checks, and log output stay identical.

**Tests:** (executable spec)
- `constant_key_skips_reserialization`: service with a constant scalar-key correlation; send 3 fragments with identical key → assert all 3 land in ONE bucket and the cached pair was built exactly once (expose a `#[cfg(test)]` counter on the service state; assert serializations == 1).
- `divergent_keys_keep_serde_semantics`: send fragments with scalar keys `k1, k2, k1` → assert two buckets with byte-identical names to `serde_json::to_string` of the values (compare against direct serde calls in the test).
- `object_keys_bypass_cache`: send fragments whose correlation yields objects with equal key-sets but different insertion order → assert TWO buckets (cache not consulted for objects; behavior identical to today).
- existing aggregator suite: unchanged code → `cargo test -p camel-processor --lib aggregator` → all pass.
  - command: `cargo test -p camel-processor --lib aggregator`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 4.1

  > Deviation (review-ratified): the memoizable set was narrowed from
  > "SCALAR keys" to `Value::String(_)` only. serde_json `Number` equality
  > treats `0.0 == -0.0` as equal while `to_string` distinguishes them
  > ("0.0" vs "-0.0") — memoizing numbers could merge serde-distinct
  > buckets (ADR-0032 adversary-controlled keys). Bool/Null serialize to
  > constant output, so their bypass costs nothing. Regression test
  > `float_zero_sign_keys_stay_distinct` pins the two-bucket behavior.

  > Deviation (review-ratified): `object_keys_bypass_cache` asserts a
  > serde-derived expected bucket-key set plus serializations==3 instead of
  > the literal "TWO buckets" — serde_json `preserve_order` is enabled
  > workspace-wide via siumai (camel-component-llm chain) but absent from
  > camel-processor's standalone closure, so Map is a BTreeMap standalone
  > and insertion-ordered under workspace feature unification; a literal
  > bucket count flips between harness shapes, while the derived assertion
  > is invariant under both.
