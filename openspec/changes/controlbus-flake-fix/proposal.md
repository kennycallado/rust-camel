# Proposal: controlbus-flake-fix

## Why

`controlbus_stops_route` (crates/camel-test/tests/controlbus_test.rs) flakes under
CPU contention: e_opus evidence audit (rc-08ng) measured 8/20 failures on the base
commit and 9/20 on HEAD under controlled CPU saturation, while idle runs are 40/40
green. The test drives a real wall-clock timer (`timer:stop-trigger?period=50`)
and then blocks in `await_exchanges(1, 3s)`. Under contention the runtime is
starved, the 3s wall-clock deadline elapses before the timer task is scheduled,
and the mock panics with a timeout. Failure signature is always a mock timeout —
never an invalid transition — so the production control-plane behavior is sound;
only the test's coupling to wall-clock time is fragile.

bd: rc-08ng. Pre-existing on main; must not gate rc-slvd (already merged).

## What Changes

Convert `controlbus_stops_route` from wall-clock waiting to tokio paused time via
the existing `CamelTestContextBuilder::with_time_control()` harness API
(`TimeController::advance`). The test keeps its assertions (route starts, ControlBus
stop action executes, route reaches Stopped, `mock:stop-done` receives exactly 1
exchange) but replaces `await_exchanges(1, 3s)` on real time with a bounded
virtual-time advance loop (precedent: harness_test.rs tests 5 and 6).

Explicitly excluded:
- No production code changes (camel-core, components, control plane untouched).
- No conversion of the other 7 tests in controlbus_test.rs (rc-08ng names only
  this test; seda flake class tracked separately as rc-50ky).
- No new harness API (with_time_control already exists and is exercised).

## Acceptance criteria

- `controlbus_stops_route` contains no wall-clock deadline as its pass/fail gate;
  pass/fail is driven by virtual-time advance and event arrival.
- Test green 40/40 consecutive runs on idle (`cargo test -p camel-test --test
  controlbus_test controlbus_stops_route -- --exact` in a loop).
- Test green 20/20 under deliberate CPU saturation (approximating the e_opus
  reproduction), demonstrating the flake class is eliminated rather than narrowed.
- Full `cargo test -p camel-test --test controlbus_test` binary green in 3
  consecutive runs; the current-thread control-plane path (timer → ControlBus →
  RuntimeBus StopRoute → mock) is validated by these repeated full-binary
  executions, not by harness precedents.
- `cargo check -p camel-test` clean; fmt + clippy clean on touched crates.

## Risk budget

Acceptable: changing only test code; worst case is a longer cooperative wait on
teardown if paused time is never resumed — mitigated by calling
`time.resume()` before the explicit `h.stop()` so teardown runs on real time, and
by the bounded virtual-time pump for the pass/fail gate (max iterations, then a
hard assert failure, never an unbounded endpoint poll). Out of bounds: touching
production control-plane code, changing MockEndpoint semantics, or modifying the
shared `await_exchanges` API used by other tests.
