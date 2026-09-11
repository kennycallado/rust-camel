# Design: add-rest-streaming-contract

## Approach

L3 pins existing behavior; it ships no production code. The contract is
established at two boundaries:

1. **DSL boundary** (`camel-dsl`): the lowered raw pipeline (user steps +
   injected `Content-Type`/default-status steps) must not touch the stream.
   Proven by driving the compiled steps of a `binding: raw` route with an
   instrumented exchange: a poll-counting stream wrapper, an `Arc` identity
   handle to the stream mutex (identity survives only if the pipeline never
   re-wraps the `StreamBody`), metadata equality, and one-then-`AlreadyConsumed`
   consumption after the pipeline. Per the pre-flight oracle (e_gpt,
   GO-WITH-CHANGES): identity and consumption behavior is the observable
   contract; the absence of transient `Arc::clone` calls is an
   implementation detail the spec deliberately does not require.

2. **HTTP boundary** (real `camel-component-http` driven from `camel-dsl`
   dev-dependency tests): `HttpConsumer::new` +
   `ConsumerContext::new(mpsc, token, route)` boots a genuine axum server on
   a free port; the test replaces the downstream pipeline with the channel
   receiver and fulfills envelopes by hand, exactly like camel-http's
   in-crate rig (`test_http_consumer_does_not_enforce_max_response_body_for_stream`).
   The client is a hand-rolled `tokio::net::TcpStream` HTTP/1.1 writer —
   no reqwest dev-dependency, and full control over chunked framing and
   disconnect timing (drop the socket mid-response). Dependency direction is
   legal: `camel-component-http` does not depend on `camel-dsl`.

   The request-side path under test: axum handler applies the
   `Content-Length` 413 pre-check, wraps the body in the
   `max_request_body` mid-stream cap, and builds `StreamMetadata`
   `{ size_hint: content_length, content_type }` (`camel-http` lib.rs
   ~1530-1594). The consumer loop installs `Body::Stream(envelope.body)` on
   the exchange (~1851). The reply finalizer takes the stream exactly once,
   uses `metadata.content_type` as fallback Content-Type, and maps an
   already-consumed stream to a logged 500 with an empty body (~2027-2049).
   `HttpReplyBody::Bytes` over `max_response_body` is replaced with a 500;
   `HttpReplyBody::Stream` is written uncapped (~1615-1653).

## Decision: response byte-limit policy for streamed replies

**Ruled (pre-flight): `max_response_body` caps materialized reply bytes
(`HttpReplyBody::Bytes`) only. `Body::Stream` replies are not byte-capped,
end-to-end, by design.**

Rationale: enforcing a byte cap on a streamed reply requires counting bytes
while forwarding — the cap then trips mid-stream, after headers are on the
wire, leaving the client with a truncated, uncancellable-body failure that
is strictly worse than no cap. Request-side limits stay intact (413
pre-check on `Content-Length` plus the mid-stream cap for chunked bodies),
so unbounded input remains impossible; the response side is the route's own
output, and a route that streams more than it should is an application bug
no transport cap can repair cleanly. Operators who need response caps can
materialize (consume the stream into `Body::Bytes`) and let the existing
Bytes cap apply.

Alternatives rejected:
- **Counted wrap failing mid-stream**: corrupts the response (headers
  already sent, body truncated at an arbitrary chunk) and converts a
  policy violation into a protocol violation.
- **Explicit per-operation `max_response_stream` knob**: plausible L4+
  follow-up for authors who want bounded streaming; not needed to pin the
  current contract and would grow the DSL surface without a consumer.

Existing evidence: camel-http's own pin
`test_http_consumer_does_not_enforce_max_response_body_for_stream`
(max_response_body 16, streams 32 bytes, expects success). This change
re-pins that policy through the REST-registered consumer path and writes it
into the spec.

## Cancellation semantics (narrowed per pre-flight)

Client disconnect is observable at two points: (a) the reply stream body is
dropped by the HTTP layer while the route still holds it — the route's
stream writes fail, which surfaces through the stream error channel, not
through the consumer; (b) the finalizer's reply-channel send fails because
the axum handler side is gone — the finalizer ignores the send result by
design (`let _ = reply_tx.send(reply)`). The pinned, testable contract: a
client disconnect during a streamed reply does not fail the consumer loop,
and subsequent requests on the same server are served. The spec does not
promise delivery semantics for half-written responses.

## Test rig notes

- Server tests serialize on a binary-local mutex (the process-global
  `ServerRegistry` is shared between parallel tests in one binary, mirroring
  camel-http's `REGISTRY_TEST_MUTEX` discipline) and use unique paths plus
  the bind-drop-rebind free-port idiom.
- Readiness uses a connect-retry loop, not a fixed sleep.
- No camel-http in-crate test is duplicated: the camel-dsl rig pins the
  REST-registered consumer path; camel-http's in-crate pins cover the
  api-registered path. The consumed-reply-500 branch previously had no test
  anywhere — this change adds the missing pin through the rig.

## Risks

- Dev-dependency adds compile surface to `camel-dsl` tests only; no
  production dependency edges change.
- Hand-rolled HTTP client must speak enough HTTP/1.1 (Content-Length and
  chunked framing, response read-to-EOF with `Connection: close`); it is
  test-local and ~80 lines.
