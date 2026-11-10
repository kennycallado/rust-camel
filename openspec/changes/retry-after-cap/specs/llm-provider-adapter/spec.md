## MODIFIED Requirements

### Requirement: Provider errors map to the LlmError taxonomy with correct retryability

The adapter SHALL map genai errors onto the project `LlmError` taxonomy. On the
STREAM path it SHALL downcast the boxed `Error::WebStream { error }` to
`HttpError` (status/body) and to `reqwest::Error` (`is_timeout`/`is_connect`)
before mapping; an unmappable stream error falls to `LlmError::provider(..)`.
Mapping: 401 and 403 → `AuthFailed`; 404 for a missing model → `ModelNotFound`;
400 and 422 → `InvalidRequest`; 5xx → `ProviderUnavailable`; `StreamParse` →
`Protocol`; connect errors → `Network`. A 429 SHALL be disambiguated by body —
quota detection matches `error.code == "insufficient_quota"` or
`error.type == "insufficient_quota"` → `QuotaExceeded` (NOT retryable); any
other 429 → `RateLimit` (retryable). On the stream path genai drops response
headers, so chat-stream 429 maps to `RateLimit { retry_after: None }` (same as
today). On the embed path the `Retry-After` header from
`ResponseFailedStatus.headers` SHALL be honoured when it is a delta-seconds
value; an HTTP-date `Retry-After` maps to `retry_after: None`. A mid-stream
error SHALL surface as a mapped `Err` item and SHALL NOT be followed by a
`Finished` event. Timeout errors SHALL name the layer that fired: a connect
timeout → `TransportTimeout { phase: Connect, after: connect bound }`
(retryable); a read-idle lapse → `TransportTimeout { phase: Idle, after:
read-idle bound }` (retryable); the producer deadline lapse →
`Timeout(deadline)` (retryable). Every timeout display SHALL contain the word
`timeout` and the duration that fired. On the decision path a transport
timeout inside a per-request total maps to `Timeout(request_total)` unchanged,
and a connect timeout maps to `TransportTimeout { Connect }`. The transport
SHALL NOT report a configured duration it did not enforce. In materialized
mode with `network_retry` enabled, a transport timeout inside one chat attempt
SHALL be retried only while the manual-loop conditions hold — the retry policy
permits another attempt, the configured deadline has not lapsed, and no
non-empty content Delta has arrived; once a content Delta has arrived the loop
SHALL NOT retry. With `network_retry` disabled, or in streaming mode, there is
no retry.

In the manual retry loop (`run_with_retry`), a `RateLimit.retry_after` greater
than the sleep cap SHALL NOT be slept as-is. With no total deadline configured
the cap is `network_retry.max_delay`; with a total deadline configured the cap
is the minimum of `network_retry.max_delay` and the remaining budget. The
loop SHALL evaluate, in order: a spent budget (`remaining` zero, including a
sleep that just ended at the deadline) SHALL return `Timeout(total)` without
launching another attempt; a zero cap with a positive header SHALL return
`RetryAfterCapped` immediately (no sleep, no further attempt, so an
unlimited-retry configuration cannot busy-spin on a truncated wait); a header
at or under the cap (equality included, `Retry-After: 0` under a zero cap
included) SHALL be slept as-is (`Duration::ZERO` sleeps nothing) and SHALL
NOT be annotated; a header above a positive cap SHALL make the loop log a
warning naming both durations, sleep the cap, and carry
`RetryAfterCapped { requested, capped_to }` — a retryable variant whose
display names both durations, where `capped_to` is the calculated bound and
not proof of a completed sleep — as the error exhaustion surfaces. The
normalization SHALL apply to an eligible rate-limit failure at every
post-policy return point, so a final over-cap `RateLimit` on the exhaustion
return is surfaced as `RetryAfterCapped`, and a later success or a different
failure SHALL NOT be replaced by a stale capped error. When the computed cap
is zero because the budget is spent, the loop SHALL return `Timeout(total)`
without launching another attempt (the sleep ending exactly at the deadline
launches no further request). N attempts SHALL produce at most N−1 sleeps.
`RetryAfterCapped` SHALL be added to the retryable set; a capped rate limit
stays retryable after the cap. A rate limit without `Retry-After` SHALL keep
the exponential backoff sequence unchanged (`delay_for`, clamped to
`max_delay`, jitter included); exponential backoff delays SHALL NOT be
re-capped by the remaining budget — the outer `apply_timeout` deadline
continues to cut those sleeps short. The raw-error returns for a disabled or
absent policy, for streaming mode, and for content-started requests SHALL be
unchanged.

