# Design: http-entity-enclosing-gate

## Context

`HttpProducer::process` (`crates/components/camel-http/src/lib.rs` ~L2015+)
resolves the method, resolves the URL, then materializes the body. The
materialization block (~L2135-2147) takes the body from the exchange with
`std::mem::take` and converts it to `Option<Vec<u8>>`. Three consumers
attach it:

1. `ssrf::send_with_ssrf_safe_redirects` (~L2150) receives
   `materialized_body` and re-sends it per 307/308 hop.
2. Direct send, stream arm (~L2175-2180): `reqwest::Body::wrap_stream`.
3. Direct send, bytes arm (~L2184): `request.body(body_bytes)`.

None of the three checks the method. This is the defect in bd rc-q1sw.

## Chosen Approach: single upstream gate

Add a free function or associated fn:

```rust
fn is_entity_enclosing(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH")
}
```

`resolve_method` already returns an uppercase `String`. Immediately after
the method and body resolution, and before the redirect branch, compute one
boolean `suppress_body = !is_entity_enclosing(&method_str)`. Then:

- When `suppress_body` is true and the exchange body is non-empty (or is a
  `Body::Stream`), emit one `warn!` with the method and correlation id,
  then treat the body as absent for all downstream consumers.
- Stream gating is unconditional: a suppressed `Body::Stream` is still
  `mem::take`n (so the exchange body stays consumed), `materialized_body`
  stays `None`, and the stream arm with `reqwest::Body::wrap_stream` never
  runs. The bodyless redirect path handles the request. A stream always
  warns when suppressed, because its emptiness cannot be known without
  consuming it.
- The `std::mem::take` still executes for both bytes and stream bodies. The
  exchange body stays consumed, which matches Apache Camel exchange
  semantics (the producer is a terminal wire step for the request body; the
  out message is built from the response).

One decision point, three consumers. No per-path edits.

## Rejected Alternatives

- **Per-path guards**: three copies of the same check drift apart. The
  redirect path is the one most likely to be forgotten.
- **`allowGetBody` escape hatch**: camel-http has none. A knob would invent
  non-parity surface for a use case RFC 9110 restricts to private
  origin-server agreements. If a user needs GET-with-body later, that user
  can request a separate change.
- **Error instead of drop**: Apache Camel drops structurally and silently.
  Erroring would break proxied exchanges that arrive with a body and an
  explicit method header, without giving the route author any way to clear
  the body first. Drop plus `warn!` is observable and recoverable.
- **Restore the body after drop**: the out message is built from the
  response. Restoring the request body would be non-parity bookkeeping with
  no consumer.

## Affected Crates and Boundaries

- `camel-http` (component crate): producer only. No consumer changes, no
  endpoint config changes, no new URI parameters.
- No changes to `camel-core`, `camel-component-api`, DSL, or YAML surface.
- Tests live in `camel-http` unit tests (mock server / captured request)
  and, where a real round-trip is required, `camel-test` integration tests
  with a local axum echo server. All tests stay local and deterministic,
  per the existing `http-emission-correctness` acceptance pattern.

## Logging

One `warn!` per suppressed body, producer-owned, structured: correlation
id and resolved method. Body length is not logged, because a stream length
is unavailable without consumption. The empty-bytes case stays silent: an
empty body with GET is normal (the resolve_method default) and a warn
there would be noise on every plain GET route. The warn test uses the
`tracing-test` crate (`tracing-test.workspace = true` dev dependency,
already declared in the workspace root Cargo.toml) with `logs_assert!` to
count exactly one matching warning.

## Header Interaction

The existing `Producer outbound header forwarding` requirement already
strips `content-length` from forwarded headers. After this change the
producer also generates no framing, because reqwest derives
Content-Length from the attached body and no body is attached. The delta
spec adds one scenario that asserts no orphan Content-Length or
Transfer-Encoding appears on a suppressed request.

## Test Strategy

Deterministic local servers only. Capture the outbound request (method,
body, headers) and assert on it. The redirect test uses a two-hop local
307/308 chain. The stream test builds a `Body::Stream` exchange and asserts
the wire request has no body and no `AlreadyConsumed` error surfaces. The
warn test uses `tracing-test` with `logs_assert!` to assert exactly one
matching warning. A consumption test asserts the exchange body is consumed
(bytes and stream) after suppression.
