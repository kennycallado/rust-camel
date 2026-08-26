# Design: cohort-activation-barrier

## Approach

Implements the e_opus papal v3 pre-blessing (2026-08-25) with one documented,
adjudication-requested deviation (gated-site count; see §2).

### 1. Primitive — `CohortActivationGate` (camel-core, private)

```rust
pub(crate) struct CohortActivationGate {
    open_rx: watch::Receiver<bool>,   // shared clone into drain tasks
    open_tx: watch::Sender<bool>,     // shared via Arc; opener role = the port handle (see §3)
}
impl CohortActivationGate {
    fn new_closed() -> Self;                       // watch seeded false
    fn open(&self);                                // send_if_modified false→true, idempotent
    fn close(&self);                               // per-boot re-arm (start_context entry)
    fn subscribe(&self) -> watch::Receiver<bool>;  // clone for drain tasks — wait_for needs &mut,
                                                   // so waiter tasks own a receiver and await
                                                   // rx.wait_for(|open| *open) at the call site
    fn is_open(&self) -> bool;                     // test/debug observable
}
```

`watch::channel::<bool>(false)` — the `StartupSignal` pattern
(crates/components/camel-component-api/src/consumer.rs uses
`watch::channel(StartupState::Pending)`, an enum; the gate has exactly two
states so a plain `bool`). Level-triggered: every waiter sees the current
value on first poll; no lost-wakeup window. The gate is shared as
`Arc<CohortActivationGate>` between the controller (which clones receivers
into drain tasks) and the port handle (which owns the opener role for the
context lifecycle).

### 2. Guard — gate the dispatch of consumer envelopes at THREE sites

The gate-await sits AFTER `rx.recv()` resolves, INSIDE the loop body, racing
the site's existing cancellation (R2):

```
let envelope = select! { rx.recv() / cancel => ... };   // unchanged
select! {                                              // cohort_rx = owned subscribed receiver
    _ = cohort_rx.wait_for(|open| *open) => {}
    _ = pipeline_cancel.cancelled() => { drop envelope(reply_tx→ChannelClosed); return; }
}
... dispatch unchanged (strict-mode check, pipeline call, reply_tx) ...
```

Sites (consumer `ExchangeEnvelope` dispatch — all three):
- `route_controller_trait.rs:416` — `ConcurrencyModel::Concurrent` branch
  (permit-then-dequeue loop; gate after the recv select, racing the same
  `pipeline_cancel`).
- `route_controller_trait.rs:488` — Sequential / forward-compat `_` branch.
- `route_controller.rs:1047` — restart-path aggregate drain, `envelope_opt`
  branch.

NOT gated (ruling D3): `route_controller.rs:1034` late branch — `late_rx`
carries aggregator OUTPUT (`mpsc::channel::<Exchange>`, :1000) fed only after
a consumer envelope traversed pre-pipeline → aggregator; transitively
post-activation. Gating it would be dead code and a self-deadlock risk.

DEVIATION from pre-blessing §III ("exactly two"): the Concurrent branch
(:416) is added as a third gated site. Verified on main: it receives consumer
`ExchangeEnvelope` via `rx.recv()` and dispatches the pipeline. Leaving it
ungated lets `?concurrent=` routes bypass activation — the topology-bypass
the ruling's own principle forbids. Flagged for the spec-blessing expert to
adjudicate (accept the third site, or rule the Concurrent branch safe-ungated
with evidence).

### 3. Ownership & wiring (D2)

- `DefaultRouteController` constructs the gate CLOSED at construction and
  stores `cohort: Arc<CohortActivationGate>` in its state.
