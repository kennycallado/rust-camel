## ADDED Requirements

### Requirement: Blocking pops are never bounded by a shorter driver response timeout

Queue-mode (BLPOP/BRPOP) consumer connections and producer connections of an
endpoint whose command is BLPOP/BRPOP SHALL be built with no driver-level
response deadline (`AsyncConnectionConfig::set_response_timeout(None)`): the
pop timeout, the transient-error budget, and route cancellation are the only
bounds on a blocking pop. A BLPOP/BRPOP SHALL NOT travel over a connection
whose response timeout is shorter than the requested pop timeout. Polling
fallbacks remain forbidden (REDIS-003). The component-level
`connection_timeout_secs` SHALL stay the sole connect bound
(`set_connection_timeout(None)`), matching the producer executor precedent.

#### Scenario: pop timeout longer than the old driver default delivers once

- **GIVEN** a fake RESP server that answers `BLPOP key 1` after 1 second with
  one item, and a queue consumer with `timeout=1`
- **WHEN** the queue session runs until cancellation
- **THEN** exactly one item is delivered, the connection is established once
  (no reconnect and no retry warning), and cancellation ends the session
  cleanly

#### Scenario: BRPOP with a 1 second pop delivers once

- **GIVEN** a fake RESP server that answers `BRPOP key 1` after 1 second
- **WHEN** the queue session runs with `QueuePopCommand::Brpop`
- **THEN** exactly one item is delivered with a single connection and no
  transient retry

#### Scenario: timeout=0 blocks without a client-side timeout

- **GIVEN** a fake RESP server that withholds the `BLPOP key 0` reply longer
  than the old 500 ms driver default
- **WHEN** the queue session runs
- **THEN** no client-side timeout error is raised, a later reply is delivered,
  and cancellation while still blocked ends the session cleanly

#### Scenario: producer BLPOP succeeds past the old driver default

- **GIVEN** a producer endpoint with command BLPOP and a fake RESP server that
  answers after 800 ms with a pop result
- **WHEN** the producer executes BLPOP with `CamelRedis.Timeout=1`
- **THEN** the command succeeds (it is not cut off at 500 ms)

#### Scenario: blocking endpoint ignores the response-timeout test seam

- **GIVEN** a `MultiplexedExecutor` built with `with_response_timeout(100 ms)`
  for an endpoint whose command is BLPOP
- **WHEN** the effective driver response deadline is computed
- **THEN** it is `None`; for an endpoint with a non-blocking command it stays
  `Some(100 ms)`
