# Tasks: seda-restart-fix

## camel-component-seda

### Task 1.1: Restore Single-mode receiver on stop (fix + unit tests)

**Files:**
- `crates/components/camel-component-seda/src/lib.rs` (modified)

**Steps:**
1. In `SedaMode::Single` consumer start (around line 600): change the forwarder-shared
   receiver from `Arc<AsyncMutex<mpsc::Receiver<ExchangeEnvelope>>>` to
   `Arc<AsyncMutex<Option<mpsc::Receiver<ExchangeEnvelope>>>>` — construct it as
   `Arc::new(AsyncMutex::new(Some(receiver)))` after the existing
   `rx_guard.take()` (line ~610).
2. Rework the Single-mode forwarder loop body (lines ~625-639): after
   `let mut guard = shared_rx.lock().await;`, match `guard.as_mut()` — `Some(rx)` runs
   the existing `tokio::select! { env = rx.recv() => env, _ = cancel.cancelled() => return Ok(()) }`
   guarded recv; `None` returns `Ok(())` (stop took the receiver back). Keep
   `forward_envelope(&ctx, envelope).await` outside the guard, exactly as today.
3. Add field `shared_rx: Option<Arc<AsyncMutex<Option<mpsc::Receiver<ExchangeEnvelope>>>>>`
   to `SedaConsumer` (line ~557), initialized `None` in `new()` (line ~570), set to
   `Some(Arc::clone(&shared_rx))` in the Single branch of `start()`, left `None` for
   Fanout.
4. In `stop()` Single branch (line ~687): after `cancel_token.cancel()` and the
   forwarder-handle abort loop, in this exact order —
   a. `active.store(false, Ordering::SeqCst)` first (race closure per design.md step 3),
   b. if `let Some(shared_rx) = self.shared_rx.take()`: `let receiver =
      shared_rx.lock().await.take();` and if `Some(rx)` store it back into the endpoint
      state slot: `*rx_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(rx);`
      where `rx_slot` is the state's `Mutex<Option<Receiver>>` matched from
      `self.state.mode`.
5. Leave Fanout branches, `has_active_consumers`, producer fencing, and
   `background_task_handle` untouched.
6. Verify existing tests that assert current behavior still hold:
   `test_seda_consumer_stop_unregisters` (producer fencing after stop — unchanged
   semantics), `test_seda_duplicate_single_consumer` (~line 1239, LIVE-instance
   second-start guard — still errors), and all fanout tests. These four existing
   tests own the MODIFIED misc-correctness scenarios and must stay green unmodified:
   `test_seda_concurrent_forwarders_count` (four forwarders + handles + concurrency
   model), `test_seda_concurrent_parallel_processing` (InOut parallel timing),
   `test_seda_concurrent_consumers_one_still_single` (single forwarder at 1),
   `test_seda_concurrent_consumers_hint` (concurrency_model report); lock-not-held
   is additionally pinned by `single_consumer_concurrent_restart` delivery.