#### Scenario: chat stream 429 is retryable without retry-after

- **GIVEN** a wiremock chat endpoint answering 429 with a rate-limit (non-quota)
  body
- **WHEN** `chat_stream` consumes the stream
- **THEN** the stream errors with `LlmError::RateLimit { retry_after: None }`
  and `is_retryable` is true

#### Scenario: embed 429 honours delta-seconds Retry-After

- **GIVEN** a wiremock embed endpoint answering 429 with header
  `Retry-After: 7` and a rate-limit body
- **WHEN** `embed` is called
- **THEN** the error is `LlmError::RateLimit { retry_after: Some(Duration::from_secs(7)) }`

#### Scenario: embed 429 with HTTP-date Retry-After maps to None

- **GIVEN** a wiremock embed endpoint answering 429 with an HTTP-date
  `Retry-After` header
- **WHEN** `embed` is called
- **THEN** the error is `LlmError::RateLimit { retry_after: None }`

#### Scenario: 429 insufficient_quota is not retryable

- **GIVEN** a wiremock endpoint answering 429 with a body whose `error.code` or
  `error.type` is `insufficient_quota`
- **WHEN** the error is mapped (either operation)
- **THEN** the result is `LlmError::QuotaExceeded` and `is_retryable` is false

#### Scenario: 401 auth and 503 unavailable

- **GIVEN** wiremock endpoints answering 401 and 503 respectively
- **WHEN** the errors are mapped
- **THEN** 401 yields `AuthFailed` and 503 yields `ProviderUnavailable`
  (retryable per `error.rs`)

#### Scenario: connection refused maps to Network

- **GIVEN** a chat endpoint that refuses connections
- **WHEN** `chat_stream` runs against it
- **THEN** the error maps to `Network` (retryable)

#### Scenario: connection refused and timeout on chat stream

- **GIVEN** a chat endpoint that refuses connections, and a configured timeout
  exceeded against a stalled wiremock endpoint
- **WHEN** `chat_stream` runs against each
- **THEN** connection refused maps to `Network` (retryable) and timeout maps to
  `Timeout` carrying the configured duration (retryable)

#### Scenario: connect timeout names the connect layer

- **GIVEN** a chat endpoint that accepts no TCP connection within the connect
  bound
- **WHEN** the connection attempt lapses
- **THEN** the error is `TransportTimeout { phase: Connect, .. }` (retryable)
  whose display contains `timeout` and the connect bound

#### Scenario: read-idle lapse names the idle layer

- **GIVEN** a chat endpoint that sends headers and then stalls, with a test
  read-idle bound
- **WHEN** the idle bound lapses before the producer deadline
- **THEN** the error is `TransportTimeout { phase: Idle, .. }` (retryable)
  whose display contains `timeout` and the idle bound

#### Scenario: producer deadline keeps Timeout with the configured duration

- **GIVEN** a stalled chat endpoint with `timeout_secs = D` smaller than the
  read-idle bound
- **WHEN** the producer deadline lapses
- **THEN** the error is `Timeout(D)` whose display contains `timeout` and `D`

#### Scenario: read-idle guard is a per-read inactivity bound, not a total

- **GIVEN** a chat endpoint streaming one chunk every 100 ms for 1.2 s under a
  300 ms test read-idle bound
- **WHEN** the stream is consumed
- **THEN** it completes with the full text — the idle timer resets on every
  read

#### Scenario: Transport timeout before content is retried

- **GIVEN** materialized mode, `network_retry` with `max_attempts = 2`, and a
  staged endpoint whose first attempt stalls before any content Delta (the
  read-idle guard fires) and whose second attempt answers normally
- **WHEN** the producer runs
- **THEN** it succeeds and the server recorded 2 requests

#### Scenario: Transport timeout after content is not retried

- **GIVEN** materialized mode with `network_retry` enabled, and a staged
  endpoint whose first attempt streams a non-empty content Delta and then
  stalls
- **WHEN** the producer runs
- **THEN** it fails with the mapped transport timeout and the server recorded
  1 request

#### Scenario: Retry-After beyond the cap sleeps the cap and names both values

- **GIVEN** a materialized producer with `network_retry` jitter-free,
  `max_attempts = 3`, `max_delay_ms = 60000`, no deadline, and a provider that
  answers 429 with `Retry-After: 86400` on every attempt (virtual time)
