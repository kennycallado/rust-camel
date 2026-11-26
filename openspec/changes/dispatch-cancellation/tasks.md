# Tasks: dispatch-cancellation

## 1. camel-api: dispatch-cancellation carrier helper

**Files**
- `crates/camel-api/src/exchange.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified)

**Steps**
1. In `exchange.rs`, beside `ORIGINAL_MESSAGE_EXTENSION` (line 15), add
   `pub const DISPATCH_CANCEL_EXTENSION: &str = "CamelDispatchCancel";` with a
   doc comment: an enqueue-only `CancellationToken` marker stamped by
   `ConsumerContext::send_and_wait`; each pipeline entry takes (removes) it;
   cancellation stops the pipeline before its next step (ADR-0043) and is
   independent of the consumer-lifetime and route-stop tokens.
2. Add `pub struct DispatchCancellation` in `exchange.rs` with private fields
   `token: tokio_util::sync::CancellationToken`,
   `guard: Option<tokio_util::sync::DropGuard>`, and
   `drain_owner: Option<tokio_util::sync::CancellationToken>`, plus:
   - `pub fn token(&self) -> &tokio_util::sync::CancellationToken`
   - `pub fn drain_on(mut self, owner: tokio_util::sync::CancellationToken) -> Self`
     that stores `Some(owner)` and returns `self`. The owner is the
     consumer-lifetime token: when it is cancelled, the drop is
     shutdown-driven and must NOT cancel the dispatch (ADR-0043 drain grace).
   - `pub fn disarm(mut self)` that defuses the guard without cancelling:
     `if let Some(guard) = self.guard.take() { drop(guard.disarm()); }`
     (`DropGuard::disarm(self)` returns the live token; dropping that token
     does not cancel. Plain `self.guard.take()` would drop the guard and
     cancel — do not use it.)
   - `impl Drop for DispatchCancellation`: take the guard; if
     `self.drain_owner.as_ref().is_some_and(CancellationToken::is_cancelled)`,
     call `guard.disarm()` (defuse — shutdown owns the exchange); otherwise
     drop the guard (its Drop cancels the token).
3. In `impl Exchange`, add:
   - `pub fn begin_dispatch_cancel(&mut self) -> DispatchCancellation` — mints
     `tokio_util::sync::CancellationToken::new()`, stamps
     `Arc::new(token.clone())` under `DISPATCH_CANCEL_EXTENSION`, and returns
     `DispatchCancellation` holding the token clone and
     `token.drop_guard()`.
   - `pub fn take_dispatch_cancel(&mut self) -> Option<tokio_util::sync::CancellationToken>`
     — `self.extensions.remove(DISPATCH_CANCEL_EXTENSION)` downcast to
     `CancellationToken`, cloned.
   - `pub fn dispatch_cancel(&self) -> Option<tokio_util::sync::CancellationToken>`
     — read-only peek for tests (`get_extension::<CancellationToken>` cloned).
4. Re-export `DISPATCH_CANCEL_EXTENSION` and `DispatchCancellation` from
   `crates/camel-api/src/lib.rs`: add both names to the re-export group that
   currently begins `pub use exchange::{CAMEL_STOP, Exchange,` and keep it
   sorted.
5. Doc-comment every new public item. State that the marker is
   non-serializable exchange metadata and that a reply from `send_and_wait`
   carries no marker (the pipeline takes it). Module-qualify
   `tokio_util::sync::CancellationToken` in signatures or import the module
   (`use tokio_util::sync;`); do not glob-import.

**Tests** (module `tests` at the bottom of `exchange.rs`)
- name: `dispatch_cancel_begin_stamps_a_live_token`
  - setup: `let mut ex = Exchange::new(Message::new("x"));`
  - action: `let cancel = ex.begin_dispatch_cancel();`
  - assert: `ex.dispatch_cancel().is_some()` and not cancelled;
    `cancel.token().is_cancelled()` is false.
  - command: `cargo test -p camel-api --lib dispatch_cancel_begin_stamps_a_live_token`
  - expected after implementation: pass.
- name: `dispatch_cancel_guard_cancels_on_drop`
  - setup: an exchange plus `let cancel = ex.begin_dispatch_cancel();`
  - action: keep `let token = cancel.token().clone(); drop(cancel);`
  - assert: `token.is_cancelled()`.
  - command: `cargo test -p camel-api --lib dispatch_cancel_guard_cancels_on_drop`
- name: `dispatch_cancel_disarm_leaves_token_live`
  - setup: an exchange plus `let cancel = ex.begin_dispatch_cancel();`
  - action: keep `let token = cancel.token().clone(); cancel.disarm();`
  - assert: `!token.is_cancelled()` after the handle is consumed.
  - command: `cargo test -p camel-api --lib dispatch_cancel_disarm_leaves_token_live`
- name: `dispatch_cancel_take_removes_the_marker`
  - setup: an exchange stamped via `begin_dispatch_cancel` (disarm it).
  - action: `let taken = ex.take_dispatch_cancel();`
  - assert: `taken.is_some()`, a second `take_dispatch_cancel()` returns
    `None`, and `dispatch_cancel()` returns `None` afterwards.
  - command: `cargo test -p camel-api --lib dispatch_cancel_take_removes_the_marker`
- name: `dispatch_cancel_drop_during_shutdown_leaves_token_live`
  - setup: an exchange; `let owner = CancellationToken::new();`
    `let cancel = ex.begin_dispatch_cancel().drain_on(owner.clone());`
  - action: keep `let token = cancel.token().clone(); owner.cancel(); drop(cancel);`
  - assert: `!token.is_cancelled()` (shutdown-driven drop defused the guard).
  - command: `cargo test -p camel-api --lib dispatch_cancel_drop_during_shutdown_leaves_token_live`
- name: `dispatch_cancel_clone_shares_the_same_token`
  - setup: an exchange stamped and disarmed.
  - action: `let copy = ex.clone(); let token = copy.dispatch_cancel().expect("marker on clone"); token.cancel();`
  - assert: `ex.dispatch_cancel().expect("marker on original").is_cancelled()`.
  - command: `cargo test -p camel-api --lib dispatch_cancel_clone_shares_the_same_token`

**Acceptance**
- `cargo test -p camel-api --lib dispatch_cancel` exits 0 (six tests).
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-missing-docs-ratchet` exits 0.
- The diff is additive: no existing public item changes shape.


- [x] 1 camel-api carrier helper
## 2. camel-component-api: send_and_wait begins and arms the dispatch cancellation

**Files**
- `crates/components/camel-component-api/src/consumer.rs` (modified)
- `crates/components/camel-component-api/src/consumer_claim_tests.rs` (modified)
- `AGENTS.md` (modified: component lane row)

**Steps**
1. In `send_and_wait` (`consumer.rs:435`), replace the plain envelope build
   with: `let mut exchange = exchange; let dispatch = exchange.begin_dispatch_cancel().drain_on(self.cancel_token());`
   then push the envelope with that exchange. Do not create a
   `CancellationToken` literal in this crate (`xtask lint-cancel-tokens`).
   The owner is the consumer-lifetime token: a drop caused by route stop
   (owner cancelled) must not cancel the dispatch.
2. After a successful `reply_rx.await` result, call `dispatch.disarm();` before
   returning. On either early return (`ChannelClosed` from the send or the
   reply), let `dispatch` drop — its guard cancels a token no live pipeline
   observes.
3. Do not link or derive from any token already on the exchange: a fresh
   independent token is minted by the helper; the pipeline takes the marker at
   entry, so a completed reply carries none.
4. Extend the `send_and_wait` doc comment: dropping the returned future before
   the reply cancels that dispatch unless the consumer-lifetime token is
   already cancelled (then the drop is shutdown-driven and the ADR-0043 drain
   grace governs the exchange). A completed reply disarms the signal. A nested
   inline `direct:` dispatch starts a fresh dispatch scope and does not inherit
   an outer dispatch cancellation.
5. Leave `send` (fire-and-forget) untouched: no token is minted.
6. In `AGENTS.md` `## COMPONENT LANES`, add
   `camel-component-api | local | cargo test -p camel-component-api` because
   this crate has a `tests/` surface (`polling_consumer_timeout_tests.rs`,
   `runtime_observability_tests.rs`, `traced_log_capture.rs`) and the lane
   sweep would otherwise print `NO LANE`.

**Tests** (in `consumer_claim_tests.rs`)
- name: `send_and_wait_stamps_a_live_dispatch_token`
  - setup: `ConsumerContext` over `mpsc::channel(1)`; a task pops the envelope
    from the receiver.
  - action: spawn `ctx.send_and_wait(exchange)`, pop the envelope.
  - assert: `envelope.exchange.dispatch_cancel()` is `Some` and
    `!is_cancelled()` while the waiter is alive; later reply `Ok` so the task
    ends. Bound every receive/join with `tokio::time::timeout`.
  - command: `cargo test -p camel-component-api --lib send_and_wait_stamps_a_live_dispatch_token`
- name: `dropping_send_and_wait_cancels_the_stamped_token`
  - setup: same context; spawn the waiter, pop the envelope, capture the
    token, then `JoinHandle::abort()` the waiter and
    `let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;`
    (the timeout-bounded join resolves only after the future is dropped).
  - action: after the join, inspect the captured token.
  - assert: the captured token `is_cancelled()`.
  - command: `cargo test -p camel-component-api --lib dropping_send_and_wait_cancels_the_stamped_token`
- name: `completed_send_and_wait_disarms_the_token`
  - setup: same context; reply `Ok(exchange)` on the envelope's `reply_tx`.
  - action: await `send_and_wait`.
  - assert: result is `Ok`; the token captured from the envelope is not
    cancelled.
  - command: `cargo test -p camel-component-api --lib completed_send_and_wait_disarms_the_token`
- name: `reply_carries_no_dispatch_marker`
  - setup: same context; a receiving task calls
    `envelope.exchange.take_dispatch_cancel()` (simulating pipeline entry),
    then replies `Ok(exchange)`.
  - action: await `send_and_wait`.
  - assert: the returned exchange has `dispatch_cancel() == None`, and a
    second `send_and_wait` on it stamps a different live token.
  - command: `cargo test -p camel-component-api --lib reply_carries_no_dispatch_marker`

**Acceptance**
- `cargo test -p camel-component-api --lib` exits 0 (new and existing tests).
- `cargo clippy -p camel-component-api --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-cancel-tokens` exits 0 with no ratchet change.
- `bash scripts/gates-local.sh` reports no `NO LANE` for the touched crates.


- [x] 2 send_and_wait arming
## 3. camel-core: second task-local, run_steps boundary and initial bypass

**Files**
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified: aggregate-loop marker take)
- `crates/camel-core/src/lifecycle/adapters/inline_dispatcher.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_dispatch_cancel_tests.rs` (new)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified: `#[path]` module declaration)

