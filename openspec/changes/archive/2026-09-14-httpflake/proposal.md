# Proposal: httpflake

## Why

bd rc-3ayy7 (P2): eight `camel-component-http` lib tests panic intermittently
with "consumer server did not become ready on port X" under
`cargo test --workspace --lib` on the loaded shared build box. Three workspace
runs each failed a different subset (1, 3, 2 of the then-known 3); the same
suite is green in isolation. The flake blocks STAGE 4 gates of sister
missions and erodes trust in the suite.

Pre-flight expert ruling (e_glm, 2026-09-14): the root cause is a cross-test
`ServerRegistry::reset()` race, primary and sufficient. `reset()` is a
test-only function that clears ALL registry entries; its 20 call sites all
sit inside tests holding `REGISTRY_TEST_MUTEX`, but the shared readiness
helper
`setup_consumer_on_free_port` (8 callers: json/text/xml/empty/bytes/stream-ct/
override-ct/bytes-ct content-type tests) holds no mutex. A concurrent reset
during the helper's stage→ready window orphans the registry entry — the serve
task survives but `bound_addr()` returns `None` forever, so the readiness
deadline fires. A secondary contributor: the 5 s wall-clock readiness budget
can be exceeded by OS thread starvation on a loaded machine (the deadline
`assert!` fires on any `None` poll past the deadline).

## What Changes

- `setup_consumer_on_free_port` holds `REGISTRY_TEST_MUTEX` from
  `stage_listener` through readiness-complete (including the tail yield
  loop), releasing before it returns. Closes the reset race for all 8
  exposed tests and the staged-clear/EADDRINUSE variant of the same window.
- Readiness poll budget: deadline 5 s → 10 s; backoff 1 ms doubling to a
  64 ms cap; panic message enriched with a cause hint (registry entry
  absent — concurrent reset or starvation). No TCP probes — the registry
  poll stays the readiness canon (rc-w1u9 law).
- New regression test
  `readiness_survives_concurrent_registry_reset`: a hammer thread looping
  legal resets `{ lock mutex; reset(); }` with try_lock contention counting
  while the helper runs bounded setups — always at least 25, continuing
  past 25 only until one contended reset is observed, hard cap 50 — then
  asserting contention occurred and every setup became ready. The hammer
  holds the mutex (a legal concurrent reset); a bare reset would defeat
  the fix and prove nothing. The hammer terminates via a stop flag set by
  a Drop guard that also joins the thread, so a panic mid-test cannot leak
  a live reset loop into later tests.
- Evidence per mission: loaded-soak before/after — full
  `cargo test -p camel-component-http --lib` loop under 12 CPU spin
  burners, plus a targeted multi-filter run. BEFORE: at least one readiness
  panic; AFTER: zero panics across ≥ 15 loaded runs.

Excluded (out of scope, filed as follow-ups where warranted): per-key
scoping of `ServerRegistry::reset()` (20 call sites assume a global wipe);
sharing one server boot across the 8 tests (mpsc receiver is
single-consumer; wrong axis once the race is closed); probe-based (TCP
connect) readiness (rc-w1u9 ruled it structurally broken).

## Acceptance criteria

- The deterministic regression test passes with the fix and fails
  (readiness panic) without it.
- All 8 content-type tests and the full `camel-component-http --lib` suite
  pass in isolation, unloaded.
- Loaded soak AFTER the fix: 0 readiness panics across ≥ 15 full-suite runs
  under the same 12-burner load that reproduced the flake BEFORE the fix.
- `cargo fmt --check`, `cargo clippy -p camel-component-http -- -D
  warnings`, `cargo xtask lint-unwrap`, doc gate on camel-http: green.
- No production code paths change: all edits live in `#[cfg(test)]` code
  plus the readiness helper's panic text.

## Risk budget

Acceptable: minor wall-time serialization of 8 tests against the ~29
existing mutex holders (ms-scale setups; the 10-second deadline starts
after mutex acquisition — acquisition itself is bounded in practice by the
µs-scale critical sections of the other holders, since std Mutex has no
timed lock). Out of bounds: any production-surface change, weakening
of the staged-listener law (ADR-0070) or the mark-ready law (rc-w1u9), and
any edit outside `crates/components/camel-http`.

Reference: bd rc-3ayy7 (discovered from rc-jj8eu / jobargs STAGE 4 gates).
