# Design: ws-stop-drain-budget

## Approach

The configured drain budget (`camel-config` `drain_timeout_ms`, default 10 s)
travels the existing graceful-stop chain and lands in a per-consumer stop-budget
slot that `WsConsumer::stop` reads. This is the same channel shape the runtime
already uses for the in-flight gauge (drainclaim): a slot installed on
`ConsumerContext` at consumer start, captured by the consumer.

1. **Slot channel.** `spawn_consumer_task` (camel-core `consumer_management.rs`)
   creates a `watch::channel::<Option<Duration>>(None)` pair. The receiver is
   installed on the `ConsumerContext` via a new builder (`with_stop_budget_slot`);
   the sender is stored on the route's `ManagedRoute` for the current consumer
   incarnation. Start/resume build fresh pairs, so a new incarnation never
   inherits a stale budget.
2. **Budget producers.** Writers publish synchronously BEFORE cancelling the
   consumer token:
   - Graceful context stop: `CamelContext` gains `drain_timeout: Duration`
     (default 10 s, documented as mirroring `camel-config`'s default), wired in
     `configure_context` from `config.drain_timeout_ms`. `stop_context` sends
     `RuntimeCommand::StopRoute { stop_budget: Some(drain_timeout), .. }`.
     Note: this is a NEW propagation for the drain knob; `stop_timeout`'s
     existing ignored `timeout_ms` argument stays untouched (separate knob,
     separate promise).
   - Registry/reload stop: `route_registry::stop_route` keeps
     `DEFAULT_SHUTDOWN_TIMEOUT` and writes `Some(DEFAULT_SHUTDOWN_TIMEOUT)` —
     explicit, so reload behavior is pinned to today's 5 s, not coupled to the
     ws fallback.
   - Suspend: writes nothing; the suspended incarnation's slot reads `None`.
3. **Forwarding layers.** `stop_budget: Option<Duration>` is added to
   `RuntimeCommand::StopRoute` (camel-api, `#[non_exhaustive]`, in-repo
   constructors only) and to the internal
   `RouteControllerCommand::StopRoute`, threaded through the actor forwarding
   layers (`commands.rs`, `apply_runtime_lifecycle`, `RuntimeExecutionPort`,
   `RuntimeExecutionAdapter`). The controller gains an override path
   (`stop_route_with_budget`) so the public `RouteController::stop_route` trait
   signature is unchanged; `stop_route_internal` writes the slot before
   driving the stop and otherwise behaves identically.
4. **Consumer side.** `WsConsumer` captures the slot receiver at start (next to
   the gauge capture) on `self` — NOT on the shared `WsAppState`, which is
   shared across routes on one server. In `stop()` it snapshots the value and
   drops the borrow before awaiting: budget = slot value, else
   `WS_STOP_SETTLE_TIMEOUT_FALLBACK` (the renamed 5 s const, doc comment no
   longer describing a known gap). Fail-open expiry (warn + proceed) and the
   warn fields are untouched.
5. **Tests.** camel-ws unit tests with `tokio::time::pause`:
   (a) budget 500 ms, gauge held — stop completes ~500 ms (not 5 s), fail-open
   warn observed; (b) budget 10 s, gauge released by a paused-clock task at 6 s —
   stop completes ~6 s, no fail-open warn (the upward case that is broken
   today). Plus a real-IO echo test with a small configured budget (echo
   delivered through the wire, teardown clean), reload/suspend regression
   assertions, and the runtime propagation test at the `stop_route_internal`
   seam. The literal (5 s, 10 s] wire case is covered semantically by (b);
   a multi-second wall-clock test is out of bounds per mission order.

## Affected crates

- `camel-component-api`: `ConsumerContext` slot field + builder + accessor.
- `camel-api`: `RuntimeCommand::StopRoute` field; `RouteController` provided
  method `stop_route_with_budget` (default delegates to `stop_route`).
- `camel-core`: slot pair creation/install, `ManagedRoute` sender storage,
  slot write in `stop_route_internal`, `CamelContext::drain_timeout` + wiring,
  `stop_context` budget production, forwarding-layer threading, registry
  explicit `Some(DEFAULT_SHUTDOWN_TIMEOUT)`.
- `camel-ws`: capture slot at start, settle bound from slot-or-fallback,
  const rename + docs, tests.
- `camel-cli`: `RuntimeCommand::StopRoute` construction-site updates only.

## Architecture boundaries

Control-plane lifecycle plumbing stays in camel-core lifecycle adapters; the
only component change is camel-ws consuming the slot. No DSL, language, or
function-layer changes. The drain contract of ADR-0043 is preserved: consumer
stop still completes before the route drain wait; only the ws settle bound's
source changes. The `Consumer` trait (34 implementors) is unchanged — the
provided `RouteController` method keeps test fakes compiling.

## Phases

Single-phase change: the seam (slot + propagation) and the consumer-side
reading are one coherent slice — splitting them would leave either dead
plumbing or a consumer reading an empty channel. Tasks are ordered
seam-first, consumer-last, with the two-directional red/green tests inside
the final task.
