## ADDED Requirements

### Requirement: Decide calls carry a default total deadline

The `llm:decide` endpoint SHALL bound every decide call with a total deadline.
When provider or global `timeout_secs` is configured, that value SHALL be the
total, unchanged (provider value wins over global, per
`effective_timeout_secs`). When neither is configured, the endpoint SHALL
apply `DEFAULT_DECIDE_TOTAL_TIMEOUT_SECS` (120 s) as the total. The default
SHALL be a crate constant next to `DEFAULT_ATTEMPT_TIMEOUT_SECS` and SHALL NOT
be a configuration field; an operator who wants a different total sets
`timeout_secs`. The deadline wraps the whole call — all attempts and all
backoff sleeps — exactly once (`apply_timeout`, ADR-0021); the adapters'
per-request HTTP timeout (30 s default) stays a transport bound, not a nested
timeout, and its value is unchanged. With the default in force, a decision
provider cannot hold a route task indefinitely, even with `max_attempts = 0`
(unlimited retries): the total is a configurable-latency budget, not a promise
of a fixed attempt count.

#### Scenario: No timeout_secs applies the default total

- **GIVEN** a decision provider and global `timeout_secs` both unset, and a
  custom `DecisionProvider` whose `decide` future never resolves (no HTTP
  attempt, so the adapters' 30 s per-request bound cannot fire)
- **WHEN** a decide call runs under a paused tokio clock and 120 s of virtual
  time elapse
- **THEN** the call ends with `LlmError::Timeout(120s)`, not an unbounded wait

#### Scenario: Provider timeout_secs wins over the default

- **GIVEN** a decision provider configured with `timeout_secs = 5` and global
  `timeout_secs` unset, and a `DecisionProvider` whose `decide` future never
  resolves
- **WHEN** a decide call runs under a paused tokio clock and 5 s of virtual
  time elapse
- **THEN** the call ends with `LlmError::Timeout(5s)` — the configured value,
  not the 120 s default

#### Scenario: Global timeout_secs is the total when the provider omits it

- **GIVEN** global `timeout_secs = 30`, a decision provider without its own
  `timeout_secs`, and a `DecisionProvider` whose `decide` future never
  resolves
- **WHEN** a decide call runs under a paused tokio clock and 30 s of virtual
  time elapse
- **THEN** the call ends with `LlmError::Timeout(30s)` — the global value, not
  the 120 s default

#### Scenario: Unlimited retries are bounded by the default total

- **GIVEN** a decision provider and global `timeout_secs` both unset,
  `network_retry` with `max_attempts = 0` (unlimited), and a `DecisionProvider`
  that answers `RateLimit { retry_after: None }` on every attempt
- **WHEN** a decide call runs under a paused tokio clock past 120 s of virtual
  time
- **THEN** the call ends with `LlmError::Timeout(120s)` after a finite number
  of attempts — the unlimited-retry configuration cannot retry forever
