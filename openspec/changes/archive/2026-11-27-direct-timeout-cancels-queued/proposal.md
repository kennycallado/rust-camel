# direct-timeout-cancels-queued

## Why

Two changes landed on 2026-11-26 with contradictory contracts for one event:

- `direct-concurrent-admission` (614) specced that a `timeout_ms` elapsing
  while waiting for admission fails the call but the queued exchange still
  executes once the slot frees.
- `dispatch-cancellation` (622) made an abandoned wait cancel the exchange.

CI caught the collision in `direct_timeout_still_counts_admission_wait`
(`MockEndpoint 'done': timed out waiting for 2 exchanges (got 0 after 1s)`).
The owner ruled that cancellation wins: a caller that gave up must not leave
work running, which is also what the MCP dispatch-timeout knob needs.

## What changes

- `direct-dispatch` spec: the `No producer-side serialization for Concurrent
  consumers` requirement — the one that owns 614's timeout scenario — now
  states the step-boundary rule for the channel path, with the inline path
  called out as a prompt drop.
- `crates/camel-test/tests/direct_concurrent_admission_test.rs`:
  `direct_timeout_still_counts_admission_wait` asserts no later step runs for
  either abandoned exchange, settling in-flight work first so a queued
  dispatch is provably dequeued; `abandoned_dispatch_stops_before_the_next_step`
  pins the admitted case.
- `docs/src/components/direct.md` and the `camel-direct` channel-path comment:
  the sentence claiming an enqueued exchange can still execute is replaced.

## Impact

No runtime change. The behaviour shipped with `rc-29t24`; this change makes
the published contract match it and guards both directions with tests.

## Out of scope

- The MCP knob that wraps `send_and_wait` in a timeout (`rc-sqpqn`).
- Route stop/suspend drain grace (ADR-0043).
- Aggregate-split marker coverage (follow-up `rc-3wrhd`).
