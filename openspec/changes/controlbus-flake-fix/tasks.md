# Tasks: controlbus-flake-fix

## camel-test

### Task 1.1: Rewrite controlbus_stops_route to paused-time determinism

**Files:**
- `crates/camel-test/tests/controlbus_test.rs` (modified)

**Steps:**
1. In `controlbus_stops_route` (~line 282), change the attribute from
   `#[tokio::test(flavor = "multi_thread")]` to plain `#[tokio::test]`
   (current-thread runtime).
2. Change the harness construction from
   `CamelTestContext::builder().with_timer().with_mock().with_component(ControlBusComponent::new()).build().await`
   to the same chain plus `.with_time_control()` before `.build()`, destructuring
   the result as `let (h, time) = builder_chain.build().await;`.
3. Keep both route definitions byte-identical (`timer:auto?period=100&repeatCount=10`
   with `authorizedRoutes` untouched on the trigger route; `timer:stop-trigger?period=50&repeatCount=1`
   → `controlbus:route?routeId=auto-route&action=stop&authorizedRoutes=auto-route`
   → `mock:stop-done`). Period values become virtual time.
4. Keep the initial assertion: `route_status(&h, "auto-route")` == `Some(RouteStatus::Started)`.
5. Replace the wall-clock wait `stop_endpoint.await_exchanges(1, Duration::from_secs(3)).await;`
   with a bounded virtual-time pump:
   - yield burst: `for _ in 0..10 { tokio::task::yield_now().await; }` so both
     timer tasks register their intervals;
   - `let mut delivered = false;` then `for _ in 0..20 { time.advance(Duration::from_millis(50)).await; for _ in 0..30 { tokio::task::yield_now().await; } if stop_endpoint.get_received_exchanges().await.len() >= 1 { delivered = true; break; } }`;
   - immediately after the loop (BEFORE any assert that may panic):
     `time.resume();` — real time must be restored even on the failure path so
     TestGuard's spawned cleanup can progress;
   - then `assert!(delivered, "stop trigger did not fire within 1s of virtual time (20 × 50ms)");`
6. Keep the post-pump assertion: `route_status(&h, "auto-route")` == `Some(RouteStatus::Stopped)`.
7. `time.resume()` already ran at the end of step 5 (both paths); `h.stop().await;`
   now runs on real time.
8. Keep the final `stop_endpoint.assert_exchange_count(1).await;` after `h.stop()`.
9. No import changes: `await_exchanges` is an inherent method on the mock
   endpoint, not an import. Confirm `Duration` remains used (the pump), and that
   `ControlBusComponent` / `RuntimeCommand` / `StepAccumulator` imports stay
   untouched.
10. Run `cargo fmt` on the file and `cargo clippy -p camel-test --all-targets -- -D warnings`.

**Tests:** (executable spec — the rewritten test IS the deliverable)
- `controlbus_stops_route`: setup = two routes (auto timer + one-shot ControlBus
  stop trigger) on a paused-time current-thread harness → action = pump virtual
  time in 20 bounded 50ms increments with yield bursts → assert = `mock:stop-done`
  receives ≥1 exchange within the budget, auto-route status transitions
  Started → Stopped, final exchange count on `mock:stop-done` == exactly 1.
  Command: `cargo test -p camel-test --test controlbus_test controlbus_stops_route -- --exact`.
  Expected: FAILS before the rewrite only under CPU contention (flake); PASSES
  deterministically after the rewrite on idle AND saturated hosts.
- Regression sweep (existing tests, unmodified): command
  `cargo test -p camel-test --test controlbus_test` — expected: all tests in the
  binary green, 3 consecutive runs.
- Idle repetition: run the `-- --exact` command above 40 times in a shell loop —
  expected 40/40 green.
- Saturation repetition: launch burners recording their PIDs
  (`pids=""; for ((i=0; i<$(nproc); i++)); do yes > /dev/null & pids="$pids $!"; done`)
  and register `trap 'kill $pids 2>/dev/null || true' EXIT` so burners die even
  if a run fails, then run the `-- --exact` command 20 times, kill the
  burners — expected 20/20 green (the e_opus reproduction showed 8-9/20
  failures on main before the fix). PID-based kill avoids terminating
  unrelated `yes` processes.

**Acceptance:**
- `grep -n "await_exchanges(1, Duration::from_secs(3))" crates/camel-test/tests/controlbus_test.rs`
  returns no match inside `controlbus_stops_route` (wall-clock gate removed from
  the pass path).
- `cargo test -p camel-test --test controlbus_test controlbus_stops_route -- --exact`
  exits 0, 40/40 consecutive runs on idle host.
- Same command exits 0, 20/20 consecutive runs while `yes > /dev/null` burners
  saturate all cores.
- `cargo test -p camel-test --test controlbus_test` exits 0, 3 consecutive runs.
- `cargo check -p camel-test` exits 0.
- `cargo fmt --check` and `cargo clippy -p camel-test --all-targets -- -D warnings`
  exit 0.

- [x] 1.1
