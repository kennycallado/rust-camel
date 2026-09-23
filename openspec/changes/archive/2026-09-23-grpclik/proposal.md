# Proposal: grpclik

## Why

bd rc-qq8zz (P1, discovered-from rc-orr73/permitcancel): a gRPC bidi
response stream stays open after consumer shutdown or abort. The
`BidiHandler` forward task (server.rs) holds a clone of the reply
sender (`reply_tx_forward`) while awaiting the CLIENT request stream;
`GrpcItemStream` ends only when ALL senders drop. Consumer shutdown
drops only the consumer-side sender, so an already-accepted bidi call
whose client keeps its request side open never terminates — the client
response stream parks forever. permitcancel's task-1.4 tests hit this
and worked around it by closing the client side before asserting
termination (deviation record in the archived permitcancel tasks.md).

## What Changes

- Wire the consumer's shutdown into the bidi forward task: the
  dispatch registration carries a cancellation token derived from
  `ConsumerContext::cancel_token()` via `child_token()` (ADR-0043
  fan-out discipline, no fresh `CancellationToken::new()` root —
  lint-cancel-tokens ratchet), the registration guard cancels it on
  every teardown path (graceful cleanup, drop, abort), and the bidi
  forward task `select!`s on it: on cancel it best-effort
  `try_send`s an UNAVAILABLE error item (same
  `"consumer shutting down"` status as `reply_unavailable`) and exits,
  dropping both senders so the response stream closes.
- `GrpcDispatchEntry` widens from a 3-tuple to a 4-tuple carrying the
  token (single source of truth; per-consumer instance, so a
  replacement registration is never governed by its predecessor's
  token).
- Regression tests: graceful and abort integration tests where the
  client KEEPS its request stream open and the response stream still
  closes within bounded time (fails pre-fix), plus unit tests for the
  handler cancel arm and the guard's token cancellation.

Excluded: `ClientStreamingHandler`'s forward task (its call future
terminates via the oneshot reply drop — no detached sender clone, no
leak); endpoint-layer changes; any change to non-bidi modes.

## Acceptance criteria

- Abort or gracefully stop a gRPC consumer with an open, accepted bidi
  call (client request side held open) → the response stream reaches a
  terminal item (`Err(UNAVAILABLE)` or end) within bounded time.
- All existing gRPC tests stay green (permitcancel's saturated
  shutdown/abort/restart suite included, unmodified).
- No new production `CancellationToken::new()` site (child token
  only); `cargo xtask lint-cancel-tokens` count unchanged.
- Affected crates: `camel-component-grpc` only.

## Risk budget

Bug fix, one crate, no public API change (`GrpcDispatchEntry` is
`pub(crate)`). Risk: shutdown ordering (token cancel vs processor
abort) — mitigated by bounded-time regression tests on both paths.
Out of bounds: any behavior change for live consumers, unary,
server-streaming, or client-streaming calls.
