# Tasks: ws-stop-drain-budget

## Seam types (camel-component-api, camel-api)

### Task 1.1: Public seam types — stop-budget slot, StopRoute budget field, controller provided method

**Files:**
- `crates/components/camel-component-api/src/consumer.rs` (modified)
- `crates/camel-api/src/runtime.rs` (modified)
- `crates/camel-api/src/route_controller.rs` (modified)
- `crates/camel-core/src/lifecycle/application/context_lifecycle.rs` (modified)
- `crates/camel-core/src/hot_reload/application/reload_actions.rs` (modified)
- `crates/camel-core/src/context_tests.rs` (modified)

**Steps:**
1. In `consumer.rs`, add to `ConsumerContext` a field
   `stop_budget: Option<tokio::sync::watch::Receiver<Option<std::time::Duration>>>`
   (initialized `None` in `ConsumerContext::new`), a builder
   `pub fn with_stop_budget_slot(mut self, rx: tokio::sync::watch::Receiver<Option<std::time::Duration>>) -> Self`
   that sets it, and an accessor
   `pub fn stop_budget_slot(&self) -> Option<tokio::sync::watch::Receiver<Option<std::time::Duration>>>`
   that clones it. Doc-comment the builder: installed by the camel-core runtime
   at consumer start; the runtime writes the effective stop budget before
   cancelling; `None` value inside the channel means "not yet written".
2. In `runtime.rs`, add `stop_budget: Option<std::time::Duration>` to the
   `RuntimeCommand::StopRoute` variant (`{ route_id, command_id, causation_id,
   stop_budget }`), keeping the `#[non_exhaustive]` enum otherwise untouched.
   Update the two in-file test constructors (runtime.rs:806, :996) with
   `stop_budget: None`.
3. Update EVERY other in-repo `RuntimeCommand::StopRoute { .. }` constructor to
   compile: `context_lifecycle.rs` (inside `stop_context`) and
   `reload_actions.rs` (4 sites) and `context_tests.rs` (2 sites) get
   `stop_budget: None` in this task (Task 2.1 flips the graceful producer).
4. In `route_controller.rs` (camel-api trait), add a provided async method to
   `RouteController`:
   `async fn stop_route_with_budget(&mut self, route_id: &str, _budget: std::time::Duration) -> Result<(), CamelError> { self.stop_route(route_id).await }`
   — documented as: budget-aware graceful stop; default delegates to
   `stop_route`, ignoring the budget.

**Tests:**
- `stop_budget_slot_roundtrip` (consumer.rs tests): build `ConsumerContext::new`,
  assert `stop_budget_slot()` is `None` → install a `watch::channel(None).1`
  via `with_stop_budget_slot` → assert `stop_budget_slot()` clones a receiver
  reading `None`. Command: `cargo test -p camel-component-api --lib stop_budget_slot_roundtrip`.
- `stop_route_with_budget_default_delegates` (route_controller.rs tests): a
  minimal fake `RouteController` whose `stop_route` records the route_id →
  call `stop_route_with_budget("r1", Duration::from_secs(10))` → assert
  `stop_route` was called with "r1" and `Ok(())` returned. Command:
  `cargo test -p camel-api --lib stop_route_with_budget_default_delegates`.
- `stop_route_command_carries_budget` (runtime.rs tests): construct
  `RuntimeCommand::StopRoute { stop_budget: Some(Duration::from_secs(10)), .. }`
  → assert the pattern `cmd.stop_budget() == Some(...)` via the existing
  accessor style used in that file (or a direct field match). Command:
  `cargo test -p camel-api --lib stop_route_command_carries_budget`.

**Acceptance:**
- `cargo check -p camel-component-api -p camel-api -p camel-core -p camel-ws -p camel-cli` exits 0.
- `cargo test -p camel-component-api --lib` and `cargo test -p camel-api --lib` pass.
- `cargo clippy -p camel-api -p camel-component-api --all-targets -- -D warnings` exits 0.

- [x] 1.1

## Core plumbing (camel-core)

### Task 2.1: Slot pair lifecycle + budget propagation through the stop chain

**Files:**
- `crates/camel-core/src/lifecycle/adapters/consumer_management.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_helpers.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_registry.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/controller_actor_commands.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/controller_actor.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/runtime_ports.rs` (modified)
- `crates/camel-core/src/context.rs` (modified)
- `crates/camel-core/src/lifecycle/application/context_lifecycle.rs` (modified)
- `crates/camel-config/src/context_ext.rs` (modified)

