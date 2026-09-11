# Proposal: seda-startup-activation

## Why

SEDA consumers report `ConsumerStartupMode::Immediate`. The route
controller pre-resolves the startup handshake at spawn time, so
`ctx.start()` returns before the spawned consumer task runs
`SedaConsumer::start()` and publishes its activation state
(`active.store(true)` in Single mode, subscriber registration in
Fanout mode). A producer that sends immediately after a successful
`ctx.start()` can hit the pre-enqueue gate
`EndpointCreationFailed("SEDA endpoint X has no active consumers")`
(camel-component-seda lib.rs, producer pre-enqueue check).

The race was stabilized test-side in rc-xzc9 (bounded retry and raw
probe enqueue). The probe pattern has since spread to three suites
(`route_interception/common.rs`, `route_interception/divert.rs`,
`camel-cli` test runner), which meets the proliferation condition
recorded in bd rc-dbrkr. The source-level fix is now due.

## What Changes

- `SedaConsumer` overrides `startup_mode()` to
  `ConsumerStartupMode::Explicit`.
- `SedaConsumer::start()` calls `ConsumerContext::mark_ready()` after
  the activation state is published (Single: `active.store(true)` and
  receiver taken; Fanout: subscriber registered) and before returning
  `Ok(())`. Early-error paths keep returning `Err` before readiness,
  which the runtime converts to route-start failures via
  `mark_failed`.
- `CamelContext::start()` now returns only after every SEDA consumer
  of an auto-started route is activated (Explicit handshake).
- Excluded: removing the existing test-side probe/retry helpers
  (defense in depth; separate concern), any producer-side gate
  change, Fanout reserve semantics.

Affected crates: `camel-component-seda` (primary), `camel-core`
(handshake await path, no code change expected — `await_consumer_startup`
already handles Explicit consumers). bd: rc-dbrkr.

## Acceptance criteria

- After `ctx.start()` returns `Ok`, a producer send to a SEDA endpoint
  of a started route is enqueued without hitting the pre-enqueue
  no-active-consumers gate (no retry/probe needed).
- A SEDA consumer `start()` failure before readiness surfaces as a
  route-start failure (no hang, no silent drop).
- Single-mode stop/start restart cycles still pass the existing
  restartability suite; Fanout mode behavior is unchanged.
- Delta spec under `specs/seda-component/spec.md` validates.

## Risk budget

Startup timing change for every SEDA route: the handshake wait is
bounded by `start()` itself (mutex take + task spawns, no I/O), so
the added latency is one scheduler turn. Deadlock risk is bounded by
the watch-based `StartupSignal` and the runtime's defensive fallback
for consumers that return `Ok` without `mark_ready`. Out of bounds:
changing producer gate semantics, Fanout reserve counting, or the
test-side probe helpers.