**Steps**
1. In `route_compiler.rs` beside `CANCEL_TOKEN` (line 52), add
   `tokio::task_local! { pub(crate) static DISPATCH_CANCEL: Option<CancellationToken>; }`
   with a comment: per-dispatch trigger, scoped per pipeline call, independent
   of the per-start route-stop token.
2. Add `pub(crate) fn dispatch_cancelled() -> bool`:
   `DISPATCH_CANCEL.try_with(|t| t.as_ref().is_some_and(CancellationToken::is_cancelled)).unwrap_or(false)`.
3. In `run_steps` (line 577), set
   `let cancelled = CANCEL_TOKEN.try_with(CancellationToken::is_cancelled).unwrap_or(false) || dispatch_cancelled();`
   and keep `PipelineOutcome::Failed(CamelError::ConsumerStopping)`. Update the
   comment to name both triggers and state that the check is only before a
   step (no terminal check after the last step).
4. After the step-invocation binding that produces `outcome` (the
   `invoke_with_span`/`invoke` match) and before the `match outcome` block,
   insert:
   `if dispatch_cancelled() && matches!(&outcome, PipelineOutcome::Failed(CamelError::ConsumerStopping)) { return PipelineOutcome::Failed(CamelError::ConsumerStopping); }`
   — a dispatch-cancelled failure is not retried and not absorbed by the
   handler. Do not key this on `CANCEL_TOKEN`.