**Tests:** (executable spec — name, arrange, act, assert)
- `single_consumer_restart_restores_receiver`: arrange the endpoint state directly —
  `let state = Arc::new(SedaEndpointState::new(&SedaConfig::from_uri("seda:restart1")?))`
  (`has_active_consumers` lives on the state, not on `Box<dyn Endpoint>`); consumers via
  `SedaConsumer::new(Arc::clone(&state), consumer_id, rt())` with distinct ids.
  Consumer A starts Ok, stops Ok; fresh consumer B starts → assert `Ok(())` (no
  "already has a registered consumer") and `state.has_active_consumers()` true; repeat
  the full stop/start cycle on fresh instances 3× — every start returns Ok. After B's
  start, also send one exchange through a producer created from an `SedaEndpoint`
  over the same state (`create_producer`) → assert the send returns Ok (fenced while
  stopped, unfenced after restart, default mode).
  Command: `cargo test -p camel-component-seda single_consumer_restart_restores_receiver`.
  Expected: fails before the fix (B's start returns the registered-consumer error).
- `single_consumer_restart_preserves_buffered_envelopes`: endpoint `seda:restart2`;
  consumer A starts with a `ConsumerContext` built over a capacity-1 mpsc channel whose
  receiver the test retains but does NOT read (blocked context — forwarder deliveries
  park once the channel is full). While A runs, push 3 identifiable envelopes ("e1",
  "e2", "e3") directly through the Single-mode `tx` matched from the endpoint state
  (in-crate access, bypasses producer fencing); sleep ~100ms so the forwarder dequeues
  "e1" (parks in the context channel buffer) and "e2" (parks awaiting send — dequeued,
  in flight) while "e3" REMAINS QUEUED in the receiver. Stop A; start fresh consumer B
  on the same state, giving it a `ConsumerContext` built over a clone of the SAME mpsc
  sender wired to the retained receiver (so B's forwarder delivers into the channel
  the test drains); drain the retained context receiver with a timeout → assert "e1"
  and "e3" arrive ("e3" is the still-queued envelope that survived restoration; "e2"
  is in-flight at stop and may be dropped per the best-effort contract — do NOT assert
  on it).
  Command: `cargo test -p camel-component-seda single_consumer_restart_preserves_buffered_envelopes`.
  Expected: fails before the fix (B cannot start).
- `single_consumer_concurrent_restart`: endpoint
  `seda:restart3?concurrentConsumers=4`; A starts (assert `forwarder_count() == 4`),
  stops; fresh B starts → assert Ok, `forwarder_count() == 4`, and an envelope sent
  post-restart (via producer after B active) is delivered.
  Command: `cargo test -p camel-component-seda single_consumer_concurrent_restart`.
  Expected: fails before the fix at B's start.
- `single_second_start_while_active_still_errors`: endpoint `seda:restart4`; consumer
  A starts; a second fresh consumer C starts while A is active → assert
  `EndpointCreationFailed` containing "already has a registered consumer" (regression
  guard: restoration did not loosen the single-active-consumer invariant).
  Command: `cargo test -p camel-component-seda single_second_start_while_active_still_errors`.
  Expected: passes before AND after the fix.
- `fanout_consumer_restart_cycle`: endpoint `seda:fanrestart?multipleConsumers=true`;
  follow the retained-rx pattern of `test_seda_fanout_two_consumers` (lib.rs ~1274):
  consumer A starts, stops; fresh consumer B starts with a `ConsumerContext` whose
  mpsc receiver the test retains → assert B's start is Ok, a producer send delivers
  the exchange on B's retained receiver (fanout stop→start keeps registering a fresh
  subscriber queue — regression guard for the "Fanout mode unaffected" scenario).
  Command: `cargo test -p camel-component-seda fanout_consumer_restart_cycle`.
  Expected: passes before AND after the fix.

**Acceptance:**
- `cargo test -p camel-component-seda` exits 0 (new + existing, including fanout,
  stop-unregisters, and the four concurrent-delivery owners above).
