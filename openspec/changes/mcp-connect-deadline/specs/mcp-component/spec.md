## ADDED Requirements

### Requirement: Producer connect deadline bounds the discover handshake

The Producer role SHALL bound `RmcpClient::connect` with a configurable
connect deadline covering the discover lifecycle future (the TCP connect
rmcp's transport performs and the `server/discover` POST), started after
DNS pinning completes. The deadline SHALL be configured per remote as
`connect_timeout_secs` (whole seconds, minimum 1) and SHALL default to 15
seconds when the option is absent. On expiry `connect` SHALL fail with an
error naming the server, the discover handshake, and the bound; route
start SHALL NOT hang past it. The deadline SHALL be scoped to connect: it
SHALL NOT bound post-connect requests, which remain governed exclusively
by `request_timeout_secs`. The DNS-resolution and TCP-connect phase limits
stay unchanged at 5 s each and stay separate phase limits. DNS resolution
precedes the wrapper (its own unchanged 5 s budget, not covered by this
deadline). TCP connect is inner to the wrapper: its 5 s error surfaces
first only when the outer bound leaves more than 5 s of budget when the
connect starts (as the default 15 s does); an outer bound at or below 5 s
MAY fire first, producing the named connect error — the configured
contract, not a regression. The post-connect `request_timeout_secs`
option SHALL remain a separate option with unchanged behaviour.

#### Scenario: a stalled discover POST trips the connect deadline with the named error

- **GIVEN** a remote configured with `connect_timeout_secs = 1` that
  accepts the TCP connection and never answers the `server/discover` POST
- **WHEN** the producer runs `RmcpClient::connect` and the 1 s deadline
  expires (the test allows a generous scheduler-tolerance watchdog rather
  than asserting an exact duration)
- **THEN** connect fails with an error naming the server, the phase, and
  the bound that fired:
  `connect to remote '<server>' did not complete the discover handshake within 1s`
  — and no inner-budget text is present (the mock accepts promptly, so
  neither the 5 s DNS nor the 5 s TCP budget fired)

#### Scenario: a healthy connect is unaffected by the deadline

- **GIVEN** a remote that accepts the connection and answers
  `server/discover` promptly, configured with an explicit
  `connect_timeout_secs`
- **WHEN** the producer runs `RmcpClient::connect` and then dispatches a
  `tools/call`
- **THEN** connect succeeds and the call round-trips unchanged — the
  connect-deadline wrapper does not disturb a healthy handshake

#### Scenario: the connect bound does not become a global request timeout

- **GIVEN** a remote configured with `connect_timeout_secs = 1` and
  `request_timeout_secs = 30` whose `server/discover` answers promptly
  but whose `tools/call` answer is delayed past the connect bound
- **WHEN** the producer connects and dispatches the `tools/call`
- **THEN** connect succeeds and the call succeeds — a request that
  outlasts `connect_timeout_secs` is governed by `request_timeout_secs`
  alone, proving the connect deadline is not leaked into the HTTP client
  as a total request timeout

#### Scenario: the default applies when unconfigured

- **GIVEN** a remote configuration that omits `connect_timeout_secs`
- **WHEN** the remote config is parsed
- **THEN** the parsed effective value is 15 — the default reaches the
  connect path (the same code path the explicit value uses)

#### Scenario: zero is rejected at parse time naming the option

- **GIVEN** a remote configuration declaring `connect_timeout_secs = 0`
- **WHEN** the remote config is parsed
- **THEN** parsing fails with an error naming the option:
  `connect_timeout_secs must be at least 1 (got 0)`

#### Scenario: an empty value is rejected at parse time naming the option

- **GIVEN** a remote configuration declaring `connect_timeout_secs: ""`
- **WHEN** the remote config is parsed
- **THEN** parsing fails with an error naming the option:
  `connect_timeout_secs must be an integer number of seconds of at least 1`

#### Scenario: existing 5 s budgets and post-connect deadline are unchanged

- **GIVEN** the component after this change, with the default
  `connect_timeout_secs = 15` (more than 5 s of outer budget remaining
  when a TCP connect starts)
- **WHEN** a DNS resolution or a TCP connect stalls past 5 s, or a
  post-connect `tools/call` stalls past `request_timeout_secs`
- **THEN** the existing errors fire unchanged:
  `timed out resolving remote host '<host>' after 5s`,
  `connect to remote '<server>' exceeded the 5s TCP connect limit`,
  `tool call '<tool>' exceeded the <n>s request deadline`