5. In `route_controller_trait.rs`, at both envelope spawn sites (Concurrent
   ~line 539 and Sequential ~line 622), call
   `let dispatch_cancel = exchange.take_dispatch_cancel();` right after
   destructuring the envelope, and wrap the scoped call as
   `DISPATCH_CANCEL.scope(dispatch_cancel, CANCEL_TOKEN.scope(cancel, async move { pipe.call(exchange).await }))`.
   Import `DISPATCH_CANCEL` from `route_compiler`. **Take**, do not peek.
6. In `inline_dispatcher.rs` (line 169), call
   `let dispatch_cancel = exchange.take_dispatch_cancel();` before the scoped
   call and nest
   `DISPATCH_CANCEL.scope(dispatch_cancel, CANCEL_TOKEN.scope(operation_cancel, async move { pipe.call(exchange).await }))`.
   An independent inline entry with `None` shadows any outer dispatch scope.
7. In `route_controller.rs` aggregate-split loop (destructure at line 1224),
   add `let _dispatch_cancel = exchange.take_dispatch_cancel();` so aggregate
   replies carry no stale marker. Do **not** scope `DISPATCH_CANCEL` there:
   aggregate-split remains out of scope, and one abandoned contributor must
   never cancel work shared by the completed bucket. Comment says exactly that.
