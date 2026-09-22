# Proposal: permitcancel

## Why

bd rc-orr73 (P2, retro520 finding): the gRPC consumer dequeues a request
envelope and then awaits `semaphore.acquire_owned()` OUTSIDE its
cancellation `select!` (`consumer.rs` `start_inner`). When the dispatcher
semaphore is saturated by long-lived streams, a stop request cannot be
observed: the consumer blocks on the permit until the core force-aborts
it. A forced abort skips the dispatch-table unregister, so restarting
the same consumer path fails with "duplicate gRPC consumer path".

Retro520 lists cancellation cleanup as a GLM-family review blind spot;
this change closes the found instance and pins it with tests.

## What Changes

Included (crate `camel-component-grpc` only):

- Permit acquisition in `start_inner` observes `ConsumerContext`
  cancellation: a `select!` races the permit wait against
  `ctx.cancelled()`.
- A dequeued-but-unprocessed envelope receives an explicit
  UNAVAILABLE reply on every shutdown-mid-wait path, so clients do
  not hang. The in-flight claim drops (RAII).
- Dispatch registration becomes abort-safe: an RAII guard removes the
  path from the dispatch table on Drop (spawned removal), covering
  forced abort and early error exits. The graceful path disarms the
  guard and keeps the awaited inline unregister, so stop-then-restart
  stays synchronous and race-free.
- Tests: unit (cancellation-observing acquire, envelope UNAVAILABLE
  reply, guard Drop removal, identity-checked cleanup, closed-entry
  replacement at insert) and integration (saturated stop exits
  cleanly; abort mid-wait permits same-path restart; saturated
  stop-then-restart re-registers the same path and serves traffic).

Excluded: producer-side semaphore waits, server lifecycle/eviction
(servers stay process-lifetime), core abort policy, and concurrency
limit validation (rc-9kgtm, shipped).

## Acceptance criteria

- A saturated consumer (concurrency=1, one held bidi stream) completes
  a graceful stop within the test timeout while a second request waits
  for a permit; the waiting client's call terminates with an error, not
  a hang.
- `JoinHandle::abort()` on a consumer waiting mid-permit removes the
  stale registration asynchronously (identity-checked, runtime alive)
  or leaves it replaceable because its sender is closed; in both cases
  a new consumer on the same host/port/path starts without a
  duplicate-path error.
- After a saturated stop, a new consumer on the same host/port/path
  starts without a duplicate-path error and serves a request.
- Gates: fmt, clippy `-D warnings` on the affected crate, affected
  tests green.

## Risk budget

Acceptable: a best-effort UNAVAILABLE reply when the reply channel is
already full or dropped (logged, not fatal). Out of bounds: any change
to the channel==semaphore invariant (rc-ey6v), permit accounting, or
server registry lifetime; any new public API.
