# Design: seda-flake-fix

## Approach

In `crates/camel-test/tests/seda_test.rs`, three tests share the flaky idiom
`sleep(window)` → `h.stop()` → `assert_exchange_count(K)`. Convert each to
`await_exchanges(K, deadline)` → `h.stop()` → `assert_exchange_count(K)`:

1. `test_seda_connects_two_routes`: period=50, repeatCount=3 → sleep(500ms)
   becomes `await_exchanges(3, Duration::from_secs(10))` on `mock:result`,
   invoked after `h.start()` and before `h.stop()`.
2. `test_seda_concurrent_load`: period=10, repeatCount=50,
   concurrentConsumers=4 → sleep(1000ms) becomes
   `await_exchanges(50, Duration::from_secs(30))` on `mock:result` (50
   exchanges across 4 consumers; 30s is 60× the nominal 0.5s completion).
3. `test_seda_inout_integration`: period=50, repeatCount=3, InOut →
   sleep(500ms) becomes `await_exchanges(3, Duration::from_secs(10))` on
   `mock:inout-result`.

Endpoint handles move above the wait (currently obtained after stop). Route
definitions, `repeatCount`, exchange patterns, and all assertions stay
byte-identical. `h.stop()` still runs before the exact assert; `repeatCount`
caps total production so the count cannot grow past K after the await returns.

Why not paused time (the rc-08ng approach): seda endpoints expose time-based
offer semantics (`timeout=3000` parameters in the fanout test) and the component
may use internal tokio timeouts around queue offers; paused time would change
which internal timers elapse and needs a component-internal audit. The mock's
`await_exchanges` is already Notify-based (no polling): converting the sleep
into an event wait removes the load-sensitive window at minimal risk, per the
e_opus fix direction recorded in rc-50ky ("await-based determinism or generous
window").

## Affected crates

- camel-test (dev-facing only): three tests rewritten in
  `tests/seda_test.rs`. No src/ changes.

## Architecture boundaries

Test-only. The production path exercised (timer producer → seda queue →
consumer → mock) is unchanged; only the settling mechanism of the tests
changes. No Runtime/DSL/Components/Services surface is modified.

## Phases

Single-phase change: three mechanically identical conversions, no milestone
grouping needed.
