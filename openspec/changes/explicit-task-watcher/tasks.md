# Tasks: explicit-task-watcher

TDD discipline: every task that introduces tests writes them FIRST and
verifies red (compile-fail counts as red for not-yet-widened APIs), then
implements until green. Checked bounded awaits throughout: never discard a
`timeout()` result — use `.expect("...") // allow-unwrap: test-only`.

## camel-core (consumer_management.rs)

### Task 1.1: Outer outcome cell, termination guard, and outer-task watcher (tests first)

**Files:**
- `crates/camel-core/src/lifecycle/adapters/consumer_management.rs` (modified)

**Steps:**
1. RED — add three synthetic unit tests in `mod tests` (they reference
   types that don't exist yet → compile-red is the verified red):
   `outer_task_watcher_silent_when_accounted`,
   `outer_task_watcher_silent_when_cancelled`,
   `outer_task_watcher_publishes_when_pending`. Each: build
   `OuterOutcomeCell::new()` + `CancellationToken` + RecordingRuntime
   harness (:1457-1483) recorder; oneshot pair; construct
   `OuterWatcherInputs`; `let watcher = spawn_outer_task_watcher(inputs);`
   fire (drop the tx / pre-cancel the token / `mark_accounted()` then drop
   tx); then
   `tokio::time::timeout(Duration::from_secs(2), watcher).await.expect("watcher must complete") // allow-unwrap: test-only`;
   assert recorder contents (0 / 0 / exactly 1 FailRoute whose `error`
   contains "terminated abnormally"). Verify red:
   `cargo test -p camel-core --lib outer_task_watcher` → compile errors.
2. Add above `ImmediateWatcherInputs` (~line 200):
   ```rust
   /// Shared task↔watcher outcome cell: false = Pending (nobody accounted
   /// this task's termination), true = Accounted (a body path published its
   /// failure, or it completed normally through the finally-stop). Relaxed
   /// ordering suffices: the TerminationGuard's oneshot send provides the
   /// happens-before edge for the watcher's read.
   #[derive(Clone)]
   pub(crate) struct OuterOutcomeCell(Arc<AtomicBool>);

   impl OuterOutcomeCell {
       pub(crate) fn new() -> Self { Self(Arc::new(AtomicBool::new(false))) }
       pub(crate) fn mark_accounted(&self) {
           self.0.store(true, Ordering::Relaxed);
       }
       pub(crate) fn is_accounted(&self) -> bool {
           self.0.load(Ordering::Relaxed)
       }
   }
   ```
3. Add the drop guard (fires on normal return, panic unwind, and abort —
   tokio abort drops the future, running Drop):
   ```rust
   /// Fires the oneshot exactly once when the outer Explicit task ends,
   /// in every termination mode. `Option` + `take()` because
   /// `oneshot::Sender::send` consumes `self` (cannot move out of `&mut self`).
   pub(crate) struct TerminationGuard {
       tx: Option<oneshot::Sender<()>>,
   }

   impl Drop for TerminationGuard {
       fn drop(&mut self) {
           if let Some(tx) = self.tx.take() {
               let _ = tx.send(());
           }
       }
   }
   ```
4. Add the watcher inputs (mirrors `ImmediateWatcherInputs` shape; NO
   `command_id` field — `publish_runtime_failure` stamps its own
   sole-publisher id via `fail_command_id`, route_helpers.rs:285-321):
   ```rust
   /// Everything the detached outer-task watcher needs, produced once by
   /// `spawn_consumer_task` for Explicit-class consumers.
   pub(crate) struct OuterWatcherInputs {
       /// Fired by the task-local TerminationGuard on any task end.
       pub terminated: oneshot::Receiver<()>,
       pub outcome: OuterOutcomeCell,
       /// Cancelled stop → the stop flow owns termination; silent exit.
       pub consumer_cancel: CancellationToken,
       pub route_id: String,
       pub runtime: Option<Weak<dyn RuntimeHandle>>,
       pub crash_notifier: Option<mpsc::Sender<CrashNotification>>,
   }
   ```
5. Add the watcher, modeled on `spawn_failure_watcher`/`run_failure_watcher`
   (:221-288) but event-driven (no grace, no polling):
   ```rust
   pub(crate) fn spawn_outer_task_watcher(inputs: OuterWatcherInputs) -> JoinHandle<()> {
       tokio::spawn(run_outer_watcher(inputs))
   }

   async fn run_outer_watcher(inputs: OuterWatcherInputs) {
       // (a) Await termination — the ONLY wait; no timeout (the guard
       // fires when the task ends, however long it runs).
       if inputs.terminated.await.is_err() {
           // Sender dropped without firing (cannot happen with the guard;
           // defensive) — treat as accounted, stay silent.
           return;
       }
       // (b) Cancelled stop owns termination — silent.
       if inputs.consumer_cancel.is_cancelled() {
           return;
       }
       // (c) Accounted (failure already published by the body, or normal
       // completion) — silent. Sole-publisher discipline.
       if inputs.outcome.is_accounted() {
           return;
       }
       // (d) Pending + not cancelled = abnormal termination (panic/abort
       // that no body path accounted for). Standard trio:
       let error_msg = format!(
           "Consumer outer task terminated abnormally (panic or abort): {}",
           inputs.route_id
       );
       // log-policy: system-broken
       error!(route_id = %inputs.route_id, "{error_msg}");
       if let Some(tx) = inputs.crash_notifier
           && tx
               .send(CrashNotification {
                   route_id: inputs.route_id.clone(),
                   error: error_msg.clone(),
               })
               .await
               .is_err()
       {
           warn!(route_id = %inputs.route_id, "CrashNotification channel closed; crash will not be restarted");
       }
       publish_runtime_failure(inputs.runtime, &inputs.route_id, &error_msg).await;
   }
   ```
6. GREEN — run `cargo test -p camel-core --lib outer_task_watcher` → 3 passed.

**Tests:**
- `outer_task_watcher_silent_when_accounted`: terminated fired + `mark_accounted()` → recorder has 0 FailRoute commands; watcher join verified.
- `outer_task_watcher_silent_when_cancelled`: token cancelled + Pending → recorder has 0 FailRoute commands; watcher join verified.
- `outer_task_watcher_publishes_when_pending`: terminated fired + Pending + not cancelled → exactly 1 FailRoute, `error` contains "terminated abnormally"; watcher join verified.
- Command: `cargo test -p camel-core --lib outer_task_watcher`. Expected: red at step 1 (compile), green at step 6 (3 passed).

**Acceptance:**
- `cargo test -p camel-core --lib outer_task_watcher` exits 0 (3 passed).
- `cargo fmt --check` exits 0.
- NOTE: full `cargo clippy -p camel-core -- -D warnings` is NOT a gate at
  this task — `spawn_outer_task_watcher` has no non-test caller yet
  (dead_code). Clippy becomes a gate at Task 1.5. Do NOT add
  `#[allow(dead_code)]` (lint-ignore flags it).

- [x] 1.1

### Task 1.2: Core-outcome integration tests (red) + fixtures

**Files:**
- `crates/camel-core/src/lifecycle/adapters/consumer_management.rs` (modified — `mod tests` only)

**Steps:**
1. Add fixture `ExplicitReadyConsumer`:
   ```rust
   struct ExplicitReadyConsumer {
       stop_panics: bool,
       bg: Option<tokio::task::JoinHandle<Result<(), CamelError>>>,
   }
   ```
   `startup_mode` → `ConsumerStartupMode::Explicit`; `start(ctx)`:
   `ctx.mark_ready(); Ok(())`; `stop`: if `stop_panics` →
   `panic!("stop exploded")` else `Ok(())`; `background_task_handle`
   (`&mut self`, returns `Option<JoinHandle<Result<(), CamelError>>>` —
   consumer.rs:519) → `self.bg.take()`.
2. Add fixture `ParkedExplicitConsumer` (no bg): `start(ctx)`:
   `ctx.mark_ready();` then `std::future::pending::<()>().await` (never
   returns). `stop` → `Ok(())`.
3. RED — add four tests (RecordingRuntime harness :1487-1535 idiom; test
   owns the cancel token: `ConsumerContext::new(tx, token.clone(), id)`;
   destructure the PLANNED 4-tuple
   `let (handle, startup_rx, _watcher_inputs, outer_inputs) = spawn_consumer_task(...)`
   → compile-red until Task 1.4 widens the return):
   - `outer_task_watcher_failroute_on_panic_in_stop` —
     ExplicitReadyConsumer `{ stop_panics: true, bg: None }`: body panics
     in finally-stop; `handle.await` returns `Err` (JoinError — assert);
     spawn watcher from `outer_inputs`; watcher completes (checked
     timeout); recorder has exactly 1 FailRoute with
     "terminated abnormally".
   - `outer_task_watcher_silent_on_normal_completion` —
     `{ stop_panics: false, bg: None }`: handle joins Ok; 0 commands.
   - `outer_task_watcher_silent_on_normal_completion_with_bg` —
     `{ stop_panics: false, bg: Some(tokio::spawn(async {
     Ok::<(), CamelError>(()) })) }`: handle joins Ok; 0 commands.
   - `outer_task_watcher_silent_on_cancel` — ParkedExplicitConsumer:
     after watcher spawned, `token.cancel(); handle.abort();` watcher
     completes; 0 commands.
   All `startup_rx.await_ready().await` must resolve Ok (latched
   mark_ready — consumer.rs:87-96/:147-162, first-signal-wins).
4. Verify red: `cargo test -p camel-core --lib outer_task_watcher` →
   compile errors in the four new tests only.

**Tests:**
- The four tests above (specs inline in step 3).
- Command: `cargo test -p camel-core --lib outer_task_watcher`. Expected: compile-red (4-tuple destructure) until Task 1.4.

**Acceptance:**
- Red verified and reported (compile errors on the 4 new tests; the 3 synthetic tests from 1.1 still pass in isolation).
- `cargo fmt --check` exits 0.

- [x] 1.2

### Task 1.3: Subtle-outcome tests (red): uncancelled abort + duplicate suppression

**Files:**
- `crates/camel-core/src/lifecycle/adapters/consumer_management.rs` (modified — `mod tests` only)

**Steps:**
1. RED — `outer_task_watcher_failroute_on_uncancelled_abort`:
   ParkedExplicitConsumer (fixture from 1.2); test owns the token;
   `spawn_consumer_task(..., Some(crash_tx), Some(runtime_weak), false)`
   with `let (crash_tx, mut crash_rx) = mpsc::channel(1);`;
   await_ready Ok; spawn watcher; `handle.abort();` WITHOUT cancelling;
   watcher completes (checked timeout); recorder has exactly 1 FailRoute
   "terminated abnormally"; AND assert the crash notification:
   `tokio::time::timeout(Duration::from_secs(2), crash_rx.recv()).await`
   yields `Some(CrashNotification { route_id, .. })` with matching
   route_id (spec.md requires the crash-notification channel, not just
   FailRoute).
2. RED — `outer_task_watcher_no_double_publish_when_stop_panics_after_bg_publish`:
   ExplicitReadyConsumer `{ stop_panics: true, bg: Some(spawn(async {
   Err(CamelError::RouteError("bg died".into()) })) }`: bg monitor
   publishes (:537) and accounts; finally-stop panics; handle.await Err;
   watcher completes; recorder has exactly 1 FailRoute and its `error`
   contains "bg died" (NOT "terminated abnormally") — sole-publisher
   regression.
3. Verify red: compile errors on the two new tests only.

**Tests:**
- The two tests above (specs inline).
- Command: `cargo test -p camel-core --lib outer_task_watcher`. Expected: compile-red until Task 1.4.

**Acceptance:**
- Red verified (compile errors on the 2 new tests; 3 synthetic still green).
- `cargo fmt --check` exits 0.

- [x] 1.3

### Task 1.4: Implementation — body accounting, guard, 4-tuple widening (green)

**Files:**
- `crates/camel-core/src/lifecycle/adapters/consumer_management.rs` (modified)

**Steps:**
1. In `spawn_consumer_task`'s `_` arm (:470-577), BEFORE `tokio::spawn`
   and following the Immediate arm's pre-spawn-clone idiom (:327-329),
   create ALL of:
   `let outer_outcome = OuterOutcomeCell::new();`
   `let outer_outcome_for_inputs = outer_outcome.clone();`
   `let (term_tx, term_rx) = oneshot::channel::<()>();`
   `let crash_notifier_for_inputs = crash_notifier.clone();`
   `let route_id_for_inputs = route_id.clone();`
   `let runtime_for_inputs = runtime_for_consumer.clone();`
   `let consumer_cancel_for_inputs = consumer_ctx.cancel_token();`
   (consumer.rs:272 — captured pre-spawn; the originals move into the
   `async move` block at :475, so the inputs literal CANNOT clone them
   after the move — that is why every input has a pre-spawn binding).
2. Inside the spawned async block, as its FIRST statement:
   `let _guard = TerminationGuard { tx: Some(term_tx) };`
   (named binding — `let _ =` would drop it immediately; the guard must
   live to the end of every body path and fire on unwind/abort; nothing
   fallible precedes it — `consumer.start()` sits after).
3. Account the Err path (:477-505): `outer_outcome.mark_accounted();`
   immediately AFTER `publish_runtime_failure(runtime_for_consumer, ...)`
   (:501) and BEFORE `consumer.stop().await` (:503) — a panic in that stop
   must NOT re-publish.
4. Account the bg-monitor publish arms: after
   `publish_runtime_failure(...)` at :537 (bg `Ok(Err(e))` arm) and at
   :557 (bg `Err(join_err)` arm), add `outer_outcome.mark_accounted();`
   before falling through to the finally-stop.
5. Account the normal end: after the final
   `let _ = consumer.stop().await;` (:573), add
   `outer_outcome.mark_accounted();` — the shutdown-quiescent arms
   (:539-541, :559-564) and the bg-cancel arm (:567-569) fall through
   here, so every non-publishing exit is accounted after stop.
6. Replace `(handle, receiver, None)` (:576) with:
   ```rust
   let outer_inputs = OuterWatcherInputs {
       terminated: term_rx,
       outcome: outer_outcome_for_inputs,
       consumer_cancel: consumer_cancel_for_inputs,
       route_id: route_id_for_inputs,
       runtime: runtime_for_inputs,
       crash_notifier: crash_notifier_for_inputs,
   };
   (handle, receiver, Some(outer_inputs))
   ```
   (`is_resume` does not affect the inputs.)
7. Immediate arm: append `None` — `(handle, startup_receiver, Some(inputs), None)`
   (:463). Widen the fn signature (:297-301) to
   `(JoinHandle<()>, StartupReceiver, Option<ImmediateWatcherInputs>, Option<OuterWatcherInputs>)`.
8. Fix ALL remaining existing 3-tuple destructures (the six tests from
   1.2/1.3 already destructure 4-way) by appending `_outer_inputs`:
   - consumer_management.rs tests (13): :903, :931, :989, :1019, :1061,
     :1303, :1385, :1504, :1571, :1641, :1813, :1875, :1933
   - handshake_tests.rs (5): :133, :196, :246, :284, :352
   - route_controller.rs PRODUCTION aggregate-start call site (1):
     :1142-1150 — same widening; its watcher wiring lands in Task 1.5
     (a `_outer_inputs` bind is acceptable ONLY until then).
9. GREEN — `cargo test -p camel-core --lib outer_task_watcher` → 9 passed
   (3 synthetic + 6 integration); full lib green.

**Tests:**
- All 9 `outer_task_watcher` tests flip green (command: `cargo test -p camel-core --lib outer_task_watcher`).
- Existing suite regression: `cargo test -p camel-core --lib` exits 0 (accounting must not alter any observable path; all widened destructures compile).

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `rg 'mark_accounted' crates/camel-core/src/lifecycle/adapters/consumer_management.rs` shows exactly 4 body call sites (after :501, :537, :557, :573) plus tests.
- NOTE: clippy is NOT a gate at this task (watcher still production-caller-less — dead_code); it becomes a gate at Task 1.5.

- [x] 1.4

## camel-core (controller wiring + real-bus E2E)

### Task 1.5: Real-bus E2E (red) then watcher wiring at all three production call sites (green)

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)

