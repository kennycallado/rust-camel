# Design: jmsmutants

## Approach

Extend the existing `#[cfg(test)]` module in `crates/components/camel-jms/src/config.rs` with three focused test groups, one per delivery task. First pin every documented reconnect default with assertions, recording `multiplier` and `max_delay` as equivalent to the same values supplied by `NetworkRetryPolicy::default()`. Next exercise `JmsEndpointConfig::from_uri` through wrong schemes, empty destination names with the destination-format error, ambiguous `jms:` shorthand, and priority 9/10 boundaries. Finally assert the concrete bridge cache path returned by the existing `camel_bridge` helper. Tests must be written first and demonstrate mutant kills or explicit equivalent-mutant adjudication; no production code is changed.

Run the crate unit tests, formatting, clippy, and the scoped mutation command when available. The mandatory workspace no-run compile check runs before parking. JMS tests use pure configuration seams and require no broker.

## Affected crates

- `camel-component-jms`: add unit coverage only in `src/config.rs`.

## Architecture boundaries

This change stays inside the Components context and tests operator-owned JMS endpoint configuration. It does not alter Runtime, DSL, Services, Languages, or Functions. It preserves the existing endpoint parsing and bridge download boundary; tests observe those contracts without starting a consumer or producer.

## Alternatives considered

- Live broker tests were rejected because all target functions are pure configuration construction or validation.
- Mutation-run-only verification was rejected because durable regression tests are required.
- Production refactoring was rejected because survivors represent missing assertions, not implementation defects.
