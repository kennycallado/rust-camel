# ADR-0102: The `sse:` consumer scheme in camel-http

- Date: 2026-11-21
- Status: Accepted (2026-11-21)
- Context: change `sse-consumer`; bd `rc-q48en` (behavior pinned by the
  epic review), bd `rc-2co7z` (feature-gated suite visibility)
- Amends: [ADR-0060](0060-mcp-first-class-component.md), scope of its SSE
  exclusion only (decision 1). No MCP behavior changes.
- Cross-refs: [ADR-0007](0007-route-supervised-consumer-failure.md)
  (route-supervised consumer failure), [ADR-0013](0013-network-retry-policy-and-migration.md)
  (NetworkRetryPolicy), [ADR-0044](0044-route-admission-backpressure.md)
  (route-admission back-pressure), [ADR-0071](0071-http-outbound-url-policy.md)
  (outbound URL policy), [ADR-0081](0081-uniform-public-cleartext-egress-policy.md)
  (public-cleartext egress policy)

## Context

Many HTTP origins expose Server-Sent Events: a long-lived `GET` whose
response body is a `text/event-stream` of `data:` blocks. The HTTP component
already owns both HTTP directions, but neither covers this shape. The
`http:` producer reads the whole reply body, so an endless stream never
completes and eventually trips its response-body bound.

Three options were considered.

A separate `camel-sse` crate was rejected. The work needs the SSRF
validator, the fresh-resolution and pinning path, the TLS allowlist, and the
pinned-client cache. All four live in `camel-http` as `pub(crate)` seams. A
crate split would either make them public with a cross-crate contract, or
copy them.

Reusing the producer with a per-event split was rejected. The producer
contract is one request in, one reply body out. Its read-whole-body path,
its 10 MiB response bound, and its retry policy stay untouched. A streaming
mode inside the producer would fork that contract for every caller.

A streaming-framework dependency was rejected. The framing parser is a small
byte state machine, and the component avoids a new dependency for it.

ADR-0060 needs a scope note. Rule 5 of that ADR excludes the deprecated
HTTP+SSE MCP transport at the Cargo level (the rmcp `transport-sse-*`
features are never enabled), and Rule 7 lists SSE under exclusions next to
the rejected stdio transport. Both statements concern the MCP component's
transport surface. Neither addresses a scheme owned by `camel-http`. The
subjects differ: MCP's SSE was a protocol transport; this is HTTP response
framing.

The implementation is `crates/components/camel-http/src/sse.rs`: the parser,
the URI config, the component, the endpoint, the consumer, and the
resubscription loop. `HttpBundle::register_all` registers it next to
`http:` and `https:`.

## Decision

### 1. A consumer-only scheme inside camel-http

`sse:` is a scheme of `camel-http`, registered by `HttpBundle` beside
`http:` and `https:`. It is consumer-only: `create_consumer` returns the SSE
consumer and `create_producer` fails with `sse: is consumer-only`. The
capability metadata advertises the consumer direction only.

The component is in-crate so that the existing `pub(crate)` seams (SSRF,
TLS allowlist, client cache) are reused without a public cross-crate
contract. `SseComponent` builds its own `PinnedClientCache` instance, the
same pattern `HttpComponent` and `HttpsComponent` follow.

This ADR amends ADR-0060 by scoping, not by reversal. The MCP component
still never enables rmcp's SSE transport features. The exclusion says
nothing about HTTP event streams, and it does not apply here.

### 2. Scheme-to-transport translation

`sse:` is not a transport. On the wire, the request targets the translated
URL: the configured authority and path under an HTTP scheme.

| Condition | Wire scheme |
| --- | --- |
| component `tls.enabled = true` | `https://` (overrides the rows below) |
| `allowCleartext=true` | `http://` |
| `allowInternal=true` and the target is internal | `http://` |
| otherwise | `https://` |

An internal target is a blocklisted IP literal, `localhost`, or a `.local`
domain, judged by the same `is_ssrf_blocked_ip` predicate the rest of the
component uses. Cleartext to a public target is the ADR-0081 consent: it
needs `allowCleartext=true` explicitly. With `allowInternal=true` and a
public target, the URL stays `https://`, so the endpoint cannot drift into
cleartext egress against a public host.

All validation runs on the translated URL, not on the `sse:` text. The
userinfo form (`user:pass@host`) is rejected at endpoint creation, because
`sse:` defines no username or password parameters. Credentials travel only
in the configured `authHeaders`.