- `start_route` / restart path clone a receiver into each spawned drain task.
- The `RouteControllerHandle` (the port's single implementor) holds the SAME
  `Arc` shared with the controller at construction. TWO required async port
  methods on `RouteOrderingPort`: `reset_cohort()` (close, idempotent) and
  `activate_cohort()` (open, idempotent) — implemented by calling
  `gate.close()`/`gate.open()` DIRECTLY on the shared Arc (watch send is
  synchronous and non-async; no actor-queue dependency). Rationale:
  actor-queue acknowledgement would deadlock whenever the controller actor
  is legitimately blocked (e.g. the F8 test's startup hold), and direct
  shared-state manipulation preserves the idempotent semantics. A
  `pub(crate)` `cohort_gate()` accessor on the handle provides test
  observability (`is_open()`).

### 4. Per-boot lifecycle (corrected R6) & failure semantics (D1)

Port contract (concrete, no defaults — `RouteOrderingPort` is crate-private
with a single implementor `RouteControllerHandle`, so required methods break
nothing): TWO required async methods, implemented as DIRECT shared-Arc calls
(see §3 — no actor-queue round-trip):
- `async fn reset_cohort(&self)` — closes the shared gate (idempotent).
- `async fn activate_cohort(&self)` — opens the shared gate (idempotent).

`start_context` (context_lifecycle.rs:42):
1. At entry — alongside the `cancel_token` reset (:52) — `reset_cohort()`.
   Ground: `auto_startup_route_ids` filters on the static `auto_startup` flag
   (route_registry.rs:95), so stop→start re-runs StartRoute for all
   auto-startup routes; drains re-spawn against a fresh closed gate.
2. Run the rest of startup (services, checks, reconciliation, the sequential
   StartRoute loop).
3. D1 — open on EVERY return after the reset, not only loop outcomes: a Rust
   `Drop` guard cannot await an async port call, so the pattern is explicit:
   capture the startup result (`let result = { ...startup... };`), ALWAYS
   `activate_cohort().await` (even when the result is Err — service-startup
   failures, validation, reconciliation, route-id listing, and the StartRoute
   loop all return through this single post-reset funnel), then return the
   original `result`. This guarantees: a stale drain left by a failed stop
   cannot strand — every post-reset return releases it; already-Started
   routes keep draining (today's partial-up semantics); the failed route is
   Failed (compensated at commands.rs:516) and opening for it is harmless.

Stop/start safety (verified by reviewer): successful StopRoute joins/aborts
the drain; `stop_context` awaits each command; actor serialization prevents
old/new drain overlap — the unconditional activation covers the failed-stop
residue case.

Hot-reload (single route, context up): the gate is open; zero added latency.
Only a full stop→start re-arms it.

### 5. F8 regression test (camel-core, IN-CRATE unit test)

In-crate (the `#[cfg(test)]` hooks and pub(crate) surface are invisible from
`tests/`; the test lives beside route_controller_tests.rs). Two routes:
A = Immediate consumer (deterministic first-tick emission) whose pipeline
executes a RuntimeBus StopRoute for B and records the result; B = route with
a test-controlled EXPLICIT consumer whose `start()` awaits a test-controlled
async release signal and calls `mark_ready()` only after release — the
controller actor parks ASYNCHRONOUSLY in `await_consumer_startup` (the
rc-slvd-sanctioned shape). Hold rationale (rc-iuuk): the earlier design
blocked the sync `emit_start_route_event` hook, which std-blocks a pooled
tokio worker on the actor task and makes the multi-thread scheduler
nondeterministically strand sibling tasks (~40-50%). With the handshake
hold, B's aggregate sits at Starting (Phase-1 persist precedes runtime
execution), so the ungated simulation's rejection is Starting→Stopped —
same invalid-transition class, asserted by class not source state
(commands.rs:337-348 pre-validation, route_runtime.rs:118-129 state machine).
Positive: A's envelope received-but-not-dispatched (grace-window absence
assert on the observation flag) while the gate is closed; release B; boot
completes; A's parked exchange dispatches and its StopRoute(B) SUCCEEDS.
Negative control (no production bypass, no actor-queue deadlock): hold B,
open the gate DIRECTLY via the handle's `cohort_gate()` accessor (shared Arc
— bypasses the parked actor), let A's exchange dispatch mid-hold, and assert
the recorded StopRoute Err contains "invalid transition".

Unit tests for the primitive: open idempotency; `wait_for` immediate when
open; close→open cycle; parked dispatch resolves via cancel (R2).

## Affected crates

- camel-core: `CohortActivationGate` (src/lifecycle/, placement per plan); 3
  drain-site guards; controller state + shared-Arc port wiring; `start_context`
  entry-reset and open-on-both-paths; F8 + unit tests.
- Docs: CONTEXT-MAP.md Key-Term "Cohort Activation Barrier" (R5);
  crates/camel-core/CONTEXT.md architecture note (R5).

## Architecture boundaries

Pure camel-core, data-plane dispatch point. No component API change, no
RuntimeBus/validation change, no handshake change. Control plane (context
boot) drives data-plane activation through the existing port seam — no
reverse dependency. The port extension is additive (`activate_cohort`), not a
behavioral change to ordering.

## Phases

Single-phase: primitive + guards + wiring + opener + tests form one coherent
slice; a primitive-only phase would ship dead code with no test exercising it.
