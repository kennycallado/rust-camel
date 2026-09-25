# Proposal: redsweep — redis zone mechanical sweep (mission 270)

## Why

Sweep epic rc-62mdr (e_opus backlog ruling 2026-09-25) collects mechanical
P3 leftovers in the redis components zone from prior missions' review
findings. Three are ready and touch the same crate family; one lease, one
worktree, one pass (LIGHT protocol, order: `.opencode/fleet/orders/270-redsweep-mission.md`).

## What Changes

1. **rc-9qjuu (code)** — `crates/components/camel-redis/tests/common/mod.rs`:
   `serve_connection` wraps `wait_until_released` in an outer 30 s
   `timeout(..).unwrap_or(false)` while the fn already has the inner
   per-wait 30 s deadline (lintwiden D4.2). The inner deadline governs;
   remove the redundant outer wrap. The bounded-wait guarantee must not
   weaken (lint-unbounded-wait ratchet ceiling 296 must not move up).
2. **rc-e04an (docs)** — `openspec/changes/archive/2026-09-22-rediserr/design.md`
   audit-table row 70 collapses two distinct sentinel-client construction
   treatments: non-TLS `SentinelClient::build` (`ProcessorErrorWithSource`,
   rules 4/5 on the inner `RedisError`) vs TLS `SentinelClientBuilder::new`
   (`CamelError::Config`, rule 1 false). Split into two rows, one per
   concern. Both are one-shot setup paths off the reconnect classification
   path (no verdict hole).
3. **rc-h7304 (hygiene)** — `crates/components/camel-redis/src/pubsub.rs`
   crossed 1k lines (1070). Extract the inline `#[cfg(test)] mod tests`
   (lines 307–1070) to sibling `src/pubsub_tests.rs` via the house
   `#[cfg(test)] #[path = "pubsub_tests.rs"] mod tests;` pattern
   (precedent: `topology_tests.rs`, `consumer_tests.rs`). Pure move, zero
   behavior change.

## Impact

- Affected: `camel-redis` (test harness + file layout), one archived
  openspec design doc. No production behavior change, no spec deltas
  (`skip_specs: true` — no canonical specs touched).
- Gates: fmt · clippy (4 legs) · camel-redis + camel-component-redis
  tests · lint-unbounded-wait ratchet 296 unmoved · lint-single-source
  clean · schema untouched.
- Protocol: LIGHT — r_glm holistic over the commit set, e_glm pre-park,
  park with buzzer (no merge).
