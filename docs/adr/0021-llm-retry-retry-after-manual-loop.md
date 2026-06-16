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
