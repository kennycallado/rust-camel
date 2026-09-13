# Design: mqttmutants

## Approach

Extend the existing `#[cfg(test)]` module in `crates/components/camel-mqtt/src/config.rs` with four focused tests. Follow the current configuration-construction helpers and assertion style. Each test is designed around the exact surviving mutant rather than merely exercising a happy path: use non-default QoS variants, a non-default reconnect policy, `u16::MAX`, and an invalid URL scheme. Run the package unit tests and standard formatting/lint checks. No broker is needed because all target functions are pure configuration transformations or validators.

## Affected crates

- `camel-component-mqtt`: add unit coverage only in `src/config.rs`.

## Architecture boundaries

This change stays inside the Components context and tests operator-owned MQTT configuration. It does not alter Runtime, DSL, Services, Languages, or Functions. It respects ADR-0027 because it verifies the MQTT 3.1.1 configuration seam without opening a connection or changing endpoint lifecycle behavior.

## Alternatives considered

- Live broker tests were rejected because the surviving functions are pure and existing unit seams provide direct coverage.
- Mutation-run-only verification was rejected because durable regression tests are required and faster to run.
- Production refactoring was rejected because the defects are coverage gaps, not implementation defects.