8. Add the new test module in `route_controller_tests.rs`:
   `#[cfg(test)] #[path = "route_controller_dispatch_cancel_tests.rs"] mod dispatch_cancel;`
   Copy the capture-component/gated-processor harness from
   `route_controller_drainclaim_tests.rs` (`CaptureComponent` pushes the
   `ConsumerContext`; `DefaultRouteController::new`; `activate_cohort`), and add
   a recording step that increments an `Arc<AtomicU32>` and forwards the
   exchange. The gated processor parks only the ordinal the test names.

**Tests**
- name: `cancelled_dispatch_stops_before_the_next_step`
  - setup: a Sequential `capture:src` route with two steps: step 1 parks on a
    `watch` release, step 2 increments the counter. Start, activate cohort,
    capture the context.
  - action, leg A (real trigger): spawn `ctx.send_and_wait(exchange)`, await
    step-1 entry, abort the waiter and
    `let _ = tokio::time::timeout(Duration::from_secs(2), waiter).await;`
    (the timeout-bounded join resolves only after the wait future is dropped,
    so the guard has fired), then release step 1.
  - assert, leg A: within a bounded `tokio::time::timeout`, the in-flight
    total is 0, the step-2 counter is 0, and step 1 entered exactly once.
  - action, leg B (observable outcome): build
    `let mut ex = test_exchange("manual"); let dispatch = ex.begin_dispatch_cancel();`,
    push `ExchangeEnvelope { exchange: ex, reply_tx: Some(tx), in_flight_claim: None }`
    through `ctx.sender()`, await step-1 entry, `drop(dispatch)`, release step 1.
  - assert, leg B: `timeout(2s, reply_rx)` resolves to
    `Err(CamelError::ConsumerStopping)`, step 2 did not run, gauge is 0.
  - command: `cargo test -p camel-core --lib cancelled_dispatch_stops_before_the_next_step`
  - expected after implementation: pass; before it: step 2 runs.
- name: `uncancelled_dispatch_is_unchanged`
  - setup: a second Sequential `capture:src2` route with two non-parking steps:
    step A increments counter A, step B increments counter B and forwards.
  - action: `ctx.send_and_wait(exchange).await`.
  - assert: `Ok`, counter A == 1, counter B == 1, the reply body equals the
    input body, `reply.dispatch_cancel() == None`, and
    `controller.in_flight_total.total() == 0`.
  - command: `cargo test -p camel-core --lib uncancelled_dispatch_is_unchanged`
