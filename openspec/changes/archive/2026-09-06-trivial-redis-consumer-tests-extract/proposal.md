# trivial-redis-consumer-tests-extract

Extract the inline test module from `crates/components/camel-redis/src/consumer.rs` into a sibling file (rc-ejt0).

## Why

Trivial change — no spec breakdown needed.

## What changes

- `crates/components/camel-redis/src/consumer_tests.rs` (new): the moved `#[cfg(test)]` test module (493 lines), `use super::*` preserved.
- `crates/components/camel-redis/src/consumer.rs`: `#[cfg(test)] #[path = "consumer_tests.rs"] mod tests;` declaration replaces the inline module; file drops 1007 → 510 lines (ADR-0045 1k ceiling). Pure mechanical move, no logic changes; sibling-file convention per `camel-ws/client_consumer.rs`.
