## ADDED Requirements

### Requirement: JMS reconnect defaults remain explicit

The JMS configuration unit suite SHALL pin every field overridden by `jms_reconnect_default`.

#### Scenario: Reconnect policy contains documented defaults

- **GIVEN** the component reconnect default
- **WHEN** `jms_reconnect_default()` is evaluated
- **THEN** `max_attempts` is 0, `initial_delay` is 5 seconds, `multiplier` is 2.0, `max_delay` is 30 seconds, and `jitter_factor` is 0.0; `multiplier` and `max_delay` are also recorded as equivalent to their `NetworkRetryPolicy::default()` values

### Requirement: JMS endpoint URI validation rejects ambiguous input

The JMS configuration unit suite SHALL verify endpoint URI scheme, destination-name, shorthand, and boundary validation.

#### Scenario: Unsupported scheme is rejected

- **GIVEN** a URI without the `jms`, `activemq`, or `artemis` scheme
- **WHEN** `JmsEndpointConfig::from_uri` parses it
- **THEN** parsing fails with the expected scheme error

#### Scenario: Empty destination name is rejected

- **GIVEN** `jms:queue:`, `jms:topic:`, or bare `jms:` with no destination name
- **WHEN** `JmsEndpointConfig::from_uri` parses it
- **THEN** parsing fails with the destination-format error rather than the `jms:` shorthand ambiguity error

#### Scenario: JMS shorthand is rejected as ambiguous

- **GIVEN** a non-empty `jms:<name>` URI
- **WHEN** `JmsEndpointConfig::from_uri` parses it
- **THEN** parsing fails with guidance to use an explicit queue or topic prefix

#### Scenario: Validation boundary has the documented polarity

- **GIVEN** a priority query value of 9 or 10
- **WHEN** `JmsEndpointConfig::from_uri` parses them
- **THEN** priority 9 is accepted and priority 10 is rejected

### Requirement: JMS bridge cache default remains concrete

The JMS configuration unit suite SHALL verify that `default_bridge_cache_dir` delegates to the concrete bridge download cache default.

#### Scenario: Bridge cache default is not an empty path

- **GIVEN** the process environment used by the bridge cache helper
- **WHEN** `default_bridge_cache_dir()` is evaluated
- **THEN** it equals the concrete helper result and differs from `PathBuf::default()`