Redirects are followed only inside the connect phase, through the shared
`send_with_ssrf_safe_redirects` seam, with per-hop SSRF validation and
cross-origin `Authorization`/`Cookie` stripping (ADR-0071, ADR-0081). A
reconnect always starts over at the configured URL, never at the redirect
target of a previous attempt.

### 3. Resubscription, not retry

The consumer runs one loop. Each iteration is one connect, one response
gate, and one read of the stream. The outcome decides the next step.

| Outcome | Class |
| --- | --- |
| transport send failure, connect timeout, idle timeout, mid-stream error | retried |
| `5xx` response | retried (body dropped unread) |
| EOF (the server closed the stream) | resubscribed |
| `204` | terminal |
| any other non-2xx status | terminal |
| `Content-Type` not `text/event-stream` | terminal |
| SSRF policy rejection, redirect policy rejection, DNS resolution failure | terminal |
| oversize event, invalid UTF-8 | terminal |

The DNS-resolution row is the deliberate boundary. The resolver call sits
behind the same seam as policy rejection, and a resolution failure is not a
transport failure. It ends the task, and route supervision (ADR-0007)
decides whether to restart the route. The loop does not retry it.

Retried and completed outcomes are both resubscriptions. The loop gates them
with `NetworkRetryPolicy::should_retry` and spaces them with `delay_for`
(ADR-0013).

The delay has two sources, both clamped:

- The default is `retryInitialMs` (3 s), with the policy multiplier `2.0`
  and `retryMaxMs` (60 s) as the cap.
- A server `retry:` field overrides the policy delay for every later
  reconnect, until a later valid `retry:` field changes it.
- Both sources are clamped into [1 s, 60 s] before the sleep.

Jitter is zero. One endpoint owns one connection, so there is no herd on a
shared endpoint to spread. A deterministic schedule also makes the
resubscription contract testable without clock or RNG injection.

The attempt budget is ten attempts. The URI carries no attempt knob. In
`NetworkRetryPolicy` a zero budget means unlimited, which would leave a
broken route retrying forever; ten attempts give supervision a bounded
point to act on.

The budget resets at one point only: the first complete frame parsed on the
new stream, an event or a comment. HTTP 200 does not reset it. Partial
bytes do not reset it. An endpoint that accepts the request and then stays
silent therefore consumes one attempt per idle cycle and drifts toward
exhaustion, instead of looping without limit.

Exhaustion returns a task error
(`sse: retry budget exhausted after N attempts`). The route records the
failure and the supervision policy decides the restart, per ADR-0007. The
consumer never restarts its own task.

### 4. Pinned bounds

| Bound | Value | Behavior past the bound |
| --- | --- | --- |
| `maxEventBytes` | 1 MiB (1 048 576) | non-retryable task error, mid-accumulation |
| non-`data` field line | 64 KiB | non-retryable task error |
| `idleTimeoutMs` | 90 000 | retriable: close and resubscribe |
| `connectTimeoutMs` | 10 000 per attempt | retriable |
| retry delay | [1 s, 60 s] | clamped at endpoint creation and at schedule time |
| resend id cap | 1 KiB | id delivered, never stored for `Last-Event-ID` |

`maxEventBytes` must be greater than zero, and the retry values are clamped
at endpoint creation. `maxEventBytes` bounds accumulated `data` bytes, so the
parser stops before allocating past the cap. A single field line is held to
64 KiB so a newline-less flood cannot grow the line buffer without bound.

There is no total-bytes cap and no max-events cap. An event stream is
endless by design: a cap on the stream total would make a healthy long-lived
subscription fail at an arbitrary point. The per-event bound is the memory
bound. Stream lifetime is a route concern, controlled by cancellation.

An event id over 1 KiB is still delivered on `CamelSseId`. It is not stored
for a resend, because a truncated id would corrupt the resubscription, and
storing the full id would let a server dictate memory growth.

### 5. Event-to-exchange mapping

Each complete event becomes one InOnly exchange.

- The body is always `Body::Text`. Multi-line `data:` values join with
  `\n`, and the trailing `\n` of the block is dropped. The body is never
  parsed: JSON stays text until a route step unmarshals it.
- A comment-only block and a block with empty `data` emit nothing. Comments
  still count as received bytes for the idle timer.
