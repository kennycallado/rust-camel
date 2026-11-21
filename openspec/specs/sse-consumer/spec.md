# sse-consumer Specification

## Purpose
TBD - created by archiving change sse-consumer. Update Purpose after archive.
## Requirements
### Requirement: SSE event mapping

The `sse:` consumer SHALL emit one `InOnly` exchange per complete SSE event,
with the event data as the message body.

#### Scenario: One exchange per event

- **GIVEN** a connected `sse:` consumer receiving a stream of three complete
  SSE events
- **WHEN** the events are dispatched to the route
- **THEN** exactly three `InOnly` exchanges arrive, in stream order, one per
  event

#### Scenario: Body is always text

- **GIVEN** an SSE event whose data payload is arbitrary bytes that decode
  as UTF-8
- **WHEN** the exchange is built
- **THEN** the message body is `Body::Text` containing exactly the event
  data, regardless of payload content

#### Scenario: Multi-line data joined

- **GIVEN** an SSE event block with three consecutive `data:` lines
- **WHEN** the event is framed
- **THEN** the body is the three values joined with `\n` in order, with no
  trailing `\n`

#### Scenario: Data never parsed as JSON

- **GIVEN** an SSE event whose data is a syntactically valid JSON object
- **WHEN** the exchange is built
- **THEN** the body remains the raw text (no auto-detection, no JSON
  conversion, no structured body)

### Requirement: SSE event headers

The consumer SHALL map SSE fields to exchange headers: `CamelSseEvent` from
the `event:` field (default `message`), `CamelSseId` from the `id:` field
(omitted when absent), and `CamelSseRetry` from the `retry:` field (omitted
when absent).

#### Scenario: Event name header

- **GIVEN** an SSE event block with `event: tick`
- **WHEN** the exchange is built
- **THEN** the exchange carries header `CamelSseEvent` = `tick`

#### Scenario: Default event name

- **GIVEN** an SSE event block with no `event:` field
- **WHEN** the exchange is built
- **THEN** the exchange carries header `CamelSseEvent` = `message`

#### Scenario: Event id header

- **GIVEN** an SSE event block with `id: 42`
- **WHEN** the exchange is built
- **THEN** the exchange carries header `CamelSseId` = `42`

#### Scenario: Retry field header

- **GIVEN** an SSE event block with `retry: 5000`
- **WHEN** the exchange is built
- **THEN** the exchange carries header `CamelSseRetry` = `5000`

### Requirement: Non-event block suppression

The consumer SHALL NOT emit an exchange for comment-only blocks or blocks
with empty data.

#### Scenario: Comment-only block suppressed

- **GIVEN** the stream contains a block consisting only of a comment line
  (`:keep-alive`)
- **WHEN** the block completes
- **THEN** no exchange is emitted and the route observes no exchange

#### Scenario: Empty-data block suppressed

- **GIVEN** the stream contains an event block whose data value is empty
- **WHEN** the block completes
- **THEN** no exchange is emitted

### Requirement: Stream charset handling

The consumer SHALL strip exactly one UTF-8 BOM at stream start only, and
SHALL accept CRLF, CR, and LF line endings interchangeably.

#### Scenario: One BOM stripped at start

- **GIVEN** a stream that begins with a UTF-8 BOM followed by an event
- **WHEN** the event is framed
- **THEN** the body contains the event data without the BOM

#### Scenario: BOM mid-stream not stripped

- **GIVEN** a stream where a BOM-like byte sequence appears in the middle of
  an event's data
- **WHEN** the event is framed
- **THEN** the bytes are preserved verbatim in the body

#### Scenario: Mixed line endings

- **GIVEN** a stream where one event uses CRLF, the next CR, and the next
  LF line endings
- **WHEN** the events are framed
- **THEN** all three events frame identically to their LF-only equivalents

### Requirement: Bounded consumption

The `sse:` consumer SHALL enforce the pinned bounds: `maxEventBytes` default
1 MiB (larger events are a task error), `idleTimeoutMs` default 90 000 reset
by ANY received bytes including comments, `connectTimeoutMs` default 10 000
per connect attempt, retry delay clamped to [1 s, 60 s], and event ids
longer than 1 KiB delivered but never stored for resend.

#### Scenario: Oversize event rejected

- **GIVEN** `maxEventBytes` at its 1 MiB default and a server sending a
  single event whose data exceeds 1 MiB
- **WHEN** the event exceeds the bound mid-accumulation
- **THEN** the consumer fails as a non-retryable task error before
  allocating beyond the bound

#### Scenario: Idle timeout includes comments

- **GIVEN** a connected stream that sends only comment bytes
- **WHEN** comments keep arriving within `idleTimeoutMs`
- **THEN** the consumer stays connected (comment bytes reset the idle timer)

#### Scenario: Idle timeout fires on silence

- **GIVEN** a connected stream that sends no bytes
- **WHEN** `idleTimeoutMs` elapses with zero bytes received
- **THEN** the connection is treated as a retriable transport failure and
  the consumer reconnects

#### Scenario: Connect timeout bounded

- **GIVEN** a server that accepts the TCP connection but never completes the
  HTTP response head