- **WHEN** the retry loop exhausts the attempts (3 attempts, 2 sleeps)
- **THEN** the loop sleeps 60 s (the cap) per retry, never 86400 s, and the
  surfaced error is `RetryAfterCapped { requested: 86400s, capped_to: 60s }`
  whose display names both `86400s` and `60s`, and `is_retryable` is true

#### Scenario: Retry-After under the cap is honoured unchanged

- **GIVEN** a materialized producer with `network_retry` jitter-free,
  `max_attempts = 3`, `max_delay_ms = 60000`, and a provider that answers 429
  with `Retry-After: 3` on every attempt (virtual time)
- **WHEN** the retry loop exhausts the attempts (3 attempts, 2 sleeps)
- **THEN** the loop sleeps 3 s per retry and the surfaced error is the original
  `RateLimit { retry_after: Some(3s) }`, not `RetryAfterCapped`

#### Scenario: Retry-After equal to the cap is honoured, not annotated

- **GIVEN** a materialized producer with `network_retry.max_delay_ms = 3000`,
  no deadline, and a provider that answers 429 with `Retry-After: 3` on every
  attempt (virtual time)
- **WHEN** the retry loop exhausts the attempts
- **THEN** the loop sleeps 3 s per retry and the surfaced error is the original
  `RateLimit`, not `RetryAfterCapped`

#### Scenario: Rate limit without Retry-After keeps exponential backoff

- **GIVEN** a materialized producer with `network_retry` jitter-free
  (`initial_delay_ms = 100`, `multiplier = 2.0`, `max_delay_ms = 60000`,
  `max_attempts = 3`), no deadline, and a provider that answers one 429 with
  no `Retry-After` header and then succeeds
- **WHEN** the retry loop runs
- **THEN** the first retry sleeps 100 ms and the second would sleep 200 ms —
  the exponential sequence unchanged — and the call succeeds on the retry with
  no `RetryAfterCapped` error

#### Scenario: Retry-After sleep cannot extend past the total deadline

- **GIVEN** a materialized producer with `timeout_secs = 10`,
  `network_retry.max_delay_ms = 60000`, and a provider that answers 429 with
  `Retry-After: 86400` on every attempt (virtual time)
- **WHEN** the retry loop computes the delay with about 10 s of budget left
- **THEN** the sleep is capped at the remaining budget and, the budget spent,
  the call ends deterministically with `Timeout(10s)` — no further request is
  launched after a sleep that ends at the deadline

#### Scenario: Zero max_delay sleeps nothing and does not busy-spin

- **GIVEN** a materialized producer with `network_retry.max_delay_ms = 0` and
  `max_attempts = 0` (unlimited), no deadline, and a provider that answers 429
  with `Retry-After: 86400`
- **WHEN** the retry loop processes the first failure
- **THEN** the loop returns `RetryAfterCapped { requested: 86400s,
  capped_to: 0s }` immediately — no sleep, no second attempt, no spin

#### Scenario: Zero Retry-After under a zero cap keeps the raw error

- **GIVEN** a materialized producer with `network_retry.max_delay_ms = 0` and
  `max_attempts = 2`, no deadline, and a provider that answers 429 with
  `Retry-After: 0` on every attempt
- **WHEN** the retry loop exhausts the attempts (2 attempts, 1 zero-length
  sleep)
- **THEN** the surfaced error is the original `RateLimit { retry_after:
  Some(0s) }`, not `RetryAfterCapped`

#### Scenario: Embed path applies the same cap

- **GIVEN** an embed producer with `network_retry.max_delay_ms = 60000`, no
  deadline, and a wiremock embed endpoint answering 429 with
  `Retry-After: 86400` on every attempt (virtual time)
- **WHEN** the embed retry loop exhausts the attempts
- **THEN** the surfaced error is `RetryAfterCapped` naming `86400s` and the
  cap, not the raw `RateLimit`

#### Scenario: Decide path applies the same cap

- **GIVEN** a decision provider with `network_retry.max_delay_ms = 60000`,
  `timeout_secs = 300`, `max_attempts = 3`, and a wiremock decision endpoint
  answering 429 with `Retry-After: 86400` on every attempt (virtual time)
  (`decisions` feature)
- **WHEN** the decide retry loop exhausts the attempts (3 attempts, 2 capped
  sleeps of 60 s each, inside the 300 s budget)
- **THEN** the surfaced error is `RetryAfterCapped` naming `86400s` and `60s`,
  not the raw `RateLimit`
