# Proposal: retry-after-cap

## Why

`run_with_retry` (`crates/components/camel-component-llm/src/producer.rs`) sleeps the
provider's `Retry-After` value as-is. A 429 with `Retry-After: 86400` parks the exchange
for a day. In materialized mode the parked exchange holds a semaphore permit-free slot or
a route task; `llm:decide` and chat have no total deadline unless `timeout_secs` is
configured, so nothing bounds that sleep. Provider guidance (OpenAI, Anthropic) expresses
`Retry-After` in seconds to minutes, not hours; a day-long sleep is a provider bug or an
attack amplified by our trust in the header. Raised by the Opus review of `rc-3lnv3`; bd
`rc-76oc7`.

## What Changes

- Cap the `Retry-After` sleep in `run_with_retry`:
  `cap = min(network_retry.max_delay, remaining total deadline)`. The configured
  `max_delay` (default 30 s) already bounds exponential backoff; the `Retry-After` path
  now obeys the same bound, tightened by the remaining deadline when one is set.
- Emit an honest error when the provider asks for a longer wait than the cap: a new
  retryable `LlmError::RetryAfterCapped { requested, capped_to }` whose display names
  both values. The capped rate limit stays retryable: the loop sleeps the cap and
  retries.
- Give `llm:decide` a default total deadline (`DEFAULT_DECIDE_TOTAL_TIMEOUT_SECS = 120`,
  applied when neither provider nor global `timeout_secs` is set). A configured
  `timeout_secs` still wins. The decision path can no longer retry forever.
- Document the cap and the decide deadline in `docs/src/components/llm.md` ("Retry and
  timeout"), the config reference, the `llm-provider-adapter` and `llm-decisions` specs,
  and an ADR-0021 addendum.

Excluded: changes to the exponential backoff sequence for values under the cap; new
dependencies; response-size caps or client policy values (565-class work is done);
`is_retryable` semantics for rate limits (a capped rate limit stays retryable).

## Acceptance criteria

- A 429 with `Retry-After: 86400` and a configured cap sleeps the cap (virtual time),
  never the header value; the surfaced error names both values.
- A 429 with `Retry-After: 3` under a larger cap sleeps 3 s; behavior is unchanged.
- A 429 without `Retry-After` still uses exponential backoff, unchanged.
- With a total deadline, the `Retry-After` sleep cannot extend past the deadline.
- `llm:decide` with no `timeout_secs` completes or fails within the default total;
  with `timeout_secs`, the configured value wins.
- All tests use virtual time (`tokio::time` pause/advance); no multi-second real sleeps.

## Risk budget

- The change is confined to `crates/components/camel-component-llm` (+ docs/specs).
- Adding a variant to the `#[non_exhaustive]` `LlmError` enum is additive; downstream
  matches already carry a wildcard arm.
- Risk accepted: operators who deliberately set `max_delay_ms` above provider norms get
  the sleep they configured — the cap is a configured bound, not a hardcoded second
  opinion.
- Out of bounds: touching transport bounds (connect 10 s, read-idle 600 s) or the
  per-attempt decision HTTP timeout (30 s default).