**Steps:**
1. Add fixture trio (idiom: `RcKh7cFailBindConsumer/Endpoint/Component`
   at route_controller_tests.rs:2955-3012): `PanickingAfterReadyConsumer`
   — `startup_mode` → Explicit; `start(ctx)`: `ctx.mark_ready();
   panic!("consumer exploded after readiness");`; `stop` → `Ok(())`.
   `PanickingAfterReadyEndpoint::create_consumer` vends it (scheme
   `"panicready"`); `PanickingAfterReadyComponent` vends the endpoint.
2. RED — add the test, modeled on
   `immediate_consumer_error_transitions_route_to_failed` (:3327-3381):
   register the component in a fresh Registry;
   `wired_bus_and_controller(registry).await` (:3250);
   `register_route_via_bus(&bus, RouteDefinition::new("panicready:x",
   vec![]).with_route_id("panic-after-ready-e2e")).await;`
   `bus.execute(RuntimeCommand::StartRoute { ... })` → assert Ok
   (mark_ready latched BEFORE the panic — first-signal-wins,
   consumer.rs:87-96); then
   `poll_route_status(&bus, "panic-after-ready-e2e", "Failed").await`
   (:3286). RUN it now: with no watcher wiring it must FAIL (route stays
   Started; poll bound 2s) — this is the mandatory red that reproduces
   the exact defect this change fixes (Started-with-dead-handle).
   Record the red output.