- **WHEN** `connectTimeoutMs` (default 10 000) elapses
- **THEN** the attempt is treated as a retriable transport failure

#### Scenario: Retry delay clamped

- **GIVEN** a computed retry delay (from `retryInitialMs`/`retryMaxMs` or a
  server `retry:` field) below 1 s or above 60 s
- **WHEN** the reconnect sleep is scheduled
- **THEN** the sleep is clamped into [1 s, 60 s]

#### Scenario: Oversize id not stored

- **GIVEN** an event whose `id:` field is longer than 1 KiB
- **WHEN** the event is framed and delivered
- **THEN** the exchange is emitted normally and no `Last-Event-ID` is sent
  on the next reconnect (the oversized id is not stored)

### Requirement: Terminal versus retried failures

The consumer SHALL reconnect (resubscribe) on transport errors and 5xx
responses only, and SHALL fail terminally — never reconnecting — on 204, any
other non-2xx status, or a response whose `Content-Type` is not
`text/event-stream`.

#### Scenario: 204 is terminal

- **GIVEN** a server that answers a subscribe request with `204`
- **WHEN** the response arrives
- **THEN** the consumer fails as a task error and no reconnect attempt is
  made

#### Scenario: Non-2xx is terminal

- **GIVEN** a server that answers with `403`
- **WHEN** the response arrives
- **THEN** the consumer fails as a task error without reconnecting

#### Scenario: Wrong content type is terminal

- **GIVEN** a server that answers `200` with `Content-Type: text/html`
- **WHEN** the response head arrives
- **THEN** the consumer fails as a task error without reconnecting

#### Scenario: Transport error retried

- **GIVEN** a connected consumer whose TCP stream breaks mid-stream
- **WHEN** the read fails with a transport error
- **THEN** the consumer reconnects after a clamped cancelable sleep

#### Scenario: 5xx retried

- **GIVEN** a server that answers the first subscribe with `503` and the
  second with a valid event stream
- **WHEN** the reconnect loop runs
- **THEN** the consumer retries after the clamped delay and consumes events
  from the second response

### Requirement: Reconnect target and delay

The consumer SHALL always reconnect to the configured endpoint URL (never a
redirect target observed on a previous connect), SHALL schedule the
reconnect sleep with the network retry policy
(`NetworkRetryPolicy::should_retry`/`delay_for`), defaulting to 3 s when no
server `retry:` field is in effect, and SHALL let a server `retry:` field
override the delay (clamped to [1 s, 60 s]) for subsequent reconnects.

#### Scenario: Reconnect goes to the configured URL

- **GIVEN** a connect that followed one or more redirects to an origin other
  than the configured URL before breaking mid-stream
- **WHEN** the consumer reconnects
- **THEN** the new connect targets the configured URL (redirects are
  re-followed only within that connect), never the previous redirect target
  directly

#### Scenario: Default reconnect delay

- **GIVEN** a transport failure with no server `retry:` field in effect
- **WHEN** the reconnect is scheduled
- **THEN** the delay is the 3 s default (subject to the [1 s, 60 s] clamp)
  and the retry policy's `should_retry` gate decides whether the attempt
  budget allows it

#### Scenario: Server retry field overrides the delay

- **GIVEN** a stream that carried `retry: 5000` before a transport failure
- **WHEN** the reconnect is scheduled
- **THEN** the sleep uses 5 s (the server override), clamped to
  [1 s, 60 s], until a later valid `retry:` field changes the override

### Requirement: Resubscription bookkeeping

The consumer SHALL resend `Last-Event-ID` (memory only) on reconnects when a
stored id exists, SHALL reset the retry budget when the FIRST complete event
or comment is parsed after a reconnect (never on HTTP 200), and SHALL
surface budget exhaustion as a task error the route supervises (ADR-0007).

#### Scenario: Last-Event-ID resent

- **GIVEN** a consumer that has received an event with `id: 42` before a
  transport break
- **WHEN** the consumer reconnects
- **THEN** the new request carries `Last-Event-ID: 42`

#### Scenario: No id, no header

- **GIVEN** a consumer that has not yet received any event id
- **WHEN** the consumer reconnects after a transport failure
- **THEN** the new request carries no `Last-Event-ID` header

#### Scenario: Budget reset on first post-reconnect event

- **GIVEN** a consumer that reconnected after N failed attempts
- **WHEN** the first complete event or comment is parsed on the new stream
- **THEN** the retry attempt counter resets to zero (a later failure starts
  the budget anew, regardless of the earlier HTTP 200)

#### Scenario: Budget exhaustion is a task error

- **GIVEN** a retry budget that exhausts while the endpoint stays down
- **WHEN** the last allowed attempt fails
- **THEN** the consumer returns a task error and route supervision
  (ADR-0007) owns recovery — the consumer does not self-supervise

### Requirement: Route back-pressure

The consumer SHALL have no internal buffering channel and SHALL await
`ConsumerContext::send` completion before reading the next stream bytes
(ADR-0044).

#### Scenario: Next read waits for route

- **GIVEN** a route whose processing is slow (a barrier that has not
  admitted the previous exchange)
