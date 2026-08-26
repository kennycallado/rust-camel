# Tasks: cohort-activation-barrier

## camel-core

### Task 1.1: CohortActivationGate primitive + unit tests

**Files:**
- `crates/camel-core/src/lifecycle/cohort_activation.rs` (new)
- `crates/camel-core/src/lifecycle/mod.rs` (modified — add `mod cohort_activation;`)

**Steps:**
1. Create `cohort_activation.rs` defining `pub(crate) struct CohortActivationGate`
   with `open_tx: watch::Sender<bool>` + `open_rx: watch::Receiver<bool>` from
   `tokio::sync::watch::channel(false)` (the `StartupSignal` pattern —
   `watch::channel(StartupState::Pending)` enum variant at
   crates/components/camel-component-api/src/consumer.rs `pair()`; we use a
   plain `bool` since the gate has exactly two states).
2. Implement `new_closed() -> Self`, `open(&self)` (send_if_modified
   false→true, idempotent, no-op when already open), `close(&self)`
   (send_if_modified true→false, idempotent), `is_open(&self) -> bool`
   (borrow-checks the receiver), and `subscribe(&self) -> watch::Receiver<bool>`
   (returns `open_rx.clone()`). NOTE: `watch::Receiver::wait_for` requires
   `&mut self`, so the gate exposes NO `async fn opened(&self)` — waiter
   tasks call `subscribe()` once at spawn, own the returned receiver, and
   await `rx.wait_for(|open| *open)` directly at their call site (level-
   triggered: resolves immediately when already open; cancellation raced at
   the call site).
3. Add `mod cohort_activation;` to `lifecycle/mod.rs`.
3. Write unit tests in the same file (below impl, `mod tests`):
   `open_is_idempotent`, `opened_resolves_immediately_when_open` (subscribe →
   open → `rx.wait_for` polls ready immediately), `close_then_open_cycle`,
   `opened_parks_until_open` (subscribe on a closed gate + spawned task
   awaiting `rx.wait_for`; `tokio::task::yield_now` pump; assert pending
   before `open()` and completes after).

**Tests:** (executable spec)
- `open_is_idempotent`: setup = gate closed → action = `open(); open();` →
  assert = `is_open() == true`, no panic, second call was a no-op.
- `opened_resolves_immediately_when_open`: setup = open gate, subscribed
  receiver → action = poll `rx.wait_for(|o| *o)` once → assert = completes
  without any yield/pump.
- `close_then_open_cycle`: setup = open gate → action = `close()` then assert
  `is_open() == false`, then `open()` → assert = true again (per-boot re-arm).
- `opened_parks_until_open`: setup = closed gate + spawned task awaiting
  `rx.wait_for` on a subscribed receiver → action = yield pump, assert
  pending; `open()` → assert = waiter completes.
  Command: `cargo test -p camel-core --lib cohort_activation`.
  Expected: FAILS before implementation (module absent), PASSES after.

**Acceptance:**
- `cargo test -p camel-core --lib cohort_activation` exits 0 (4 tests).
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `rg -n 'watch::channel\(false\)' crates/camel-core/src/lifecycle/cohort_activation.rs`
  matches (watch primitive, not AtomicBool+Notify).

- [x] 1.1

### Task 1.2: Shared gate on controller + port methods reset_cohort/activate_cohort

**Files:**
- `crates/camel-core/src/lifecycle/application/ports/route_ordering_port.rs` (modified — trait lives here)
- `crates/camel-core/src/lifecycle/adapters/route_ordering_impl.rs` (modified — `RouteOrderingPort` impl delegating to the handle)
- `crates/camel-core/src/lifecycle/adapters/controller_actor_commands.rs` (modified — `RouteControllerHandle` struct at :188 gains `cohort: Arc<CohortActivationGate>` field + `pub(crate) fn cohort_gate()` accessor)
- `crates/camel-core/src/lifecycle/adapters/controller_actor.rs` (modified — `spawn_controller_actor` at :379 clones `controller.cohort` into the handle BEFORE the controller is moved into the actor; ALSO update the 6 test-only `RouteControllerHandle { tx }` construction literals in THIS file at :609, :631, :878, :887, :935, :982 to pass a fresh `new_closed()` gate)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified — `DefaultRouteController` state)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified — construction wiring)

**Steps:**
1. In `application/ports/route_ordering_port.rs`: add TWO required async
   methods to the `RouteOrderingPort` trait (crate-private, single
   implementor — no default bodies): `async fn reset_cohort(&self);` and
   `async fn activate_cohort(&self);` (match the trait's prevailing
   async-trait declaration style used by `auto_startup_route_ids`).
