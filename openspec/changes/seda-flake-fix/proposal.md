# Proposal: seda-flake-fix

## Why

The `seda_test` binary (crates/camel-test/tests/seda_test.rs) flakes under
parallel load: verified 2026-08-24, main's unmodified binary fails ~4/8 full-binary
runs. Failing tests vary — `test_seda_concurrent_load` ("expected 50 exchanges,
got 49"), `test_seda_inout_integration`, `test_seda_connects_two_routes` — but all
three share one idiom: a timer-driven producer with a bounded `repeatCount`, a
fixed `tokio::time::sleep` window (500ms–1000ms), then an exact
`assert_exchange_count`. When the six tests in the binary run in parallel the
wall-clock window occasionally loses the last timer tick and the exact assert
fails one exchange short. Same mock-count-under-load class as rc-2ume but a
distinct mechanism (timer window, not callsite poisoning).

bd: rc-50ky. Pre-existing on main; verified in worktree seda-resume-test.

## What Changes

Convert the three flaky tests from the fixed-sleep idiom to event-driven
settling: replace `sleep(N)` with `await_exchanges(K, generous_deadline)` placed
BEFORE `h.stop()`, keep the exact `assert_exchange_count(K)` after stop. Arrival
is signaled by the mock's `Notify` (no polling); the deadline (10s for the
3-exchange tests, 30s for the 50-exchange load test) is only a failure backstop —
60× or more of the nominal completion time. Production is capped by `repeatCount`, so the
exact count after `await_exchanges` cannot overshoot.

Explicitly excluded:
- No production code changes (camel-seda, camel-core, mock untouched).
- `test_seda_fanout_integration` and both `seda_single_consumer_survives_*`
  tests already settle via `await_exchanges`/bounded retry and are not among the
  evidenced failures — left unchanged.
- No paused-time conversion here: seda endpoints carry time-based semantics
  (`timeout` offer parameter) and the component may use internal timeouts;
  await-based settling is the lower-risk deterministic form for this surface.

## Acceptance criteria

- The three named tests contain no bare `sleep` as their settling mechanism;
  settling is `await_exchanges` before `h.stop()`.
- Full `cargo test -p camel-test --test seda_test` binary green in 10
  consecutive runs (the bd reproduction showed ~4/8 failure rate before).
- `cargo check -p camel-test` clean; fmt + clippy clean on touched crates.

## Risk budget

Acceptable: test-only change; worst case is a rarer flake if delivery exceeds
the generous deadline under extreme saturation. Out of bounds: touching
camel-seda component code, changing MockEndpoint semantics, converting the two
already-settled tests, or paused-time experiments on seda internals.
