## ADDED Requirements

### Requirement: Producer request deadline bounds every post-connect request

The Producer role SHALL apply a configurable request deadline to every
post-connect request it issues to a remote MCP server (`tools/call`,
`resources/read`). The deadline SHALL be configured per remote as
`request_timeout_secs` (whole seconds, minimum 1) and SHALL default to 120
seconds when the option is absent. The startup budgets stay unchanged and
stay separate phase limits — 5 s for DNS resolution and 5 s for TCP connect
— and the DNS-resolution and TCP-connect errors SHALL name those 5 s
budgets. DNS pinning SHALL remain unchanged.

#### Scenario: stalled server trips the deadline when it expires

- **GIVEN** a remote configured with `request_timeout_secs = 1` that accepts
  the connection, answers `server/discover`, and never answers a
  `tools/call` request
- **WHEN** the producer dispatches `mcp:call?server=<remote>&tool=slow` and
  the 1 s deadline expires (the test allows a generous scheduler-tolerance
  watchdog rather than asserting an exact duration)
- **THEN** the dispatch fails with an error naming the operation, the tool,
  and the bound that fired:
  `tool call 'slow' exceeded the 1s request deadline`

#### Scenario: stalled resource read trips the deadline when it expires

- **GIVEN** a remote configured with `request_timeout_secs = 1` that accepts
  the connection, answers `server/discover`, and never answers a
  `resources/read` request
- **WHEN** the producer dispatches `mcp:read?server=<remote>&uri=<uri>` and
  the 1 s deadline expires (same watchdog tolerance)
- **THEN** the dispatch fails with an error naming the operation, the
  resource URI, and the bound that fired:
  `resource read '<uri>' exceeded the 1s request deadline`

#### Scenario: a fast call is unaffected by the deadline

- **GIVEN** a remote that answers `tools/call` promptly, configured with an
  explicit `request_timeout_secs`
- **WHEN** the producer dispatches `mcp:call` for a tool
- **THEN** the call succeeds and the reply content reaches the Exchange
  unchanged

#### Scenario: the default applies when the option is absent

- **GIVEN** a remote configuration that omits `request_timeout_secs`
- **WHEN** the remote config is parsed and a client connects to the remote
- **THEN** the parsed effective deadline is 120 seconds and the connected
  adapter client reports `request_deadline()` of 120 seconds — the default
  reaches the adapter, not just the config struct

#### Scenario: zero is rejected at parse time naming the option

- **GIVEN** a remote configuration declaring `request_timeout_secs = 0`
- **WHEN** the remote config is parsed
- **THEN** parsing fails with an error naming the option:
  `request_timeout_secs must be at least 1 (got 0)`

#### Scenario: an empty value is rejected at parse time naming the option

- **GIVEN** a remote configuration declaring `request_timeout_secs: ""`
- **WHEN** the remote config is parsed
- **THEN** parsing fails with an error naming the option:
  `request_timeout_secs must be an integer number of seconds of at least 1`

#### Scenario: DNS resolution timeout names its bound

- **GIVEN** a hostname-based remote whose DNS resolution stalls
- **WHEN** the 5 s DNS resolution budget elapses
- **THEN** the connect error names the host and the 5 s DNS budget:
  `timed out resolving remote host '<host>' after 5s`

#### Scenario: TCP connect timeout names its bound

- **GIVEN** a remote whose TCP connect stalls past the 5 s connect budget
- **WHEN** the producer start runs `RmcpClient::connect` and the connect
  timeout surfaces through rmcp's transport error
- **THEN** the connect error names the remote and the 5 s TCP connect
  limit; other transport errors pass through unchanged rather than being
  mislabeled timeouts
