# http-envelope-channel-capacity

Derive the HttpConsumer RequestEnvelope channel capacity from
`maxInflightRequests` instead of the hardcoded 64.

## Why

Trivial change — single-line hot-path fix plus guard, no spec surface.

The dispatcher semaphore (`maxInflightRequests`, default 1024, URI-configurable)
is meant to be the single inflight backpressure point. But `HttpConsumer::start`
(crates/components/camel-http/src/lib.rs:1387) creates the RequestEnvelope mpsc
with hardcoded capacity 64. When 64 envelopes are buffered, dispatcher tasks
park in `send().await` while still holding their semaphore permit, so the real
cap becomes 64 + in-flight instead of the configured limit — a second, hidden,
non-configurable inflight cap. Same hot path as the rc-vdy2 mutex convoy.

## What changes

- `HttpConsumer::start`: channel capacity = `envelope_channel_capacity(max_inflight_requests)`
  (each in-flight envelope holds exactly one semaphore permit, so a buffer of N
  can never fill before the semaphore exhausts; `send()` never blocks on a full
  buffer, the semaphore stays the only backpressure point).
- `envelope_channel_capacity(n) = n.max(1)`: `maxInflightRequests=0` is
  representable but `tokio::sync::mpsc::channel(0)` panics — guard keeps
  consumer start panic-free (all requests 503 via the empty semaphore).
- Unit tests for the mapping + a `maxInflightRequests=0` consumer-start test.
- CONTEXT.md: note that the envelope channel derives from the same limit.
