# Design: seda-restart-fix

## Approach

Single-mode restart is broken because the one-shot `mpsc::Receiver` is moved out of the
endpoint state on first start and never returned. Fix by making the receiver recoverable:

1. **Forwarder-shared receiver becomes optional**: the `Arc<tokio::sync::Mutex<Receiver>>`
   shared by the forwarder tasks becomes
   `Arc<tokio::sync::Mutex<Option<mpsc::Receiver<ExchangeEnvelope>>>>`. The forwarder
   loop locks, matches `guard.as_mut()`:
   - `Some(rx)` → existing `select! { rx.recv(), cancel.cancelled() }` on the inner receiver.
   - `None` → forwarder exits `Ok(())` (stop took the receiver back).
2. **Consumer retains a handle**: `SedaConsumer` stores
   `shared_rx: Option<Arc<AsyncMutex<Option<Receiver>>>>` (set on Single-mode start,
   `None` for Fanout).
3. **`stop()` restores**: after `cancel_token.cancel()` and aborting forwarder handles,
   Single-mode stop takes the receiver back and publishes it in a fixed order to close
   the concurrent stop/start race:
   1. `active.store(false)` first. A start that observes the endpoint now either sees
      the `rx` slot still empty (returns the existing "already has a registered
      consumer" error — the same window as today's pre-start state) or, after step 2,
      finds the receiver and proceeds cleanly.
   2. lock `shared_rx`, `take()` the receiver, store it into the endpoint state's
      `rx: Mutex<Option<Receiver>>` slot.
   The order matters for the reverse mistake: if the receiver were published BEFORE
   clearing `active`, a concurrent start could take the receiver, set `active=true`,
   and spawn forwarders — and the stop side's later `active.store(false)` would then
   overwrite the flag, fencing producers against a live consumer. Flag-first prevents
   that: any start that acquires the receiver does so only after `active` is false,
   so its own `active.store(true)` is the final write.
   Envelopes still inside the receiver's queue at restoration survive (the receiver is
   moved, not drained); already-dequeued or in-flight envelopes keep the existing
   best-effort shutdown behavior.

Ordering safety: `handle.abort()` triggers at the forwarder's next `.await`; a forwarder
holding the async-mutex guard when aborted drops the guard on cancellation, so the
stop-path `lock().await` acquires after the dying task releases. Envelope handoff to the
pipeline can no longer be in progress because `forward_envelope` runs outside the guard.

Next start (fresh consumer instance from the same endpoint state, as route
restart/resume builds per CONTEXT.md "ConsumerRestart" / "Suspended Route") finds
`rx` repopulated: no "already has a registered consumer" error, forwarders spawn again,
`active` flips true, producers unfence.

Fanout is untouched: subscribers already re-register per start.

## Affected crates

- `camel-component-seda`: `SedaMode::Single` forwarder-shared receiver type, `SedaConsumer`
  field + `stop()` restore path, tests, CONTEXT.md lifecycle note.
- `camel-core`: route-level restart regression test only (test infra already supports
  seda routes — see `route_interception_test.rs` workaround this change removes).

## Architecture boundaries

Component-internal fix: no `camel-api` contract change, no Consumer/Endpoint trait
change, no Runtime handshake change. `has_active_consumers()` semantics unchanged.
Respects ADR-0007 (crash supervision surface identical — same handles aborted) and the
SEDA staging contract in `components/camel-component-seda/CONTEXT.md` (stop remains
best-effort for in-flight replies per ADR-0004 scope note; the queue itself is now
preserved across restart, documented as the new contract).

Out of scope (separate bd follow-up **rc-slvd**, linked to rc-gwvs):
Immediate-consumer start-error propagation through `await_consumer_startup`
(camel-core `consumer_management.rs` pre-resolved receiver) — affects all Immediate
consumers, an rc-w1u9 contract decision.

## Test contracts

Component-level (`crates/components/camel-component-seda/src/lib.rs` test module):

1. `single_consumer_restart_restores_receiver` — create endpoint state via
   `SedaConfig::from_uri("seda:restart1")`; consumer A starts (Ok), stops (Ok);
   consumer B (fresh instance, same state) starts → assert `Ok(())` and
   `state.has_active_consumers()` true. Repeat the cycle 3×.
2. `single_consumer_restart_preserves_buffered_envelopes` — start A over a capacity-1
   blocked ConsumerContext retained unread; push e1/e2/e3 while A runs; sleep so e1
   parks in the context channel, e2 is dequeued in-flight, e3 remains queued; stop A,
   start B on a clone of the same ConsumerContext sender; assert e1 and the
   still-queued e3 arrive on the retained receiver; never assert in-flight e2.
3. `single_consumer_concurrent_restart` — `concurrentConsumers=4`: start/stop A, start
   B → assert `forwarder_count() == 4` and post-restart sends deliver.
4. Existing `test_seda_consumer_stop_unregisters` — update only if it asserts the
   one-shot receiver defect (error on second start); otherwise leave untouched.
   `test_seda_duplicate_single_consumer` guards the LIVE-instance second start.
5. Producer fencing across the restart cycle is folded into test 1 (producer send
   fenced while stopped — pinned by the existing `test_seda_consumer_stop_unregisters`
   — and unfenced after B starts, asserted at the tail of test 1).

Route-level (`crates/camel-core`, next to the existing route-restart test patterns):

6. `seda_single_consumer_survives_context_restart` (camel-test, `tests/seda_test.rs`) —
   harness context (direct+seda+mock), `from seda:out` consumer route +
   `direct:in → seda:out` send route; start, stop/start cycle through the locked
   underlying `CamelContext` (NOT `h.stop()`/`h.start()` — `h.stop()` latches the
   harness `stopped` flag permanently and would neutralize final teardown), send one
   exchange via a direct producer → assert mock:result receives exactly that exchange;
   end with `h.stop()` for deterministic teardown. Default `multipleConsumers=false`;
   remove the `multipleConsumers=true` workaround in
   `divert_survives_route_stop_and_restart` (camel-core route_interception/divert.rs).
7. `fanout_consumer_restart_cycle` (component crate) — fanout stop→start on a fresh
   consumer keeps registering a fresh subscriber queue; guards the "Fanout mode
   unaffected" scenario.


## Phases

Single-phase: one coherent fix (~2 files + tests). No milestone grouping needed.
