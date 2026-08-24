# Proposal: seda-restart-fix

## Why

bd rc-gwvs. A seda Single-mode endpoint cannot restart its consumer: `SedaConsumer::start`
takes the one-shot mpsc receiver out of `SedaMode::Single { rx }` and moves it into the
forwarder tasks; `stop()` aborts the forwarders but never restores the receiver into the
endpoint state. On the next `ctx.stop()` / `ctx.start()` cycle (route restart or resume),
the recreated consumer's `start()` finds the `rx` slot empty and fails with
`endpoint '<name>' already has a registered consumer`.

The failure is also silent: seda is an Immediate-startup consumer, and
`spawn_consumer_task` (camel-core `consumer_management.rs`) pre-resolves the startup
receiver to Ready for Immediate consumers, so the controller's `await_consumer_startup`
returns Ok before the consumer task runs. The route reports Started while
`has_active_consumers()` stays false; producers then fence every send with
"SEDA endpoint has no active consumers". Repro: consumer route `from seda:out` + a send
route, stop, restart, send — nothing flows and nothing fails loudly.

Discovered while testing divert route restart (advice-route-interception Task 5), where
the test worked around it with `multipleConsumers=true` (Fanout registers a fresh
subscriber per start).

## What Changes

- **Fix (camel-component-seda)**: restore the Single-mode receiver on `stop()`. The
  forwarder-shared receiver becomes `Arc<tokio::sync::Mutex<Option<Receiver>>>` so
  `stop()` can take it back (envelopes still inside the receiver's queue when
  restoration occurs survive; already-dequeued or in-flight envelopes keep the
  existing best-effort shutdown behavior) and put it into the endpoint state's `rx`
  slot for the next consumer start. Forwarders treat `None` as shutdown.
- **Tests (camel-component-seda)**: unit tests for stop→start on a fresh consumer
  instance (no "already has a registered consumer" error, `has_active_consumers`
  true again), still-queued envelope preservation across restart, and
  concurrent-consumers restart. Update the existing stop-unregisters test if it
  asserts the old one-shot receiver semantics.
- **Route-level regression test (camel-core)**: the ticket repro — seda consumer route +
  send route, stop, restart, send, expect delivery — replacing the
  `multipleConsumers=true` workaround where the restart semantics are what the test
  actually exercises.
- **Docs**: seda CONTEXT.md lifecycle section records the receiver-restoration contract.

**Excluded**: propagating Immediate-consumer start errors through the startup handshake
(camel-core `spawn_consumer_task` pre-resolved receiver). That is a broader contract
change to rc-w1u9 startup semantics affecting every Immediate consumer (timer, file,
sql, direct, …); filed as bd follow-up **rc-slvd** linked to rc-gwvs.

## Acceptance criteria

- Single-mode seda endpoint survives any number of stop/start cycles; each start
  succeeds and `has_active_consumers()` reflects the active consumer.
- Envelopes still inside the receiver's queue at stop-time restoration are delivered
  by the restarted consumer's forwarders.
- Route-level restart repro test passes with default `multipleConsumers=false`.
- Fanout mode behavior unchanged; existing seda tests pass unmodified except where
  they assert the one-shot receiver defect.
- Follow-up bd ticket **rc-slvd** covers the Immediate-consumer silent-start-error
  contract.

## Risk budget

Low. Change confined to camel-component-seda internals plus tests; no public API, no
URI options, no Fanout path, no core handshake semantics. Accepted behavior change:
envelopes still queued inside the receiver when restoration occurs now survive
restart, while dequeued/in-flight envelopes retain the existing best-effort shutdown
behavior — documented in CONTEXT.md.
