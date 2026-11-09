## MODIFIED Requirements

### Requirement: HTTP failures map to the LlmError taxonomy

`map_status`, `is_insufficient_quota`, `parse_retry_after`, and `truncate_body` SHALL
move unchanged to `provider/http_errors.rs`, which does not import genai. The genai
adapter SHALL use them from there, and its mapping SHALL behave as before. The
decision adapters SHALL map 413 and 415 to `InvalidRequest`, then call `map_status`
for every other status:

| Status | Result | Retryable |
|---|---|---|
| 401/403 | `AuthFailed` | no |
| 404 | `ModelNotFound` | no |
| 400/422 | `InvalidRequest` | no |
| 429 with an `insufficient_quota` code or type | `QuotaExceeded` | no |
| other 429 | `RateLimit { retry_after }` (delta-seconds `Retry-After`) | yes |
| 5xx, including 529 | `ProviderUnavailable` | yes |
| reqwest timeout inside the per-request total | `Timeout(configured)` | yes |
| connect timeout | `TransportTimeout { phase: Connect, after: connect bound }` | yes |

The decision path SHALL pass the whole capped error body to `map_status`, so the
`insufficient_quota` check sees valid JSON. Error text SHALL hold at most 200 body
bytes; `map_status` and `truncate_body` enforce this. Error text and `Debug`
output (config and adapter) SHALL NOT contain the API key. The HTTP status scenarios require the `decisions`
feature.

The decision adapters SHALL build their client with the decision transport
policy (`connect 10s`, no read-idle guard): no redirects, `no_proxy`, and
`resolve_to_addrs`. A per-request `.timeout(configured)` SHALL be set. The
decision client SHALL NOT carry a client-wide `read_timeout` (reqwest 0.13.5
bounds time-to-headers with it and offers no per-request override, which would
cap cold starts below the configured total).

#### Scenario: Throttled and retried with Retry-After

- **GIVEN** a `typesafe` provider on wiremock with `network_retry` enabled
  (`max_attempts = 3`). The first response is a synthetic 429 with `Retry-After: 1`;
  TypeSafe documents no Retry-After header. The next response is a valid 200.
- **WHEN** the producer runs in real time
- **THEN** it succeeds, wiremock got 2 requests, and the elapsed time is at least
  1 second

#### Scenario: Quota exhaustion fails fast

- **GIVEN** an `openai` provider with retry enabled, and a 429 response with body
  `{"error":{"code":"insufficient_quota"}}`
- **WHEN** `decide` runs through the producer
- **THEN** the result is an error whose text contains `quota exceeded`, and wiremock
  got 1 request

#### Scenario: Status table

- **GIVEN** a wiremock that returns each status in the table (`529` included)
- **WHEN** an adapter `decide` call runs once per status
- **THEN** each result is the `LlmError` variant that the table names

#### Scenario: Auth failure is redacted

- **GIVEN** an `openai` provider with `api_key = "sk-secret-123"`, and a 401 response
- **WHEN** `decide` runs
- **THEN** the result is `AuthFailed`, and `sk-secret-123` is in none of these: the
  error text, `format!("{:?}")` of the config, or `format!("{:?}")` of the adapter

#### Scenario: Redirect is not followed

- **GIVEN** a `typesafe` provider on wiremock (`allow_internal = true`) that returns
  302 with `Location` pointing to a second wiremock server
- **WHEN** `decide` runs
- **THEN** the result is an error, and the second server got 0 requests

#### Scenario: Timeout

The per-attempt and total timeouts are both 1 second here, so this scenario proves
that a timeout fires. It does not tell the two timeouts apart.

- **GIVEN** an endpoint with `timeout_secs = 1`, no `network_retry`, and a wiremock
  response delayed 5 seconds
- **WHEN** the producer runs
- **THEN** the result is `CamelError::ProcessorError` whose text contains `timeout`,
  and it returns in less than 4 seconds

#### Scenario: Connect timeout names the connect layer

- **GIVEN** a decision endpoint whose TCP connection cannot be established within
  the connect bound
- **WHEN** `decide` runs
- **THEN** the error is `TransportTimeout { phase: Connect, .. }` (retryable) whose
  display contains `timeout` and the connect bound

#### Scenario: Cancellation releases the permit

- **GIVEN** an endpoint with `max_concurrency = 1`. Wiremock delays the first response
  10 seconds (up to 1 time), and the next response is a fast 200.
- **WHEN** the first producer call is dropped by a 200 ms `tokio::time::timeout`, and
  then a second call runs under a 3-second `tokio::time::timeout`
- **THEN** the second call returns `Ok`