2. In `DefaultRouteController` (state struct in
   `adapters/route_controller.rs`): add `cohort: Arc<CohortActivationGate>`
   constructed `new_closed()` at controller construction
   (route_controller_trait.rs construction path). NO controller-actor
   command-enum changes: the actor is NOT involved in reset/activate (see
   step 3).
3. `RouteControllerHandle` (struct in `controller_actor_commands.rs`:188,
   currently holds only `tx`) gains `cohort: Arc<CohortActivationGate>` —
   the SAME Arc as the controller. In `spawn_controller_actor`
   (`controller_actor.rs`:379) clone `controller.cohort` into the handle
   BEFORE the controller moves into the actor task; update the 6 test-only
   handle construction literals in `controller_actor.rs` (:609, :631, :878,
   :887, :935, :982) to pass a fresh `new_closed()` gate. `reset_cohort`/`activate_cohort` (impl in
   `route_ordering_impl.rs`) call `gate.close()`/`gate.open()` DIRECTLY on
   the shared Arc (watch send is synchronous and non-async — no actor
   round-trip; methods stay async for trait-shape consistency). Rationale:
   actor-queue indirection would deadlock when the actor is legitimately
   parked (Task 1.5's Explicit-handshake hold), and direct shared-state
   manipulation preserves idempotency.
4. Add a `pub(crate) fn cohort_gate(&self) -> Arc<CohortActivationGate>`
   accessor on `RouteControllerHandle` — test observability probe
   (`is_open()`), no public API growth.

**Tests:** (executable spec — module `route_ordering_port_gate`, names
contain the filter)
- `route_ordering_port_gate_reset_then_activate_roundtrip`: setup =
  controller + handle sharing the gate → action = `reset_cohort().await;
  activate_cohort().await;` → assert = `handle.cohort_gate().is_open() ==
  true`.
- `route_ordering_port_gate_activate_idempotent`: setup = open gate →
  action = `activate_cohort().await` twice → assert = no error, still open.
- `route_ordering_port_gate_reset_rearms`: setup = open gate → action =
  `reset_cohort().await` → assert = `is_open() == false`.
  Command: `cargo test -p camel-core --lib route_ordering_port_gate`.
  Expected: FAILS before (trait methods absent → compile error), PASSES
  after.

**Acceptance:**
- `cargo test -p camel-core --lib route_ordering_port_gate` exits 0 (3
  tests actually collected — verify the count in output, not just exit 0).
- `rg -n 'async fn (reset_cohort|activate_cohort)' crates/camel-core/src/lifecycle/application/ports/route_ordering_port.rs`
  matches both, no default body.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: Gate the three consumer-envelope drain sites

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)

**Steps:**
1. Thread the gate into the three drain-spawn sites as a SUBSCRIBED
   receiver: at each spawn site call `gate.subscribe()` once (same capture
   pattern as `pipeline_cancel`); the spawned task owns the
   `watch::Receiver<bool>` (mutable — `wait_for` requires `&mut`).
2. Site `route_controller_trait.rs:416` (Concurrent branch): after the
   envelope recv select (~:414-421) and before `strict_dispatch_denies`,
   add: `tokio::select! { _ = rx.wait_for(|o| *o) => {} , _ =
   pipeline_cancel.cancelled() => { /* drop envelope; reply_tx resolves to
   ChannelClosed on drop */ return; } }` where `rx` is the owned subscribed
   receiver.
3. Site `route_controller_trait.rs:488` (Sequential/`_` branch): identical
   placement after its recv select (~:486-493), same owned-receiver await.
4. Site `route_controller.rs:1047` (restart aggregate drain, `envelope_opt`
   branch): identical placement after the recv match arm (~:1047-1050),
   racing that loop's existing cancel token.
5. Do NOT touch the late branch `route_controller.rs:1034` (`late_rx`
   aggregator output — transitively post-activation; add a one-line comment
   there stating why it is ungated, citing the papal D3 ruling).
6. Dropped-envelope semantics: on the cancel arm, `reply_tx` (if any) is
   dropped → `send_and_wait` waiter resolves `Err(ChannelClosed)` — existing
   mapping, no new error variant.

