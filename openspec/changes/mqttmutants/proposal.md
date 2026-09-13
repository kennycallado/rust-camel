# Proposal: mqttmutants

## Why

The `camel-component-mqtt` configuration module has four surviving mutation-test defects recorded in bd issues rc-ldvm, rc-xv4e, rc-blcy, and rc-e83t. The current unit suite does not prove QoS conversion, reconnect fallback selection, the keep-alive upper boundary, or both broker URL validation polarities. These gaps can allow configuration regressions without a failing test.

## What Changes

Add focused unit tests in `crates/components/camel-mqtt/src/config.rs` that kill the four named mutants. The tests cover every `QosLevel` mapping, both `effective_reconnect` branches with non-default values, the exact keep-alive boundary, and valid/invalid broker URL schemes. No production behavior, public API, broker setup, or integration infrastructure changes.

## Acceptance criteria

- Every `QosLevel` variant maps to its expected `rumqttc::QoS` value, killing rc-ldvm.
- Endpoint reconnect overrides and fallback policy are both asserted with distinguishable values, killing rc-xv4e.
- `u16::MAX` keep-alive is accepted and the next value is rejected, killing rc-blcy.
- Valid MQTT/MQTTS URLs pass and a non-MQTT scheme fails, killing rc-e83t.
- Tests pass without a live broker.

## Risk budget

The change is test-only and limited to the MQTT configuration unit-test module. No risk from runtime behavior changes is accepted; live-broker or cross-component changes are out of scope.
