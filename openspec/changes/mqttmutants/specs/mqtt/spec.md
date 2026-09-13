## ADDED Requirements

### Requirement: MQTT configuration mutation coverage

The MQTT configuration unit suite SHALL verify the behavior represented by bd issues rc-ldvm, rc-xv4e, rc-blcy, and rc-e83t without requiring a live broker.

#### Scenario: QoS variants map to their rumqttc values

- **GIVEN** each supported `QosLevel` variant
- **WHEN** its `to_rumqttc` conversion is evaluated
- **THEN** `AtMostOnce`, `AtLeastOnce`, and `ExactlyOnce` map to the corresponding `rumqttc::QoS` variants

#### Scenario: Endpoint reconnect policy selects the configured override

- **GIVEN** an endpoint with a non-default reconnect policy and a different fallback policy
- **WHEN** `effective_reconnect` is evaluated
- **THEN** the endpoint policy is returned

#### Scenario: Endpoint reconnect policy selects the fallback

- **GIVEN** an endpoint without a reconnect override and a non-default fallback policy
- **WHEN** `effective_reconnect` is evaluated
- **THEN** the fallback policy is returned

#### Scenario: Keep-alive accepts the maximum representable MQTT interval

- **GIVEN** an endpoint keep-alive of `u16::MAX` seconds
- **WHEN** endpoint validation runs
- **THEN** validation succeeds

#### Scenario: Keep-alive rejects values above the maximum representable interval

- **GIVEN** an endpoint keep-alive of `u16::MAX + 1` seconds
- **WHEN** endpoint validation runs
- **THEN** validation fails

#### Scenario: Broker validation accepts MQTT schemes

- **GIVEN** a broker URL beginning with `mqtt://` or `mqtts://`
- **WHEN** broker validation runs
- **THEN** validation succeeds

#### Scenario: Broker validation rejects non-MQTT schemes

- **GIVEN** a broker URL beginning with a non-MQTT scheme
- **WHEN** broker validation runs
- **THEN** validation fails