**Tests:** (executable spec — module `drain_gate`, names contain the filter;
gate-behavior integration, full F8 in 1.5)
- `drain_gate_concurrent_parks_until_activation`: setup = route with
  `?concurrent=2` consumer + gate closed + envelope sent into the channel →
  action = yield pump → assert = mock/pipeline observer sees NO dispatch;
  `open()` gate → assert = dispatch occurs.
- `drain_gate_sequential_parks_until_activation`: same shape on a default
  (Sequential) route.
- `drain_gate_restart_parks_until_activation`: same shape on the restart path
  (stop→start a route with a held gate).
- `drain_gate_parked_exits_on_cancel`: setup = parked envelope + closed gate →
  action = cancel the pipeline token → assert = drain task exits (JoinHandle
  resolves) and send_and_wait waiter got `ChannelClosed`.
  Command: `cargo test -p camel-core --lib drain_gate`.
  Expected: FAILS before (no gate), PASSES after.

**Acceptance:**
- `cargo test -p camel-core --lib drain_gate` exits 0 (4 tests).
- `rg -n 'late_rx' crates/camel-core/src/lifecycle/adapters/route_controller.rs`
  shows the ungated comment near :1034.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 1.3

### Task 1.4: start_context wiring — reset at entry, activate on every return

**Files:**
- `crates/camel-core/src/lifecycle/application/context_lifecycle.rs` (modified)

**Steps:**
1. In `start_context` (:42): after the `cancel_token` reset (:52), call
   `route_controller.reset_cohort().await` (the `&dyn RouteOrderingPort`
   parameter already reaches here).
2. Refactor the remainder (services loop, startup checks, reconciliation,
   `auto_startup_route_ids`, the sequential StartRoute loop :97-105) into a
   captured result: `let result: Result<(), CamelError> = inner_startup().await;`
   where `inner_startup` is a local async block / fn containing the existing
   body — NO early `?` returns after the reset.
3. After the capture: unconditionally `route_controller.activate_cohort().await`
   (log-on-error at most, never propagate an activation failure over the
   original result), then `result`.
4. Ground the per-boot re-arm with a comment citing route_registry.rs:95
   (auto_startup flag → stop/start re-issues StartRoute).

**Tests:** (executable spec — module `start_context_gate`, names contain the
filter; gate observability via `handle.cohort_gate().is_open()`)
- `start_context_gate_boot_failure_still_activates`: setup = context whose
  FIRST service start returns Err (pre-loop failure) → action =
  `ctx.start().await` → assert = returns Err AND
  `cohort_gate().is_open() == true`.
- `start_context_gate_boot_success_activates`: setup = normal two-route
  context → action = start → assert = gate open.
- `start_context_gate_second_boot_rearms`: setup = started context → action =
  stop, then start again → assert = gate observed closed at entry (probe
  ordering: reset fires before the services) and open after the second boot.
  Command: `cargo test -p camel-core --lib start_context_gate`.
  Expected: FAILS before, PASSES after.

**Acceptance:**
- `cargo test -p camel-core --lib start_context_gate` exits 0 (3 tests).
- `rg -n '\?' crates/camel-core/src/lifecycle/application/context_lifecycle.rs`
  shows NO bare `?` between the reset and the capture block (all returns
  funnel through the capture).
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 1.4

### Task 1.5: F8 regression test (positive + negative control) — in-crate

**Files:**
- `crates/camel-core/src/lifecycle/adapters/cohort_activation_regression.rs` (new — in-crate `#[cfg(test)]` module, sibling file of `route_controller_tests.rs`; register it in `route_controller.rs` using the SAME idiom as the existing sibling at :1208-1210 — the attribute form is `#[cfg(test)] #[path = "route_controller_tests.rs"] mod tests;` — add `#[path = "cohort_activation_regression.rs"] mod cohort_activation_regression;` beside it under the same cfg(test) attribute)

AMENDED (rc-iuuk): the original hold — blocking the sync `emit_start_route_event` hook — std-blocks a pooled tokio worker on the controller actor task and the multi-thread scheduler nondeterministically (~40-50%) never polls sibling actor-spawned tasks. The hold is now B's EXPLICIT startup handshake: the actor parks ASYNCHRONOUSLY in `await_consumer_startup`, so sibling tasks keep scheduling.

