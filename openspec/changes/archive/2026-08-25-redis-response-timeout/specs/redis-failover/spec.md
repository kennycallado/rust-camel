## ADDED Requirements

### Requirement: Multiplexed connection build accepts a driver response timeout

The system SHALL let `MultiplexedExecutor` (component `camel-redis`) carry an
optional driver-level response timeout, set through a builder-style
`with_response_timeout(Duration)` that does not alter the `new(...)`
signature. When no value is set, the connection build SHALL remain the
config-less `get_multiplexed_async_connection()` call, preserving the redis
driver's default per-command deadline (500 ms in redis 1.6.0) and all
existing behavior. When a value is set, every connection built through
`get_conn` — the initial connect and every rebuild performed by `refresh()`
and `reconnect` — SHALL be constructed through
`get_multiplexed_async_connection_with_config` with
`AsyncConnectionConfig::set_response_timeout` set to that value, so the
configured deadline governs each command pipelined over the connection
instead of the driver default. The setting SHALL NOT alter the connect
timeout: `connection_timeout_secs` continues to bound only the TCP connect
phase.

#### Scenario: unset response timeout keeps the driver default path

- **GIVEN** a `MultiplexedExecutor` built with `new(...)` and no
  `with_response_timeout` call
- **WHEN** `get_conn` builds the connection
- **THEN** the build uses the config-less call and the driver's 500 ms
  default response deadline continues to govern commands (behavior
  identical to before this change; existing executor tests pass unchanged)

#### Scenario: configured large timeout outlives the driver default

- **GIVEN** a `MultiplexedExecutor` with `with_response_timeout(10 s)`
  against a silent peer whose TCP connect succeeds but never replies, under
  tokio's paused clock
- **WHEN** the command future is polled and virtual time advances past the
  500 ms driver default (e.g. to 1 s virtual)
- **THEN** the command is still pending at that virtual-time boundary — the
  configured value, not the driver default, sets the deadline

#### Scenario: configured small timeout fires before the driver default

- **GIVEN** a `MultiplexedExecutor` with `with_response_timeout(100 ms)`
  against a silent peer, under tokio's paused clock
- **WHEN** the command future is polled and virtual time advances to the
  100 ms boundary
- **THEN** the command has failed by the configured deadline — before the
  500 ms driver default fires

#### Scenario: refresh rebuild carries the configured deadline

- **GIVEN** a `MultiplexedExecutor` with a configured response timeout
  holding a connection to a silent peer, under tokio's paused clock
- **WHEN** `refresh()` drops the cached connection and rebuilds, and a
  command probe is polled with virtual time advanced to the configured
  boundary
- **THEN** the probe fails by the configured deadline, not the 500 ms
  default — the rebuilt connection carries the same configured response
  timeout

#### Scenario: configured response timeout does not alter the connect timeout

- **GIVEN** a `MultiplexedExecutor` with `with_response_timeout(100 ms)`
  and `connection_timeout_secs = 1` against a peer that accepts the TCP
  connection but never completes the RESP handshake, under tokio's paused
  clock
- **WHEN** `get_conn` attempts to build the connection and virtual time is
  advanced (polling the future before each advance) to the 1 s connect
  boundary
- **THEN** the failure is the connect timeout ("Redis connection … timed
  out after 1s") — the response-timeout setting bounds only command
  round-trips after the connection is established

### Requirement: Repository service sets a driver response timeout above its local backstop

The repository service crate (`camel-redis-repo`) SHALL construct its
`MultiplexedExecutor` with a driver response timeout strictly greater than
its crate-local per-command backstop (`DEFAULT_RESPONSE_TIMEOUT` = 30 s,
ADR-0063 Decision 13) — the implementation uses a fixed 5 s margin (35 s);
the binding
contract is the strict ordering, observable through behavior, not the exact
figure. The local backstop SHALL therefore always fire first: the error
message and transient-Io classification asserted by the existing backstop
tests SHALL remain exactly the tested ones, and the driver deadline SHALL
act only as defense-in-depth for any path that bypasses the local backstop.
The repository service crate SHALL NOT disable the driver deadline (`None`)
on its connections.

#### Scenario: local backstop governs classification over the driver deadline

- **GIVEN** a repository executor built through the production
  `connect_executor` path against a silent peer, with a short injectable
  local backstop, under tokio's paused clock where both deadlines are
  deterministic
- **WHEN** a command round-trip exceeds the local backstop
- **THEN** the failure is the local backstop's error ("redis command
  response timed out after …") classifying as transient Io — the driver's
  deadline sits above the backstop and does not fire first

#### Scenario: driver deadline sits strictly above the backstop

- **GIVEN** `connect_executor_with_topology` building the executor with the
  driver response timeout configured from the fixed margin
- **WHEN** a command runs against a silent peer with the local backstop
  injected below the driver deadline, under tokio's paused clock
- **THEN** the local backstop error wins — proving the driver deadline on
  the constructed connection is strictly greater than the backstop (the
  exact margin value is an implementation constant, not a contract)
