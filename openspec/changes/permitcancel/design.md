# Design: permitcancel

## Approach

Three coordinated edits inside `GrpcConsumer::start_inner`
(`crates/components/camel-component-grpc/src/consumer.rs`):

1. **Cancellation-safe permit acquisition.** Replace the bare
   `sem.acquire_owned().await` with an inner `tokio::select!`
   (`biased`) racing `ctx.cancelled()` against the acquire. On the
   cancellation arm the loop breaks; on the semaphore-closed arm the
   existing `ChannelClosed` error path applies (unreachable today —
   nothing closes the semaphore — but the guard below now covers its
   early-exit).
2. **UNAVAILABLE reply for the dequeued envelope.** On cancellation
   winning the permit race, `reply_unavailable(envelope)` consumes the
   envelope. Unary and client-streaming oneshot replies receive
   `GrpcReply::Err(Status::unavailable(...))`. Server-streaming and
   bidi mpsc replies receive a best-effort
   `GrpcStreamItem::Error(Status::unavailable(...))` through
   `try_send`; full or closed reply channels do not delay shutdown.
   Envelopes still buffered in `env_rx` are not covered and retain
   existing channel-closure behavior.
3. **Abort-safe, identity-owned dispatch registration.** Immediately
   after insertion, create a `DispatchRegistrationGuard` containing
   the dispatch table, path, a clone of the inserted `env_tx`, and an
   armed flag. Removal SHALL occur only when the current entry's
   sender identifies the same Tokio mpsc channel
   (`Sender::same_channel`), so delayed cleanup cannot remove a
   replacement registration.

   The graceful path calls an awaited guard cleanup method. That
   method acquires the write lock, conditionally removes the owned
   entry, and disarms the guard before releasing the lock, with no
   await between removal and disarm. It MUST NOT disarm before
   awaiting the lock.

   `Drop` first attempts synchronous cleanup with `try_write`. If the
   lock is unavailable, it uses `Handle::try_current()` to spawn
   identity-checked cleanup. Runtime teardown does not guarantee
   execution of spawned cleanup; registration inserts therefore
   atomically replace an existing entry whose sender is closed while
   still rejecting live duplicates. This also permits immediate
   same-path restart after an aborted task has completed.

Rationale for guard-over-explicit-only: forced abort and the early
`?` after insert are exactly the paths that skip trailing code; Drop
is the only mechanism that runs on both. Identity-checked removal
(`same_channel`) closes the delayed-cleanup-vs-restart race in both
orders: cleanup before re-insert removes the stale entry; re-insert
first replaces the closed entry and the delayed cleanup no-ops.

## Affected crates

- `camel-component-grpc`: `src/consumer.rs` (loop restructure, helper,
  guard, unit tests), `tests/integration.rs` (three integration
  scenarios). No other crate.

## Architecture boundaries

Component layer only. No changes to `camel-component-api`
(`ConsumerContext::cancelled()` already exposes the token), no core
abort policy, no DSL/producer surface. The channel==semaphore
invariant (rc-ey6v) and the single-backpressure-point design are
untouched: the permit count and channel capacity still derive from the
same `concurrency` value. No new public API — guard and helper are
`pub(crate)`/private.

Producer-side semaphore waits are excluded because they belong to
outbound producer request lifecycles and neither own consumer dispatch
registrations nor observe `ConsumerContext` cancellation.

## Test matrix

Unit, in-crate (`consumer.rs` `#[cfg(test)]`, full access to
`pub(crate)` types):

- `permit_wait_cancel_safe_grants_permit_when_free` — ARRANGE:
  `Semaphore::new(1)` uncontended, fresh-token ctx. ACT: await the
  acquire helper. ASSERT: permit granted within 500 ms.
- `permit_wait_cancel_safe_returns_cancelled_on_token_cancel` —
  ARRANGE: `Semaphore::new(1)` with the permit already owned; ctx
  token. ACT: spawn helper, cancel token. ASSERT: helper returns the
  cancelled variant within 500 ms (`tokio::time::timeout`).
- `reply_unavailable_unary_and_client_streaming_send_reply_err` —
  ARRANGE: oneshot reply channel per envelope kind. ACT:
  `reply_unavailable`. ASSERT: `GrpcReply::Err` with code
  `Unavailable` received.
- `reply_unavailable_streaming_try_sends_error_item` — ARRANGE: mpsc
  reply channel; also a pre-filled (full) variant. ACT:
  `reply_unavailable`. ASSERT: open channel receives
  `GrpcStreamItem::Error(Unavailable)`; full channel returns without
  panic or delay.
- `dispatch_guard_drop_removes_owned_entry` — ARRANGE: real
  `GrpcDispatchTable`, entry `(tx, mode, None)`, armed guard holding a
  `same_channel` tx clone. ACT: `drop(guard)`. ASSERT: poll (1 s cap)
  table no longer contains the path.
- `dispatch_guard_cleanup_spares_replacement_entry` — ARRANGE: guard
  owns tx1; table entry replaced by tx2. ACT: `guard.cleanup().await`.
  ASSERT: tx2 entry survives; guard disarmed (second cleanup no-op).
- `dispatch_insert_replaces_closed_sender_entry` — ARRANGE: entry
  whose receiver is dropped (`tx.is_closed()`). ACT: insert new entry
  via the insert helper. ASSERT: replaced without duplicate error;
  a live-sender entry still yields the duplicate error.

Integration (`tests/integration.rs`, behavioral — registry internals
are `pub(crate)`; saturation via `streaming.StreamService/BidiEcho`
with `consumerConcurrency=1`; client A holds an open bidi call, client
B fires a second call that waits mid-permit; determinism via the
existing 100 ms readiness-sleep pattern):

- `grpc_consumer_shutdown_during_saturated_permit_wait_exits_cleanly`
  — ACT: cancel token while B waits. ASSERT: consumer task completes
  within 2 s; B's call terminates with an error (no hang).
- `grpc_consumer_abort_mid_permit_wait_allows_same_path_restart` —
  ACT: `consumer_task.abort()` while B waits; then start a second
  consumer on the same host/port/path via `start()`. ASSERT: startup
  succeeds without duplicate-path error; a bidi call against the
  restarted consumer completes.
- `grpc_consumer_saturated_stop_then_restart_reregisters_same_path`
  — ACT: graceful cancel under saturation, await task completion,
  restart same path. ASSERT: restart Ok within 2 s; new consumer
  serves a bidi call.

## Alternatives considered

- **Close the semaphore on cancel** (`Semaphore::close` wakes waiters
  with `Err(Closed)`): couples shutdown to an API meant for permanent
  teardown and changes error semantics for all waiters. Rejected.
- **`JoinSet` for waiters + abort_all on shutdown**: the dequeued
  envelope and claim would need to move into a spawned waiter, forcing
  reply handling into a second task and complicating permit ownership.
  The inner select achieves the same with less machinery. Rejected.
- **Spawn-based unregister without disarm**: introduces the
  remove-vs-restart race described above. Rejected.
- **std `RwLock` for the dispatch table** (sync Drop removal):
  changes `GrpcDispatchTable` typing shared with the server crate
  surface; disproportionate. Rejected.

Single-phase change — no `## Phases` section.
