# Proposal: sse-consumer

## Why

camel-http has no Server-Sent Events consumer. The `http:` producer reads the
whole response into one body (`HttpProducer::call`, bounded by
`read_timeout_ms` / `max_response_bytes`): an SSE stream never ends, so it
either times out or exceeds the cap, and a finite one still yields a single
exchange instead of one exchange per event. There is no SSE framing parser
anywhere in the tree. Integration points that need to consume SSE endpoints
(LLM feeds, webhook-style notifications) currently cannot do so without
hand-rolled client code. This is the consume half of epic rc-bs25c (bd
rc-q48en, p2, designed in the e_opus review; decisions are pinned there).

## What Changes

- New `sse:` consumer scheme in `crates/components/camel-http`, registered by
  `HttpBundle`, consumer-only (no producer on `sse:`).
- One `InOnly` exchange per SSE event; body always `Body::Text`; headers
  `CamelSseEvent` / `CamelSseId` / `CamelSseRetry`.
- Small in-crate framing parser (no streaming framework, no new dependency):
  BOM, CRLF/CR/LF, multi-line `data:` join, comment/empty-block suppression.
- Resubscription loop (not producer retry): `NetworkRetryPolicy` +
  cancelable sleep, `Last-Event-ID` resent from memory, budget reset on the
  first event/comment after a reconnect; 204 / non-2xx / wrong content-type
  are terminal; transport errors and 5xx are retried.
- Bounded: `maxEventBytes` 1 MiB, `idleTimeoutMs` 90 000, `connectTimeoutMs`
  10 000, retry delay clamped [1 s, 60 s], event id capped at 1 KiB.
- Reuses existing crate seams: `ssrf`, `tls_allowlist`, `client_cache`
  (validation + DNS pinning re-applied on every reconnect), ADR-0081
  redirect handling per hop.
- Malformed oversize / invalid UTF-8 are non-retryable task errors with one
  metric label each.
- The producer's read-whole-stream path is NOT touched.
- Out of scope (explicit MVP exclusions): the serve half (rc-o1b9v), MCP
  dns_pin fold (rc-yoawh), POST bodies, persisted event ids, max-events cap,
  total-bytes cap.

## Acceptance criteria

- One exchange per event with the header mapping and the pinned bounds.
- Resubscription with `Last-Event-ID` and the named budget-reset point.
- 204 / non-2xx / wrong content type terminal; transport / 5xx retried.
- SSRF and DNS pinning re-applied per reconnect; ADR-0081 per redirect hop.
- Oversize and invalid UTF-8 fail loudly with metrics.
- Cancellation works mid-read and mid-sleep.
- `cargo xtask schema` regenerated; ADR merged; capability spec with a
  scenario per rule; camel-http docs section with runnable example.

## Risk budget

- Acceptable: consumer loop complexity (backoff, reconnect) — mitigated by
  the pinned design and unit-testable framing parser.
- Not acceptable: changes to `http:` producer semantics, new streaming
  dependencies, self-supervised reconnection beyond ADR-0007 (budget
  exhaustion must surface as a task error the route supervises).

Affected crates: `camel-http` (scheme, parser, consumer, tests),
`camel-api`/`camel-core` only if schema regeneration shows drift.
bd: rc-q48en. Domain language per CONTEXT-MAP.md (Consumer, InOnly,
ConsumerContext::send back-pressure, ConsumerRestart supervision).