**Steps:**
1. `route_helpers.rs` `ManagedRoute`: add field
   `stop_budget: Option<tokio::sync::watch::Sender<Option<std::time::Duration>>>`
   (default `None` at every existing construction site).
2. `consumer_management.rs` `spawn_consumer_task`: create
   `let (stop_budget_tx, stop_budget_rx) = tokio::sync::watch::channel(None);`
   install the receiver on the consumer context
   (`consumer_ctx = consumer_ctx.with_stop_budget_slot(stop_budget_rx)` for
   both Immediate and Explicit branches BEFORE the context moves into the
   task), and return `stop_budget_tx` as a 5th tuple element. Update the three
   destructuring callers (`route_controller_trait.rs:742`, `:1019`,
   `route_controller.rs:1352`) to store it into the `ManagedRoute` slot field.
3. `consumer_management.rs` `stop_route_internal`: AFTER taking the route
   write lock and BEFORE `managed.consumer_cancel_token.cancel()` (currently
   line ~736), write
   `if let Some(tx) = managed.stop_budget.as_ref() { let _ = tx.send(Some(shutdown_timeout)); }`
   — one write site covers graceful (configured budget) and registry/reload
   (`DEFAULT_SHUTDOWN_TIMEOUT`) because the budget arrives as the existing
   `shutdown_timeout` parameter.
4. `consumer_management.rs` join reconciliation: add named const
   `CONSUMER_STOP_JOIN_GRACE: Duration = Duration::from_millis(500)` (doc:
   the consumer's own budgeted settle wait expires at the propagated budget;
   this grace extends ONLY the outer consumer-join timeout so the consumer's
   fail-open warn + teardown complete before forced abort — bd rc-evw3z
   review). Apply it at the consumer join: `consumer_budget =
   deadline.saturating_duration_since(Instant::now()) +
   CONSUMER_STOP_JOIN_GRACE`. The pipeline `deadline` and the drain wait keep
   the un-graced budget (drain contract unchanged); suspend (no
   stop_route_internal) is untouched; on reload the join may extend at most
   500 ms past the previous effective abort point while settle and drain
   bounds are unchanged.
5. `route_registry.rs`: add `pub(super) async fn stop_route_with_budget(&mut
   self, route_id: &str, budget: Duration) -> Result<(), camel_api::CamelError>`
   calling `stop_route_internal(&mut self.routes, route_id, budget)`. Existing
   `stop_route` keeps `DEFAULT_SHUTDOWN_TIMEOUT` unchanged.
6. `route_controller.rs`: add `pub(super) async fn stop_route_with_budget`
   mirroring `stop_route_internal` but calling
   `self.routes.stop_route_with_budget(route_id, budget)`.
7. `route_controller_trait.rs` `impl RouteController for DefaultRouteController`:
   override `stop_route_with_budget` to call the controller method from step 5
   plus `self.health_registry().mark_route_stopped(route_id)`, mirroring
   `stop_route` (:809-813).
8. Thread `stop_budget: Option<Duration>` through the internal command path:
   `RouteControllerCommand::StopRoute` variant (`controller_actor_commands.rs:26`),
   the `RuntimeCommand::StopRoute` → `RouteControllerCommand` translation
   (`commands.rs` ~:82 and `apply_runtime_lifecycle` ~:711),
   `RuntimeExecutionPort::stop_route` (`runtime_ports.rs:150`),
   `RuntimeExecutionAdapter` (`runtime_execution.rs:41`), and both actor
   handlers (`controller_actor.rs:56`, `:919`; the drop-arm at `:996` uses
   `..` and needs no change). The actor handler calls
   `controller.stop_route_with_budget(route_id, budget)` when `stop_budget`
   is `Some(budget)`, else `controller.stop_route(route_id)`.
9. `context.rs`: add `drain_timeout: std::time::Duration` field (default
   `Duration::from_secs(10)`, doc: mirrors camel-config
   `default_drain_timeout_ms`), `pub fn set_drain_timeout(&mut self, ...)`,
   `pub fn drain_timeout(&self) -> Duration`; thread through the context
   parts/builder struct (context.rs:71/:112/:142 pattern).
10. `context_lifecycle.rs` `stop_context`: add parameter
   `drain_budget: Duration`; the graceful `RuntimeCommand::StopRoute`
   constructor inside it sets `stop_budget: Some(drain_budget)`.
   `CamelContext::stop_timeout` passes `self.drain_timeout`. Do NOT touch
   `stop_timeout`'s existing `_timeout` parameter or its doc promise
   (separate knob `timeout_ms`).
