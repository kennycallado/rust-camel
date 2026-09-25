# Tasks: redsweep (mission 270, LIGHT)

## 1. Remove redundant outer deadline in serve_connection (rc-9qjuu)

- [x] 1.1 In `crates/components/camel-redis/tests/common/mod.rs`, locate
  `serve_connection` (line ~191) wrapping the `wait_until_released` call
  in an outer 30 s `timeout(..).unwrap_or(false)`. Remove the outer wrap
  so the call is awaited directly; the inner per-wait 30 s deadline in
  `wait_until_released` (lintwiden D4.2) survives and governs. The `false`
  contract (close connection) is unchanged. Verify `Duration` imports
  still used. Run `cargo test -p camel-redis` (all green) and
  `cargo clippy -p camel-redis --all-targets -- -D warnings`.

## 2. Split rediserr design.md sentinel-client audit row (rc-e04an)

- [x] 2.1 In `openspec/changes/archive/2026-09-22-rediserr/design.md`
  line 70, replace the single `topology.rs sentinel client (×1)` row with
  two rows reflecting the code truth in `crates/components/camel-redis/src/topology.rs`:
  non-TLS `SentinelClient::build` → `ProcessorErrorWithSource` (preserve
  `RedisError` source, rules 4/5); TLS `SentinelClientBuilder::new` →
  `CamelError::Config` (rule 1 false). Match the table's existing phrasing
  style. Verify site counts against topology.rs (build at ~362, TLS
  builder at ~429, second Config site at ~495 — check its context and
  reflect it honestly in the row multiplicity).

## 3. Extract pubsub.rs test module to sibling file (rc-h7304)

- [x] 3.1 Move the inline `#[cfg(test)] mod tests` block
  (lines 307–1070 of `crates/components/camel-redis/src/pubsub.rs`) to
  sibling `src/pubsub_tests.rs` following the `topology_tests.rs` /
  `consumer_tests.rs` precedent: header doc comment, contents dedented
  one level, `use super::*;` preserved, and
  `#[cfg(test)] #[path = "pubsub_tests.rs"] mod tests;` appended at the
  end of `pubsub.rs`. Pure move, zero behavior. pubsub.rs lands under
  1k lines. Run `cargo test -p camel-redis`, `cargo fmt --check`,
  `cargo clippy -p camel-redis --all-targets -- -D warnings`,
  `cargo xtask lint-single-source`, and confirm lint-unbounded-wait
  ceiling stays 296.