**Steps:**
1. Route B uses a test-controlled EXPLICIT consumer: a minimal in-crate consumer (implement the consumer trait the way camel-core in-crate tests do — see consumer_management.rs test consumers) whose `start()` awaits a test-controlled async release signal (e.g. `tokio::sync::Notify` or a `watch<bool>` owned by the test) and calls `ctx.mark_ready()` ONLY after release. NO blocking inside any sync hook; NO `emit_start_route_event` usage.
2. Route A = Immediate consumer (timer, deterministic first tick) that sets an INGRESS LATCH (an "emitted" flag latched when its first exchange is sent) and whose pipeline step executes `runtime.execute(RuntimeCommand::StopRoute { route_id: B, ... })`, recording the result in a shared slot (`Arc<Mutex<Option<Result<...>>>>`) and setting a dispatch-observation flag (`AtomicBool`) BEFORE the execute call.
3. Boot order: A starts before B (route ordering — name/startup_order so A's consumer is spawned first; A's drain parks on the closed gate). The actor reaches B's start and parks in `await_consumer_startup` (async park — A's consumer and drain keep being polled).
4. Positive path `cohort_regression_parks_first_dispatch_until_cohort_completes`: boot with B's handshake held; await the INGRESS LATCH (A's consumer sets an "emitted" AtomicBool/oneshot when it sends its first exchange — generous deadline await, so the absence assert below can never pass vacuously before A even emitted); then after a short grace window (300ms) assert the dispatch-observation flag is STILL false (A's exchange received-but-parked, gate closed); release B's handshake (`mark_ready` fires); `start()` returns; assert observation flag true AND the recorded StopRoute result is Ok (B reaches Stopped, no invalid-transition).
5. Negative control `cohort_regression_ungated_simulation_shows_the_rejection`: hold B's handshake, open the gate DIRECTLY via `handle.cohort_gate().open()` (shared Arc — bypasses the parked actor), let A's exchange dispatch while B is held; assert the recorded StopRoute result is Err whose message contains "invalid transition" (B's aggregate is at Starting during the hold — Phase-1 persist precedes runtime execution — so the class is Starting→Stopped; assert the class, not the exact source state; rejection site: commands.rs:337-348 pre-validation + route_runtime.rs:118-129 state machine, worker-validated).
6. Module doc-comment: rc-ava7/rc-jxkj lineage + why the test lives in-crate (pub(crate)/pub(super) surface) + why the hold is the Explicit handshake (rc-iuuk: sync-hook hold std-blocks a worker).

**Tests:** (executable spec — the file IS the deliverable)
- `cohort_regression_parks_first_dispatch_until_cohort_completes` (positive): setup = held-B (Explicit handshake) two-route context → action = boot + grace + release → assert = parked-not-dispatched during hold, then dispatched + StopRoute Ok after cohort completes.
- `cohort_regression_ungated_simulation_shows_the_rejection` (negative): setup = same + early direct `cohort_gate().open()` → assert = recorded Err contains "invalid transition".
  Command: `cargo test -p camel-core --lib cohort_regression`.
  Expected: positive FAILS before the barrier exists (exchange dispatches during B's hold → rejection); negative PASSES both before and after.

**Acceptance:**
- `cargo test -p camel-core --lib cohort_regression` exits 0 (2 tests collected), stable across ≥10 consecutive runs (the rc-iuuk flake class must NOT resurface — the async-park hold is the whole point).
- The positive test fails when run against a build with the gate-site guards reverted (verify once via a local minimal revert of the Task 1.3 guard edits, run, restore; document in the task report; do NOT commit the revert).

- [x] 1.5

### Task 1.6: Docs — CONTEXT-MAP Key-Term + camel-core CONTEXT.md note

**Files:**
- `CONTEXT-MAP.md` (modified)
- `crates/camel-core/CONTEXT.md` (modified)

**Steps:**
1. CONTEXT-MAP.md Key-Terms: add "Cohort Activation Barrier" — definition:
   the context-lifecycle mechanism that parks the first consumer-envelope
   dispatch of each route drain from `start_context` entry until the
   sequential StartRoute cohort completes (or the boot returns, whichever
   first); level-triggered via watch<bool>; per-boot re-arm.
2. crates/camel-core/CONTEXT.md: add an architecture note (matching the
   file's existing note style) under the lifecycle section: the barrier's
   three gated drain sites (Concurrent trait branch, Sequential `_`
   branch, restart aggregate envelope branch), the ungated late branch and
   why, the port pair reset_cohort/activate_cohort, and the
   activate-on-every-return rule.

**Tests:** (docs — lint-verifiable)
- `cargo xtask lint-context-citations` exits 0 (CONTEXT.md format rules
  hold).
- Manual check: `rg -n "Cohort Activation Barrier" CONTEXT-MAP.md` matches.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- Both doc edits present and in the files' prevailing English/style.

- [x] 1.6