- **WHEN** the consumer has an event pending dispatch
- **THEN** the consumer does not read further stream bytes until
  `ctx.send().await` for the pending event completes

### Requirement: Malformed input fails loudly

The consumer SHALL treat oversize events and invalid UTF-8 as non-retryable
task errors, each incrementing exactly one metric label
(`b-prime:sse:oversize`, `b-prime:sse:utf8`), and SHALL record the deviation
from the SSE U+FFFD replacement rule (invalid UTF-8 fails the task instead
of replacing).

#### Scenario: Oversize event metric

- **GIVEN** an event exceeding `maxEventBytes`
- **WHEN** the task error is raised
- **THEN** the metric error label `b-prime:sse:oversize` is incremented once
  for the route

#### Scenario: Invalid UTF-8 fails with metric

- **GIVEN** an event block whose data bytes are not valid UTF-8
- **WHEN** the consumer frames the event
- **THEN** the consumer raises a non-retryable task error (no U+FFFD
  replacement) and increments the metric error label `b-prime:sse:utf8`
  once

### Requirement: Transport security on every connect

The consumer SHALL apply SSRF validation with DNS pinning, the TLS
allowlist, ADR-0081 redirect handling (Authorization stripped across
origins, per hop), and userinfo rejection on EVERY connect attempt,
including every reconnect.

#### Scenario: SSRF and pinning re-applied per reconnect

- **GIVEN** a consumer that reconnects to the configured URL
- **WHEN** each reconnect attempt runs
- **THEN** SSRF validation and a fresh DNS resolution with address pinning
  are performed for that attempt (no cached pre-connect resolution bypass)

#### Scenario: TLS allowlist honored

- **GIVEN** an `sse:` URL whose host has a pinned expired-certificate entry
  in the TLS allowlist
- **WHEN** the consumer connects over TLS
- **THEN** the pinned certificate is accepted exactly per the allowlist
  rules shared with the `https:` producer

#### Scenario: Redirect auth stripped cross-origin

- **GIVEN** a configured `Authorization` header and a server that redirects
  to a different origin during connect
- **WHEN** the redirect hop is followed (ADR-0081)
- **THEN** the `Authorization` header is stripped on the cross-origin hop

#### Scenario: Redirects only on connect

- **GIVEN** a consumer streaming events from a final response after one or
  more connect redirects
- **WHEN** the stream later breaks and the consumer reconnects
- **THEN** redirect following happens only within the new connect phase,
  never mid-stream

#### Scenario: Userinfo rejected

- **GIVEN** an `sse:` URL containing userinfo (`user:pass@host`)
- **WHEN** the endpoint is created
- **THEN** creation fails with a configuration error before any connect

#### Scenario: Credentials only from config headers

- **GIVEN** an endpoint configured with credential headers (e.g.
  `Authorization`, API keys) on the URI
- **WHEN** each connect (including reconnects) is made
- **THEN** credential material is taken only from those configured headers —
  `sse:` defines no username/password/bearer parameters of its own — while
  protocol headers the consumer itself requires (`Accept`,
  `Last-Event-ID`, framing headers) are always present, and ADR-0081
  stripping on cross-origin redirect hops applies on top of this rule

#### Scenario: Scheme rides TLS by default

- **GIVEN** an `sse://` URL targeting a public host
- **WHEN** the consumer connects
- **THEN** the wire request uses `https://` (the `sse:` scheme is
  translated for transport); cleartext `http://` is used only where the
  existing egress policy allows it — loopback/`allowInternal=true` targets,
  or public hosts with explicit `allowCleartext=true` (ADR-0081) — and all
  SSRF validation, DNS pinning, and redirect handling run on the translated
  URL

### Requirement: Cancellation

The consumer SHALL honor cancellation promptly both mid-read (waiting on
stream bytes) and mid-sleep (waiting in the reconnect backoff), without
emitting partial exchanges.

#### Scenario: Cancel mid-read

- **GIVEN** a connected consumer blocked reading a stream that sends no
  bytes
- **WHEN** the route/ConsumerContext cancel token fires
- **THEN** the consumer stops promptly and no further exchange is emitted

#### Scenario: Cancel mid-sleep

- **GIVEN** a consumer sleeping in a reconnect backoff
- **WHEN** the cancel token fires during the sleep
- **THEN** the sleep is abandoned promptly and the consumer stops without
  another connect attempt

### Requirement: MVP exclusions

The `sse:` consumer SHALL NOT accept request bodies, SHALL NOT persist event
ids across restarts, SHALL NOT enforce a max-events cap, SHALL NOT enforce a
total-bytes cap across the stream, and SHALL leave the `http:` producer
read-whole-stream path unchanged.

#### Scenario: Exclusions hold

- **GIVEN** the `sse:` scheme surface as delivered
- **WHEN** the endpoint options and behavior are inspected
- **THEN** there is no body/request option, no durable id store, no
  max-events option, no total-bytes cap (only the per-event
  `maxEventBytes` bound exists), and the `http:` producer path shows no
  behavioral change (its existing bounds and tests unchanged)