- name: `inline_entry_without_marker_shadows_outer_dispatch_scope`
  - setup: a Sequential `capture:direct-src` route with two non-parking
    recording steps; start it and capture the `ConsumerContext`, then read
    `ctx.inline_dispatcher().expect("inline capability")`.
  - action: `outer_token.cancel();` first, then spawn
    `DISPATCH_CANCEL.scope(Some(outer_token.clone()), async move { dispatcher.dispatch(exchange).await })`
    and await it.
  - assert: the dispatch resolves `Ok` and both step counters are 1 — the
    inline entry took `None` from the exchange and shadowed the already-
    cancelled outer scope (an inherited scope would stop before step 1).
  - command: `cargo test -p camel-core --lib inline_entry_without_marker_shadows_outer_dispatch_scope`

**Acceptance**
- All three tests pass; `cargo test -p camel-core --lib` passes (ADR-0043
  suites included).
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-cancel-tokens` exits 0 (no new site).


- [x] 3 camel-core wiring and boundary
## 4. camel-core + camel-processor: retry loop honours dispatch cancellation

**Files**
- `crates/camel-api/src/error_handler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-processor/src/error_handler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_dispatch_cancel_tests.rs` (modified)

**Steps**
1. In `crates/camel-api/src/error_handler.rs`, add a defaulted method to
   `pub trait RetryableStep` (line 134):
   `fn dispatch_cancelled(&self) -> bool { false }` with a doc comment:
   the retry loop consults it between attempts; the default `false` keeps
   every existing implementor unchanged; it must never consult the route-stop
   token.
2. In `route_compiler.rs`, override it on `impl RetryableStep for OwnedRetryable`
   (line 745): `fn dispatch_cancelled(&self) -> bool { dispatch_cancelled() }`.
3. In `crates/camel-processor/src/error_handler.rs` `retry_step` (line 343):
   - before the `for attempt in 0..backoff.max_attempts` loop, if
     `step.dispatch_cancelled()` return
     `RetryOutcome::Exhausted { exchange: original, error: CamelError::ConsumerStopping, policy }`;
   - immediately after `tokio::time::sleep(delay).await` (line 385) and before
     the `original.branch_clone()`, repeat the same check and return;
   - inside the `PipelineOutcome::Failed(retry_err)` arm, when the loop is not
     already returning and `is_business` is false, check
     `step.dispatch_cancelled()` and return the same `Exhausted` with
     `ConsumerStopping`.
   Never abort an active attempt: the hook is read only between attempts.
   Keep the route-stop token entirely out of this file.
4. In `run_steps`, inside the `RetryOutcome::Exhausted { exchange, error, policy }`
   arm (line 653) and before the `handler.handle_step(policy, exchange, error)`
   call, insert:
   `if dispatch_cancelled() && matches!(&error, CamelError::ConsumerStopping) { return PipelineOutcome::Failed(CamelError::ConsumerStopping); }`
   — a dispatch-cancelled exhaustion never reaches the disposition handler.
5. Add the gated retry test to the dispatch-cancel test module.

**Tests**
- name: `dispatch_cancelled_stopping_bypasses_the_error_handler`
  - setup: a Sequential `capture:src3` route with `ErrorHandlerConfig`
    carrying one policy that matches `CamelError::ProcessorError` and retries
    (`RedeliveryPolicy` with `max_attempts: 3` and a delay of at least 100 ms),
    and one step that increments an `Arc<AtomicU32>` on entry, signals the
    test, and returns `Err(CamelError::ProcessorError("retry me"))` on its
    first invocation. The step must never be invoked a second time.
  - action: spawn `send_and_wait`, await the first invocation signal,
    abort + join the waiter (timeout-bounded; dispatch token cancelled) while
    the handler is in its backoff sleep, then wait past the backoff delay with
    `tokio::time::sleep_until` (not `sleep`).
  - assert: the step-entry counter is exactly 1 (the pre-invoke hook stopped
    the redelivery), the in-flight total is 0, and the route stop completes.
  - command: `cargo test -p camel-core --lib dispatch_cancelled_stopping_bypasses_the_error_handler`
  - expected after implementation: pass; without the hook the retry
    re-invokes the step (counter 2).

**Acceptance**
- The new test passes; `cargo test -p camel-processor --lib` passes.
- `cargo clippy -p camel-api -p camel-core -p camel-processor --all-targets -- -D warnings` exits 0.
- The trait addition has a default and changes no existing implementor.


- [x] 4 retry-loop cancellation gate
## 5. camel-core: release proof and route-stop regression

**Files**
- `crates/camel-core/src/lifecycle/adapters/route_controller_dispatch_cancel_tests.rs` (modified)

**Steps**
1. Add a `Concurrent { max: Some(1) }` harness with a **two-step** route
   (`.with_concurrency(ConcurrencyModel::Concurrent { max: Some(1) })`): step 1
   parks on its first invocation only (ordinal 1) and passes later invocations
   straight through; step 2 increments a counter. The release test's second
   exchange passes both steps; the route-stop test relies on the step-2
   boundary check.
2. Release test: spawn the first `send_and_wait`, await the parked entry,
   assert `controller.in_flight_total.total() > 0`, abort + join (timeout-
   bounded) the waiter, release the parked invocation, then submit a second
   `send_and_wait` on the same route and await it.
3. Route-stop regression: clone the route's pipeline token before moving the
   controller into the stop task:
   `let route_token = controller.routes.get(route_id).expect("route").pipeline_cancel_token.clone();`
   Spawn `stop_route_with_budget(route_id, Duration::from_millis(100))`,
   await `timeout(deadline, route_token.cancelled())` (this proves the stop
   path fired the token; do not rely on sleeping past the budget), then
   release the parked step, then await both the stop task and the waiter with `tokio::time::timeout`
   bounds. Assert the stop task returned `Ok`.
4. Shutdown-drain test: use the Sequential two-step route (step 1 parks only
   ordinal 1), a **generous** stop budget (e.g. 2 s) so the drain grace is
   active, a live `send_and_wait` waiter, and the captured `ConsumerContext`.
   Spawn `stop_route_with_budget`; await `ctx.cancel_token().cancelled()`
   (the stop path cancels the consumer-lifetime token first), then abort +
   join the waiter (the guard sees the cancelled owner and must defuse),
   release step 1, and await the stop task with a `tokio::time::timeout`
   bound.

**Tests**
- name: `cancelled_dispatch_releases_the_exchange`
  - setup: Concurrent(max 1) two-step route; the first invocation of step 1
    parks.
  - action: assert positive gauge while parked; abort + join (timeout-bounded)
    the first waiter; release the parked invocation; `send_and_wait` a second
    exchange.
  - assert: the second dispatch returns `Ok` within a bounded timeout, its
    reply `dispatch_cancel() == None`, and
    `controller.in_flight_total.total() == 0` after both settle. There is no
    reply for the abandoned waiter (it was aborted); do not assert one.
  - command: `cargo test -p camel-core --lib cancelled_dispatch_releases_the_exchange`
- name: `route_stop_still_cancels_in_flight_dispatch`
  - setup: Concurrent(max 1) two-step route, live waiter (never dropped)
    parked in step 1, 100 ms shutdown budget.
  - action: spawn `stop_route_with_budget`; await the cloned
    `pipeline_cancel_token.cancelled()` bounded by a timeout; release the
    parked step; await the stop task and the waiter.
  - assert: the waiter resolves with `Err(ConsumerStopping)` within a bounded
    timeout, the step-2 counter is 0, step 1 was invoked once, and the stop
    task returned `Ok`. No dispatch token is cancelled in this test.
  - command: `cargo test -p camel-core --lib route_stop_still_cancels_in_flight_dispatch`
- name: `abandoned_wait_during_route_stop_drains_to_completion`
  - setup: Sequential two-step route (step 1 parks ordinal 1, step 2 counts),
    2 s shutdown budget, live waiter.
  - action: spawn `stop_route_with_budget`; await
    `ctx.cancel_token().cancelled()`; abort + join (timeout-bounded) the
    waiter; release step 1; await the stop task.
  - assert: step 2's counter is 1 (the exchange drained to completion instead
    of being cut by the abandoned wait), step 1 was invoked once, and the stop
    task returned `Ok`.
  - command: `cargo test -p camel-core --lib abandoned_wait_during_route_stop_drains_to_completion`

**Acceptance**
- All three tests pass and the existing route-stop/drain tests stay green.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- No `tokio::time::sleep` in the new tests (`sleep_until` or bounded
  timeouts only); `cargo xtask lint-test-sleep` and
  `cargo xtask lint-unbounded-wait` pass without raising a ratchet.


- [x] 5 release, route-stop and drain tests
## 6. camel-core: TransactedSegment observes the dispatch trigger

**Files**
- `crates/camel-core/src/transacted_segment.rs` (modified)

**Steps**
1. In `TransactedSegment::run_in`, read both tokens:
   `let route_cancel = CANCEL_TOKEN.try_with(CancellationToken::clone).ok();`
   and `let dispatch_cancel = DISPATCH_CANCEL.try_with(|t| t.clone()).ok().flatten();`
   (import `DISPATCH_CANCEL` from `route_compiler`).
2. Add a private
   `struct CancelSurface { route: Option<CancellationToken>, dispatch: Option<CancellationToken> }`
   with `fn is_cancelled(&self) -> bool` and
   `async fn cancelled(&self)` that resolves when either token fires (both
   absent → pending). Replace the current `cancel: Option<&CancellationToken>`
   at the body race and pass the surface to `finalize`.
3. Extend the body race (`cancelled_or_pending`, line ~367) to use
   `CancelSurface::cancelled`. Keep the frozen `biased` order
   `deadline → cancellation → body` unchanged (a tie with the deadline
   resolves as `Deadline`).
4. Extend the R6 commit cutoff (`finalize`, line ~278) to
   `surface.is_cancelled()`. Keep `CamelError::ConsumerStopping` as the
   `BodyEnd::Cancelled` primary. The R7 commit gate and the post-handoff commit
   resolution are untouched: once `commit()` is called, neither token is
   re-consulted.
5. Document in the module header that the new trigger joins the existing
   block-level cancellation arm and finalizes through the frozen 604d
   pseudocode (rollback before the cutoff, detach on uncertainty).

**Tests**
- name: `cancelled_transacted_block_finalizes_without_leaking_a_session`
  - setup: a stateful fake checkout in the test module implementing
    `TransactionManager`/`TransactionSession`: begin `Active` with exactly one
    outstanding lease (an `Arc<AtomicUsize>`); `rollback()` records the call,
    asserts the body was already dropped (an `Arc<AtomicBool>` set by the
    body's drop guard), sets state `Done`, and releases the lease;
    `commit()` records and must never be called. The body is an
    `OutcomePipeline` (the existing `BodyScript`/`OutcomeSegment::new` pattern)
    whose step signals entry then parks on a `watch` release until dropped.
  - action: build `let mut segment = TransactedSegment::new(DS, manager, body);`
    and `let token = CancellationToken::new();`, then
    spawn `DISPATCH_CANCEL.scope(Some(token.clone()), async move { segment.run(exchange).await })`;
    await body entry (bounded); `token.cancel()`; await the join (bounded).
  - assert: outcome is `Failed(ConsumerStopping)`; the fake recorded exactly
    one `"rollback"` and no `"commit"`; the body drop flag was set before the
    rollback; the session state is `Done`; outstanding leases are 0.
  - command: `cargo test -p camel-core --lib cancelled_transacted_block_finalizes_without_leaking_a_session`
  - expected after implementation: pass; before it: no cancellation is
    observed and the body hangs (bounded by the test timeout).

**Acceptance**
- The new test passes; every existing `transacted_segment` test passes
  unchanged (`cargo test -p camel-core --lib transacted_segment`).
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- No change to `sql-transactions` spec text: the existing cancellation-cutoff
  requirement governs the rollback path.

- [x] 6 transacted block cancellation
