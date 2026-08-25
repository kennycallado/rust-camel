# Design: controlbus-flake-fix

## Approach

Rewrite `controlbus_stops_route` (crates/camel-test/tests/controlbus_test.rs) to use
the harness time controller instead of real time:

1. Builder gains `.with_time_control()`; `build()` returns `(CamelTestContext,
   TimeController)`. This pauses tokio time at build time (harness.rs typestate:
   `CamelTestContextBuilder<WithTimeControl>`).
2. Drop `flavor = "multi_thread"` → default current-thread `#[tokio::test]`.
   The test only uses async mock accessors (`await_exchanges`,
   `get_received_exchanges`, `assert_exchange_count`), never the sync
   `exchange(idx)` accessor that panics on current-thread.
   `TestGuard::drop` already handles CurrentThread (spawn-based best-effort
   cleanup) and the test calls `h.stop()` explicitly before that.
   NOTE: harness_test.rs tests 5/6 are precedent only for the timer + mock
   current-thread path — they do NOT exercise the control plane. The ControlBus
   path is fully async command dispatch (`RuntimeCommand::StopRoute` through the
   RuntimeBus, camel-controlbus/src/lib.rs ~360-368), so it is cooperative-task
   safe on current-thread, but that claim is validated by the repeated
   full-binary executions in the acceptance criteria, not by precedent.
3. Replace the wall-clock wait block:

   ```
   stop_endpoint.await_exchanges(1, Duration::from_secs(3)).await;
   ```

   with a bounded virtual-time pump, following the harness_test 5/6 idiom:

   - yield burst (10× `yield_now`) so both timer tasks register their intervals,
   - loop up to N=20 iterations: `time.advance(50ms)`, yield burst (30×), then
     check `stop_endpoint.get_received_exchanges().await.len() >= 1`; break on
     arrival,
   - after the loop, hard-assert arrival with a clear message ("stop trigger did
     not fire within 1s of virtual time") so a broken pipeline fails fast instead
     of hanging. 20 × 50ms virtual comfortably covers the trigger timer's first
     tick (tokio intervals fire immediately on first tick) plus pipeline hops.

4. Before the post-stop assertions that follow the pump loop and before teardown:
   after the loop asserts arrival, read `route_status(auto-route)`, assert
   Stopped, then call `time.resume()` BEFORE `h.stop()` — teardown (and the
   TestGuard fallback) then runs on real time, closing the paused-teardown-hang
   risk. Final `assert_exchange_count(1)` on `mock:stop-done` after `h.stop()`.
   All other assertions unchanged: initial `route_status(auto-route) == Started`.
   Route definitions keep their period parameters (values become virtual).
   The pump loop bounds only endpoint polling: cooperative awaits inside
   `advance`/`route_status`/`stop` are not wall-clock-bounded — `time.resume()`
   before teardown is what prevents an unbounded paused-time wait there.

Why not a generous mock bound (3s → 30s): it narrows the flake window but keeps
the wall-clock coupling — under saturation the same class resurfaces. Paused time
removes the coupling and follows in-repo precedent.

## Affected crates

- camel-test (dev-facing only): one test rewritten in
  `tests/controlbus_test.rs`. No src/ changes.

## Architecture boundaries

Test-only change. No Runtime/DSL/Components/Services/Languages/Functions surface
is modified. The test exercises the same production path as before (timer
consumer → controlbus component → RuntimeBus StopRoute → mock endpoint); only the
clock that drives it changes from real to paused tokio time, which is exactly what
`TimeController` (camel-test public helper) exists for.

## Phases

Single-phase change: one coherent test rewrite, no milestone grouping needed.