- Exactly one UTF-8 BOM is stripped at stream start. A BOM inside the stream
  passes through verbatim. CRLF, CR, and LF line endings are equivalent.
- A stream that ends mid-block dispatches nothing for that block, per the
  SSE grammar: dispatch happens only on the blank-line delimiter.

| Header | Source | Presence |
| --- | --- | --- |
| `CamelSseEvent` | `event:` field | always; `message` when the field is absent |
| `CamelSseId` | `id:` field | omitted when absent or empty |
| `CamelSseRetry` | `retry:` field, decimal milliseconds | omitted when absent or invalid |

There is no internal buffering channel. Each exchange is submitted with
`ctx.send(exchange).await`, and the read of the next chunk happens only
after that call completes. Back-pressure is the pipeline's, per ADR-0044.

### 6. Recorded deviation: invalid UTF-8 fails the task

The SSE specification (WHATWG) replaces invalid UTF-8 in `data` with
U+FFFD. This consumer does not. `data` bytes accumulate raw and are decoded
strictly at dispatch. Invalid UTF-8 ends the task with
`sse event contains invalid UTF-8 data`.

The deviation is deliberate. Replacement corrupts payloads silently: a
route would receive text that no producer wrote, with no signal that
anything went wrong. A failed task names the problem through route state and
the metric label, and supervision can alert on it.

Both malformed-input classes increment exactly one metric error label for
the route: `b-prime:sse:utf8` for invalid UTF-8 and `b-prime:sse:oversize`
for an event or field line past its bound. The labels are matched by exact
message equality, so the two classes stay distinguishable.

The strict rule covers the event data payload. Field values that never
become the body (`event:`, `id:`) are read lossily.

### 7. Security re-validation on every connect

Nothing about a previous connect is trusted. Every attempt, including every
reconnect, runs the full pass:

- SSRF validation on the translated URL under the endpoint's
  `allowInternal`/`allowCleartext` policy.
- A fresh DNS resolution, then client construction pinned to the resolved
  addresses. The pinned-client cache is keyed by host and addresses, so a
  changed resolution cannot reuse a client pinned to the old addresses.
- The TLS configuration, including the per-host expired-certificate
  allowlist and its chain rules, through the shared `build_client` path.
- The redirect seam with per-hop validation and cross-origin credential
  stripping.

A redirect observed during a previous connect is never a target. The next
attempt starts from the configured URL and re-follows redirects only inside
that attempt.

### 8. Out of scope (MVP)

- No request bodies. The consumer sends `GET` only, so there is no POST-body
  form and no body option.
- No persisted ids. `Last-Event-ID` is memory-only and resets on restart; a
  durable cursor needs a store contract this component does not own.
- No max-events cap and no total-bytes cap, per decision 4.
- No attempt-count URI option. The budget stays a policy constant.
- No change to the `http:` producer path, its bounds, or its tests.
- No new dependency, and no separate crate.

## Consequences

- `sse:` appears in the component metadata and the generated schema with the
  options above and consumer-only capabilities. A route that uses `sse:` on
  the producer side fails at endpoint creation with a clear message.
- A stream that cannot be established, or that keeps breaking, past ten
  reconnect attempts fails the route. Operators who want automatic recovery
  configure a supervision policy (ADR-0007); the consumer does not restart
  itself.
- A transient resolver outage ends the route instead of being retried
  in-loop. The trade-off is accepted: the resolver seam cannot distinguish a
  transient outage from a policy rejection, and supervision carries the
  backoff. A wrong `Content-Type` or a `204` fails fast, which surfaces a
  misconfigured URL or a non-SSE endpoint immediately.
- Invalid UTF-8 fails the route where the specification would have degraded
  the payload. A producer that sends non-UTF-8 `data` must fix the stream.
  Routes that need byte-fidelity for another encoding can base64 the payload
  at the producer.
- Memory is bounded per event, not per stream. A server that sends endless
  valid events is bounded only by back-pressure and route lifetime. A
  stream that goes silent is closed by the idle timeout, which detects a
  dead connection rather than capping volume.
- Zero jitter makes reconnect timing deterministic. Several routes against
  the same origin reconnect in lockstep after a shared outage. Operators can
  stagger them with per-URI `retryInitialMs`.
- ADR-0060 stands for the MCP component. This ADR changes the scope of its
  SSE exclusion note only.