- `cargo clippy -p camel-component-seda --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- `cargo xtask lint-unwrap` exits 0 (lock handling uses
  `unwrap_or_else(|e| e.into_inner())` per crate convention; no new `unwrap()`).

- [x] 1.1

## camel-core + camel-test

### Task 2.1: Route-level restart regression tests (drop the fanout workaround)

**Files:**
- `crates/camel-core/tests/route_interception/divert.rs` (modified)
- `crates/camel-test/tests/seda_test.rs` (modified)

**Steps:**
1. In `divert.rs` `divert_survives_route_stop_and_restart` (line ~493): change
   `let consumer_uri = "seda:out?multipleConsumers=true";` to
   `let consumer_uri = "seda:out";` and rewrite the doc comment (lines ~485-492): the
   divert now survives restart through the DEFAULT single-consumer mode because stop
   restores the receiver (bd rc-gwvs fix); remove the "pre-existing seda limitation"
   sentence. Keep the rest of the test body (stop → restart → send → assert arrival +
   tap copy) unchanged.
2. In `seda_test.rs`, add test `seda_single_consumer_survives_context_restart`
   (follow the file's `CamelTestContext` idiom):
   - builder `.with_direct().with_seda().with_mock().build()`
   - consumer route: `RouteBuilder::from("seda:out").route_id("consumer-route").to("mock:result")`
   - send route: `RouteBuilder::from("direct:in").route_id("send-route").to("seda:out")`
   - `h.start()`, then perform the stop/restart cycle through the locked underlying
     context — NOT via `h.stop()`/`h.start()` — because `h.stop()` permanently sets the
     harness `stopped` flag (harness.rs:278-284) and would make the final teardown
     no-op: `let ctx = h.ctx().lock().await; ctx.stop().await.unwrap();
     ctx.start().await.unwrap(); drop(ctx);`
   - send one exchange into `direct:in` holding a fresh lock across the whole
     resolve/create/send sequence (harness exposes `Arc<Mutex<CamelContext>>`):
     resolve the `direct` component from the registry, `create_endpoint("direct:in",
     &ctx)`, `create_producer` (same pattern as `send_to_direct` in
     `camel-core/tests/route_interception/common.rs` lines 60-88),
     `oneshot(Exchange::new(Message::new("after-restart")))`;
   - assert `h.mock().get_endpoint("result")` receives exactly 1 exchange with body
     "after-restart" (`await_exchanges(1, timeout)` + `assert_exchange_count(1)`);
   - end the test with `h.stop().await` (first harness-level stop — effective
     deterministic teardown since the flag is still false).

**Tests:**
- `divert_survives_route_stop_and_restart` (modified, default mode): context with
  intercept on plain `seda:out`; stop → restart → send → assert both the real arrival
  (mock:arrival) and the tap copy (mock:tap) deliver "after-restart".
  Command: `cargo test -p camel-core --test route_interception_test divert_survives_route_stop_and_restart`.
  Expected: fails before Task 1.1 (silent start failure → no arrival), passes after.
- `seda_single_consumer_survives_context_restart` (new): harness context as in Steps;
  stop → restart → send → assert mock:result holds exactly the one post-restart
  exchange.
  Command: `cargo test -p camel-test --test seda_test seda_single_consumer_survives_context_restart`.
  Expected: fails before Task 1.1, passes after.

**Acceptance:**
- `cargo test -p camel-core --test route_interception_test` exits 0.
- `cargo test -p camel-test --test seda_test` exits 0.
- `cargo clippy -p camel-core --tests -- -D warnings` exits 0 and
  `cargo clippy -p camel-test --tests -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 2.1

## docs

### Task 3.1: Record the receiver-restoration contract in seda CONTEXT.md

**Files:**
- `crates/components/camel-component-seda/CONTEXT.md` (modified)

**Steps:**
1. In the `## Lifecycle` section, after the sentence describing `stop()` clearing the
   active Consumer or fanout subscriber registration, add: Single-mode `stop()` also
   restores the queue receiver into the endpoint state (clearing the active flag
   first), so a fresh Consumer on the same endpoint can start again — route stop/start
   and resume cycles work in default mode. Envelopes still queued inside the receiver
   when restoration occurs survive the cycle and are delivered after restart;
   already-dequeued or in-flight envelopes keep the existing best-effort shutdown
   behavior (no in-flight drain at stop).
2. Do not touch the Staging model, Concurrency, `#[non_exhaustive]`, or Related
   decisions sections.

**Tests:** (documentation change — verification is lint-based)
- `cargo xtask lint-context-citations` exits 0 (CONTEXT.md still satisfies citation
  policy).
- Visual check: the new sentences use canonical terms (Consumer, endpoint, restart)
  per `crates/components/CONTEXT.md` language.

**Acceptance:**
- `git diff crates/components/camel-component-seda/CONTEXT.md` shows only the
  Lifecycle-section addition.
- `cargo xtask lint-context-citations` exits 0.
- `cargo fmt --check --all` exits 0 (no code touched; guard against accidental edits).

- [x] 3.1
