# Design: grpclik

## Approach

Leak anatomy (server.rs `BidiHandler::call`): the spawned forward task
loops on the client request stream (`stream.next().await`) holding
`reply_tx_forward` (a clone of the reply mpsc sender) and `body_tx`.
It exits only on client stream end, decode error, or a failed
`body_tx.send`. `GrpcItemStream` (the response stream) ends only when
every reply sender drops. On consumer shutdown the processors are
aborted (`join_set.shutdown()` / future drop), dropping the
envelope-side sender — but if the idle client sends nothing, the
forward task stays parked on `stream.next()` forever holding the
clone: response stream never closes.

Fix — explicit cancel wiring, four surgical edits in
`camel-component-grpc`:

1. `GrpcDispatchEntry` (server.rs) widens to
   `(Sender<GrpcRequestEnvelope>, GrpcMode, Option<Arc<GrpcKernelAuth>>,
   CancellationToken)`. The token is per-consumer-instance: created in
   `start_inner` as `ctx.cancel_token().child_token()` — a derived
   fan-out token (ADR-0043 pipeline cancellation tree discipline),
   never a fresh `CancellationToken::new()` root in production code
   (lint-cancel-tokens ratchet, monotone count).
2. `DispatchRegistrationGuard` (consumer.rs) gains the same token and
   cancels it in `cleanup()` (after removal + disarm) and on `Drop`.
   Placement is load-bearing: `token.cancel()` runs UNCONDITIONALLY at
   the top of `drop()` (before the existing runtime-conditional
   entry-removal match), because `cancel()` is synchronous and
   runtime-free — the no-runtime branch (`Handle::try_current()` Err,
   forced abort during teardown) must still cancel the token or the
   leak persists there. The entry-removal logic below it keeps its
   three branches (uncontended `try_write`, spawned removal, teardown
   no-op) unchanged. Parent cancellation cascades to the child, so
   graceful shutdown closes streams immediately at token fire; the
   guard covers forced abort, where the ctx token never fires.
   `CancellationToken::cancel()` is idempotent — both paths may run.
3. `handle_grpc_request` (server.rs) destructures the 4-tuple and
   clones the token into `BidiHandler` (other handlers unchanged).
4. `BidiHandler::call`'s forward task wraps its loop in
   `tokio::select! { biased; _ = cancel.cancelled() => .. , result =
   stream.next() => .. }`. The cancel arm does a non-blocking
   `try_send(GrpcStreamItem::Error(Status::unavailable(..)))` — same
   `"consumer shutting down"` message as `reply_unavailable`
   (consumer.rs; hoist the const to `pub(crate)` and reuse) — then
   breaks, dropping `reply_tx_forward` and `body_tx`. All senders
   gone → `GrpcItemStream` returns `None` → tonic closes the call.
   `stream.next()` and mpsc sends are cancel-safe; the shutdown path
   never blocks on a full channel (`try_send`, best-effort, matches
   the rc-orr73 `reply_unavailable` idiom).

Replacement isolation: a restart inserts a fresh entry with its own
child token; the old guard's identity-checked removal
(`remove_owned_entry`) never touches the replacement, and the old
token's cancellation cannot reach the new consumer's handlers.

Tests: unit — `BidiHandler` cancel arm (token fired → stream terminal
without client stream end; token live → forwarding unchanged), guard
cancel-on-cleanup and cancel-on-drop. Integration — graceful and
abort scenarios reusing `open_bidi_call`/`wait_for_total`: the client
holds its request side open, shutdown fires, the response stream must
be terminal within 2 s (pre-fix this times out). All waits bounded
(`tokio::time::timeout`, sync `wait_for_total`) per ADR-0069 §13.2 —
lint-unbounded-wait ratchet (393) must not grow.

## Affected crates

- `camel-component-grpc`: server.rs (entry type, handler lookup,
  BidiHandler), consumer.rs (insert, guard, start_inner),
  tests/integration.rs (regression tests). No public API change.

## Architecture boundaries

Component-layer only (inbound transport). No Runtime/DSL/Services
touch. Follows the rc-orr73 canonical spec
(`openspec/specs/grpc-consumer-cancellation/`) — this change adds a
requirement to that capability. ADR-0043 (cancellation tree:
consumer-lifetime token from `ConsumerContext`, fan-out via
`child_token`), ADR-0069 §13.2 (bounded test waits). The token rides
the existing dispatch-table seam — no new registry, no second map
(single source of truth per lint-single-source).
