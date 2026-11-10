# ADR-0021: LLM Retry Honors Provider retry_after via Manual Loop

**Date:** 2026-06-13
**Status:** Accepted

## Decision

camel-component-llm uses a manual retry loop (`NetworkRetryPolicy::should_retry`
+ `delay_for`) instead of the shared `retry_async` helpers from ADR-0013.

## Context

`LlmError::RateLimit` carries `retry_after: Option<Duration>` — a
provider-requested back-off that must override exponential backoff when
present. The `retry_async` helpers do not accept a per-attempt delay override.

## New manual-retry criterion

**Per-attempt delay override.** When the error specifies a delay
(`retry_after`), that delay replaces the policy's computed backoff. ADR-0013's
existing "stay manual" criteria (mutable state across `await`, retry spanning
a polling/event-stream loop, async pre-retry lifecycle side effects,
non-idempotent resend) do not cleanly cover this.

The loop is also justified by two hardening-spec decisions:

- **Permit release during backoff** — the semaphore permit is dropped before
  the backoff `sleep` and re-acquired on the next attempt, freeing the
  concurrency slot for other requests during the wait.
- **Total timeout wraps everything** — one `tokio::time::timeout` deadline
  encloses all attempts and all backoff sleeps; there is no per-attempt
  timeout (which would let N attempts run for N × timeout).

## Consequences

- One explicit loop in the LLM producer, with a comment pointing here.
- If a second component needs per-attempt delay override, extend
  `retry_async_cancelable` with a `delay_override` callback at that time
  (do not pre-emptively grow camel-component-api).
- Cancellation is delivered via Rust's standard future-drop semantics
  (dropping `Body::Stream` drops the inner provider stream). No explicit
  `CancellationToken` is needed — in Rust, drop IS cancellation.

## Addendum (2026-11, llm-timeout-deadline)

The hidden 30s client total (`hardened_http_client_with_pinning`) was itself a
per-attempt timeout, which this ADR forbids. That total is removed. The
single-deadline rule is restored: the configured deadline (`timeout_secs`) owns
the total (materialized `apply_timeout`, streaming per-event, decision
per-request).

Transport bounds are now: a 10s connect timeout; plus a 600s transport
read-idle guard on the chat/embed client. The guard measures inactivity, not
lifetime, so it is a different kind of bound than the per-attempt lifetime
timeout this ADR forbids. The decision client carries no read-idle guard; its
per-request total is unchanged.

Response bodies are capped. Chat payloads are capped at 8 MiB while they
stream or accumulate (the genai stream loop and the materialized producer
loop); embed payloads are capped at 64 MiB at the conversion boundary, after
the provider call returns. A breach fails the request with
`LlmError::ResponseTooLarge { path, cap_bytes, read_bytes }`, which names the
site (`chat_stream`, `chat_materialized`, or `embed`), the cap, and the bytes
read. The error is not retryable. Payloads under the cap are unchanged. bd
`rc-kemrg` (body-size cap) is implemented.

Residual, not fixed here: genai SDK-internal buffering — SSE lines, oversized
individual events, captured tool arguments, the embed JSON body, and genai
error bodies — remains unbounded, as does direct streaming from non-genai
providers through the producer passthrough. The caps bound payload size only:
a peer that trickles bytes under 600s of inactivity still holds a connection
indefinitely when `timeout_secs` is unset. Set `timeout_secs` for a lifetime
bound. Follow-ups: bd `rc-v8skx` (genai SDK-side bounds) and bd `rc-ucneu`
(Keycloak hardened client).

## Addendum (2026-11, retry-after-cap)

The per-attempt delay override is bounded. A `Retry-After` above
`min(network_retry.max_delay, remaining deadline)` is not slept as-is. The loop
sleeps the cap, stays retryable, and surfaces
`LlmError::RetryAfterCapped { requested, capped_to }`, whose text names both
values. `capped_to` is the calculated bound, not proof of a completed sleep. A
zero cap with a positive header returns the error at once, so an unlimited
policy cannot spin on a truncated wait. A header at or under the cap is slept
as-is, and exponential backoff is unchanged. bd `rc-76oc7` (retry-after cap) is
implemented.

The single-deadline rule is unchanged. The cap reads the existing deadline as a
bound and adds no timeout. Backoff sleeps are not re-capped by the deadline:
the outer `apply_timeout` cuts them short, as before.

The decide path carries a default 120 s total
(`DEFAULT_DECIDE_TOTAL_TIMEOUT_SECS`) when no `timeout_secs` is configured. The
earlier note that the decision per-request total is unchanged still holds for
the deadline itself: the total is configured-or-default, never absent. The
default is not a config field; set `timeout_secs` for a different total.