3. Wire the start path — route_controller_trait.rs (:578-586): 4-way
   destructure
   `let (consumer_handle, startup_rx, watcher_inputs, outer_inputs) = ...;`
   after the Immediate-watcher conditional (:619-621) add:
   ```rust
   // Detached outer-task watcher for Explicit consumers (rc-a7rh):
   // spawned only after the handshake resolved Ok — rollback
   // terminations (abort-then-cancel above) happen before this point
   // and are never watched.
   if let Some(outer) = outer_inputs {
       consumer_management::spawn_outer_task_watcher(outer);
   }
   ```
4. Wire the resume path (:794-802): same 4-way destructure; after the
   watcher_inputs conditional (:819-821), the same conditional spawn.
5. Wire the aggregate start path — route_controller.rs:1142-1150 (third
   PRODUCTION call site, aggregate routes): replace the `_outer_inputs`
   bind from Task 1.4 with `outer_inputs`; after the Immediate-watcher
   conditional (:1170-1172), the same conditional
   `spawn_outer_task_watcher(outer)` spawn (before the wrapper spawn at
   :1178). Explicit consumers on aggregate routes get identical
   whole-lifetime coverage — the spec has no aggregate carve-out.
6. GREEN — rerun the E2E: `poll_route_status(... "Failed")` passes.
7. Regression: full controller + aggregate suites green.

**Tests:**
- `outer_task_watcher_panic_after_ready_transitions_route_to_failed`: wired real RuntimeBus + controller actor → StartRoute Ok → GetRouteStatus polls to "Failed" within 2s. Red before steps 3-5 (stays Started), green after.
- Command: `cargo test -p camel-core --lib outer_task_watcher` — 10 passed.
- Regression: `cargo test -p camel-core --lib route_controller` and `cargo test -p camel-core --lib` exit 0.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0 (first clippy gate — all three watcher spawns make every new symbol referenced).
- `cargo fmt --check` exits 0.
- Exactly three production watcher spawns: `rg -n 'spawn_outer_task_watcher' crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs crates/camel-core/src/lifecycle/adapters/route_controller.rs` → 3 hits (start, resume, aggregate).

- [x] 1.5