11. `context_ext.rs`: next to `set_shutdown_timeout` (~:744) add
    `ctx.set_drain_timeout(std::time::Duration::from_millis(config.drain_timeout_ms));`.
12. Leave `reload_actions.rs` constructors at `stop_budget: None` (reload
    drains through the external configured-budget poll; its registry stop
    writes `Some(DEFAULT_SHUTDOWN_TIMEOUT)` via step 3's single write site).

**Tests:**
- `stop_route_internal_writes_configured_budget_slot` (consumer_management.rs
  tests): build a `ManagedRoute` with the existing test consumer fake, call
  `stop_route_internal(&mut routes, "route-1", Duration::from_millis(50))` →
  subscribe a receiver to the route's slot sender and assert it reads
  `Some(Duration::from_millis(50))` after the stop. Command:
  `cargo test -p camel-core --lib stop_route_internal_writes_configured_budget_slot`.
- `registry_stop_writes_default_shutdown_budget` (route_registry.rs or
  consumer_management.rs tests): `route_registry`-level `stop_route` on a
  managed route with a slot → assert slot reads
  `Some(DEFAULT_SHUTDOWN_TIMEOUT)`. Command:
  `cargo test -p camel-core --lib registry_stop_writes_default_shutdown_budget`.
- `stop_route_with_budget_propagates_configured_budget` (route_registry.rs
  tests): `stop_route_with_budget(id, Duration::from_secs(10))` → assert slot
  reads `Some(Duration::from_secs(10))`. Command:
  `cargo test -p camel-core --lib stop_route_with_budget_propagates_configured_budget`.
- `stop_route_internal_join_grace_lets_consumer_teardown_finish`
  (consumer_management.rs tests): fake consumer whose `stop()` sleeps
  (paused clock) `Duration::from_millis(120)` with budget
  `Duration::from_millis(100)` → `stop_route_internal` must NOT abort the
  consumer task before its stop() completes (consumer's post-expiry
  completion flag set true, no "did not stop within" warn). RED before step 4
  (join aborts at 100 ms). Command:
  `cargo test -p camel-core --lib stop_route_internal_join_grace_lets_consumer_teardown_finish`.
- Existing tests `shutdown_timeout_aborts_tasks_not_detach`,
  `shutdown_timeout_aborts_both_tasks`, `stop_route_internal_returns_not_found_when_route_absent`,
  `stop_route_internal_short_circuits_when_already_stopped` stay green.

**Acceptance:**
- `cargo test -p camel-core --lib` passes.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `reload_actions.rs` constructors unchanged at `stop_budget: None` (grep
  evidence in task report).

- [x] 2.1

## Consumer side (camel-ws)

### Task 3.1: WsConsumer settle bound from slot-or-fallback + two-direction red/green tests

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)

**Steps:**
1. Add field `stop_budget: Option<tokio::sync::watch::Receiver<Option<std::time::Duration>>>`
   to `WsConsumer` (NOT to the shared `WsAppState`), initialized `None` in both
   constructors.
2. In `start()` (next to the gauge capture at ~:1710), capture
   `self.stop_budget = ctx.stop_budget_slot();`.
3. In `stop()`'s settle block (~:1902): snapshot the value and drop the borrow
   before awaiting —
   `let configured = self.stop_budget.as_ref().and_then(|rx| *rx.borrow());`
   then use `let settle_bound = configured.unwrap_or(Self::WS_STOP_SETTLE_TIMEOUT_FALLBACK);`
   in the `tokio::time::timeout`.
4. Rename `WS_STOP_SETTLE_TIMEOUT` to `WS_STOP_SETTLE_TIMEOUT_FALLBACK`;
   rewrite the doc comment (~:1834): it is the fallback settle bound used only
   when no drain budget was propagated (suspend, direct stops in tests); the
   graceful bound is the configured drain budget propagated via the consumer
   stop-budget slot (bd rc-evw3z).
5. RED FIRST: with steps 1-4 applied EXCEPT the `stop()` flip (leave the old
   constant usage in place), write the tests below and run them — both
   direction tests must FAIL (`stop-downward-red`, `stop-upward-red` evidence
   captured verbatim in the task report). Then apply the `stop()` flip and
   re-run: all green. Report both runs' outputs. The two direction tests
   exercise the consumer seam directly (no outer join in the unit harness);
   survival of the consumer's fail-open teardown against the OUTER join
   deadline is covered by Task 2.1's
   `stop_route_internal_join_grace_lets_consumer_teardown_finish` regression.

