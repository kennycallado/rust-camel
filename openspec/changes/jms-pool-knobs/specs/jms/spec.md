## ADDED Requirements

### Requirement: JMS producer pool idle knobs are configurable, validated, and wired to the bridge

The JMS producer pool SHALL expose `pool_idle_timeout_ms` and
`pool_expiration_check_ms` on `JmsPoolConfig` with a default of 30000 each,
SHALL reject values below 1000 or above 86400000 at config validation with an
actionable message naming the field and the accepted range, and SHALL pass the
configured values to the bridge process as `JMS_POOL_IDLE_TIMEOUT_MS` and
`JMS_POOL_EXPIRATION_CHECK_MS` environment variables. The Java bridge SHALL
apply the received values to the producer pool before start on both broker
branches, and the JMS documentation SHALL state the defaults, the knob
semantics, and the trade-offs.

#### Scenario: Defaults reach the bridge spawn

- **GIVEN** a `JmsPoolConfig` with no explicit pool knobs
- **WHEN** the component builds the bridge process config
- **THEN** the spawn environment contains `JMS_POOL_IDLE_TIMEOUT_MS=30000` and
  `JMS_POOL_EXPIRATION_CHECK_MS=30000`

#### Scenario: Configured values reach the bridge spawn verbatim

- **GIVEN** a `JmsPoolConfig` with `pool_idle_timeout_ms` 60000 and
  `pool_expiration_check_ms` 5000
- **WHEN** the component builds the bridge process config
- **THEN** the spawn environment contains `JMS_POOL_IDLE_TIMEOUT_MS=60000` and
  `JMS_POOL_EXPIRATION_CHECK_MS=5000`

#### Scenario: Rust validation bounds are enforced per field

- **GIVEN** a `JmsPoolConfig` where `pool_idle_timeout_ms` or
  `pool_expiration_check_ms` is set to 0, 999, or 86400001
- **WHEN** `validate()` runs
- **THEN** it rejects each out-of-range value with a config error naming the
  field and the accepted range, and it accepts both inclusive endpoints 1000
  and 86400000 for each field

#### Scenario: Java bridge applies the values to both pool branches

- **GIVEN** bridge config values `poolIdleTimeoutMs` 60000 and
  `poolExpirationCheckMs` 5000
- **WHEN** `JmsClientFactory.createFactory()` builds the pool for the activemq
  branch and for the artemis branch
- **THEN** the activemq pool reports idleTimeout 60000 and expiration-check
  interval 5000, and the artemis pool reports connection-idle-timeout 60000 and
  connection-check interval 5000

#### Scenario: Java bridge bounds mirror the Rust bounds

- **GIVEN** either pool environment variable is non-numeric or outside
  [1000, 86400000]
- **WHEN** `BridgeConfig` parses it
- **THEN** it rejects the value with an error naming the variable and the
  accepted range; an unset or blank variable selects the documented default
  30000, and both inclusive endpoints are accepted

#### Scenario: Documentation states defaults and trade-offs

- **GIVEN** the JMS component guide
- **WHEN** a reader looks up the producer pool knobs
- **THEN** the guide states the 30000/30000 defaults, what each knob controls,
  the reconnect-churn versus idle-connection trade-off, the camel-112
  measurements, and that values apply when the bridge process starts; the guide
  also states that the pinned library sweep default was -1 (disabled), that the
  explicit 30000 ms default deliberately enables periodic eviction, and that
  applying these knobs requires the updated Java bridge while older binaries
  ignore them
