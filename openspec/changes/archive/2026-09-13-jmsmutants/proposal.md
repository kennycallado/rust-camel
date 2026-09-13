# Proposal: jmsmutants

## Why

Mutation testing identified eight unexplained survivors in the JMS configuration module. The unit suite does not pin reconnect defaults, reject ambiguous or invalid endpoint URIs at their boundaries, or prove the concrete bridge cache directory default. These gaps allow configuration regressions to pass without a failing test. The work is tracked by bd issues rc-j5zp, rc-6f62, rc-jtsw, and rc-isxj0.

## What Changes

Add focused unit tests in `crates/components/camel-jms/src/config.rs` for the three claimed behaviors and the bridge-cache survivor folded into this mission. Tests will kill the distinguishable reconnect-field deletion, the three `from_uri` mutants, and the `default_bridge_cache_dir` replacement. The `multiplier` and `max_delay` deletions are behaviorally equivalent to the current `NetworkRetryPolicy::default()` values and will be individually adjudicated with source evidence. No production behavior, public API, broker setup, or integration infrastructure changes.

## Acceptance criteria

- `jms_reconnect_default` asserts `max_attempts`, `initial_delay`, `jitter_factor`, `max_delay`, and `multiplier`; the distinguishable field-deletion mutants die and the latter two are adjudicated as equivalent to `NetworkRetryPolicy::default()`.
- `JmsEndpointConfig::from_uri` tests reject empty `queue:`/`topic:` names and bare `jms:` with the destination-format error, wrong schemes, ambiguous non-empty `jms:` shorthand, and priority 9/10 boundaries, killing all three survivors.
- `default_bridge_cache_dir` asserts the concrete delegated default path; its `Default::default()` mutant dies.
- The eight survivors covered by rc-j5zp, rc-6f62, rc-jtsw, and rc-isxj0 are killed or individually adjudicated with written rationale.
- Tests remain unit-level and pass without a live broker.

## Risk budget

The change is test-only and limited to the JMS configuration unit-test module. No runtime behavior changes, production refactors, broker setup, or unrelated component changes are in scope.
