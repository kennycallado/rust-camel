## ADDED Requirements

### Requirement: Consumer stop settle bound follows the configured drain budget

The system SHALL bound the `WsConsumer` stop settle wait by the configured
drain budget (`camel-config` `drain_timeout_ms`) when the stop is driven by a
graceful context shutdown, and SHALL preserve the existing 5 s bound for
reload and suspend stops. On settle expiry the consumer SHALL fail open
(warn + proceed with teardown) with the existing warn fields.

#### Scenario: Graceful stop honors a budget smaller than the old bound

- **GIVEN** a started `WsConsumer` with an in-flight exchange outstanding and
  a propagated drain budget of 500 ms
- **WHEN** `stop()` runs under a paused tokio clock and the budget expires
  before the exchange completes
- **THEN** the settle wait expires at ~the configured 500 ms bound (not 5 s),
  the consumer logs the fail-open warning and proceeds with teardown, and
  `stop()` returns successfully

#### Scenario: Graceful stop honors a budget larger than the old bound

- **GIVEN** a started `WsConsumer` with an in-flight exchange that completes
  6 s into the settle wait (simulated on a paused tokio clock) and a
  propagated drain budget of 10 s
- **WHEN** `stop()` runs
- **THEN** the settle wait does NOT expire at the old 5 s bound, the exchange
  completion is observed before expiry, and `stop()` returns successfully
  without the fail-open warning

#### Scenario: Reload stop keeps the default shutdown bound

- **GIVEN** a route stopped through the registry/reload stop path
- **WHEN** the `WsConsumer` settle wait runs
- **THEN** the settle bound is `DEFAULT_SHUTDOWN_TIMEOUT` (5 s), matching the
  pre-change behavior

#### Scenario: Suspend stop falls back when no budget was propagated

- **GIVEN** a consumer incarnation whose stop-budget slot was never written
- **WHEN** `stop()` runs
- **THEN** the settle bound is the documented fallback constant (5 s)

#### Scenario: Real-IO echo survives under a configured budget

- **GIVEN** a connected WebSocket client with an echo exchange in flight and
  a small configured drain budget propagated to the consumer
- **WHEN** stop is initiated while the echo exchange is still completing
  inside the budget
- **THEN** the echo response is delivered on the connection before teardown
  and the consumer stop completes cleanly
