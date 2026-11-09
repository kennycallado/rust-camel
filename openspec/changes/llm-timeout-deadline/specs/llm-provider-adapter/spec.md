## ADDED Requirements

### Requirement: Transport client policy separates chat from decision

The component SHALL build its hardened reqwest clients without a client-wide
total timeout, from a per-caller `LlmHttpPolicy` of `connect` (10 s) and
`read_idle: Option<Duration>`. The chat/embed policy SHALL carry a 600 s
transport read-idle guard (`read_idle = Some(600s)`); the decision policy
SHALL carry none (`read_idle = None`) because reqwest 0.13.5 `read_timeout` is
client-wide, also bounds time-to-headers, and has no per-request override.
The public `hardened_http_client_with_pinning` / `hardened_http_client`
signatures SHALL be unchanged and delegate with the chat policy. DNS pinning,
`redirect(Policy::none())`, `no_proxy()`, `connect_timeout(10s)`, and the lazy
`OnceCell` build-once pattern SHALL be preserved. No value in the policy SHALL
be derived from `timeout_secs`. The chat path SHALL have no hidden client-wide
total timeout. The bounds in force are: the connect bound (10 s), the
read-idle guard (inactivity between consecutive reads or until headers
arrive), and the configured deadline — the materialized-mode total
(`apply_timeout`) or, in streaming mode, the per-chunk activity timeout.
Which guard fires first depends on the configuration; no guard is required to
fire after the configured deadline.

#### Scenario: Read-idle guard fires before a longer configured deadline

- **GIVEN** a chat endpoint that sends headers and then stalls, with
  `timeout_secs = 900`
- **WHEN** 600 s pass with no response activity
- **THEN** the call ends with `TransportTimeout { phase: Idle, after: 600s }` —
  the read-idle guard fires before the configured deadline

#### Scenario: Body read longer than 30 s completes under a 90 s deadline

- **GIVEN** the real hardened chat-policy client against a staged response
  server that sends headers immediately and delays the first body chunk by a
  simulated 45 s (paused clock), with `timeout_secs = 90`
- **WHEN** a chat completion runs
- **THEN** it completes successfully — no transport wall fires at 30 s

#### Scenario: Decision client has no read-idle guard

- **GIVEN** `LlmHttpPolicy::decision()`
- **WHEN** the decision client is built
- **THEN** it carries no client-wide `read_timeout`, and each decision request
  keeps its own per-request `.timeout(configured)`

#### Scenario: Hung chat connection is still bounded

- **GIVEN** a chat endpoint that sends headers and then never sends another
  byte, with `timeout_secs` unset
- **WHEN** a chat completion runs
- **THEN** the read-idle guard ends the call after 600 s of no response
  activity (not silently forever)

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