**Tests:** (all in the existing `mod tests` of `crates/components/camel-ws/src/lib.rs`;
use `#[tokio::test(start_paused = true)]`; build consumers via the existing
test helpers, installing the gauge with
`.with_in_flight_counter(Arc::new(InFlightGauge::new()))` and holding one
claim, and the slot with
`ConsumerContext::new(...).with_stop_budget_slot(rx)`)

- `stop_settle_follows_configured_budget_downward`: gauge held (1 claim),
  slot seeded `Some(Duration::from_millis(500))` → spawn `consumer.stop()` →
  assert the future is still Pending after advancing 400 ms → advance to 600 ms
  → stop completes Ok; assert paused-clock elapsed is in
  [500 ms, 1000 ms) — i.e. NOT the 5 s fallback. RED evidence: before the
  `stop()` flip this times out at the 5 s constant inside the test's own
  `tokio::time::timeout(Duration::from_secs(2), ...)` wrapper.
  Command: `cargo test -p camel-ws --lib stop_settle_follows_configured_budget_downward`.
- `stop_settle_honors_budget_above_old_bound`: gauge released by a spawned
  task after `sleep(Duration::from_secs(6))` (paused clock), slot seeded
  `Some(Duration::from_secs(10))` → run `consumer.stop()` wrapped in
  `tokio::time::timeout(Duration::from_secs(11), ...)` under incremental
  `advance` → stop completes Ok with elapsed ~6 s and NO fail-open warning
  (assert the warn did not fire — capture with `tracing_test` or an
  assert on elapsed being >= 6 s, which the old 5 s bound cannot produce).
  RED evidence: before the flip the 5 s timeout expires first → elapsed
  ~5 s < 6 s → assertion fails.
  Command: `cargo test -p camel-ws --lib stop_settle_honors_budget_above_old_bound`.
- `stop_without_slot_uses_fallback`: gauge held, NO slot → advance 4 s →
  still Pending → advance past 5 s → completes Ok (fail-open path).
  Command: `cargo test -p camel-ws --lib stop_without_slot_uses_fallback`.
- `stop_echo_delivered_under_configured_budget_real_io` (REAL IO, no paused
  time): use the existing ws test-server harness; slot seeded
  `Some(Duration::from_secs(1))`; client connects, exchange in flight
  (~200 ms delay step), stop initiated while in flight → echo frame delivered
  before teardown, `stop()` returns Ok, client received the echo.
  Command: `cargo test -p camel-ws --lib stop_echo_delivered_under_configured_budget_real_io -- --nocapture`
  (wall-clock ~1 s max; this is the mission-sanctioned small-budget
  real-IO alternative for the echo direction).

**Acceptance:**
- Both direction tests show captured RED output before the `stop()` flip and
  GREEN after (report verbatim `cargo test` summary lines for both runs).
- `cargo test -p camel-ws --lib` fully green.
- `cargo clippy -p camel-ws --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- Warn fields (`host`, `port`, `path`) and fail-open behavior unchanged
  (grep evidence in task report).

- [x] 3.1

## Docs + verification sweep

### Task 4.1: CONTEXT.md lifecycle bullet, targeted hardcode greps, module test sweep

**Files:**
- `crates/components/camel-ws/CONTEXT.md` (modified)

**Steps:**
1. Update the camel-ws CONTEXT.md lifecycle bullet (~:92) that says "When
   core propagates a configurable drain budget, this bound must follow it":
   state that the graceful-stop settle bound follows the configured drain
   budget propagated through the consumer stop-budget slot, and that the 5 s
   constant is the fallback for un-propagated paths (suspend, direct stops).
2. Targeted greps for other consumers hardcoding a drain-style bound that
   should follow config — run:
   `grep -rn "SETTLE\|settle" crates/components/*/src --include="*.rs" | grep -iv "test"`,
   `grep -rn "from_secs(5)" crates/components/*/src --include="*.rs"`,
   `grep -rn "timeout" crates/components/camel-component-grpc/src/consumer.rs crates/components/camel-http/src/lib.rs | grep -i "stop\|shutdown\|settle"`.
   Report findings verbatim in the task report; do NOT change any other
   component (scope bound: camel-ws/camel-core seam only).
3. Run `cargo test -p camel-ws --lib` and `cargo test -p camel-core --lib`
   and report the summary lines.

**Tests:**
- No new tests; this task is doc + evidence sweep. Verify the doc renders:
  `cd docs && mdbook test` is NOT required (no docs/** change); CONTEXT.md is
  covered by the crate doc gate in STAGE 4.

**Acceptance:**
- CONTEXT.md bullet no longer describes the coupling as a future gap.
- Grep findings recorded in the task report (verbatim output).
- Both module test suites green.

- [x] 4.1
