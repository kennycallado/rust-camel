# Proposal: idemon

## Why

Two silent data hazards in `idempotent_consumer` (`crates/camel-processor/src/idempotent_consumer.rs`), reproduced by the demo team on 0.68.0:

1. **rc-36ujy (p0)** — `eager: true` + route-level retries + no `remove_on_failure`: the first attempt reserves the key, the child fails, the retry finds the key, skips the child, and the route ends SUCCESSFULLY. Work ran zero times. The failed-rollback path (`:193`) only logs a warning and produces the same "zero work, reported success" outcome.
2. **rc-v67ye (p2)** — an absent or empty key silently disables idempotency (`:62`), and the Sync arm (`:56`) uses `Some("")` as a real key, so every keyless message after the first is dropped as a duplicate. The duplicate path emits no property and no metric, so a skip cannot be observed.

Both hazards fail silently on the untrusted-data boundary (ADR-0032) with an unundoable side effect (a lost duplicate gate). ADR-0090 asks for loud failure, not silent fallback.

## What Changes

- `OnMissingKey` enum (`fail` | `process`), default `fail`, on the async AND sync arms of the step; empty and absent keys treated identically (fixes the Sync-arm `Some("")` collision).
- Under `eager`, `remove_on_failure` defaults to `true`; an explicit `false` stays as an opt-in at-most-once contract and raises a load-time warning.
- A failed eager rollback fails the exchange with the child's error verbatim as the primary error (structural `on_exceptions` matching preserved) and the rollback failure attached as structured context; the key enters a shared pending-rollback set with single-owner recovery — a re-entry with such a key retries the remove first, then reserves and runs; a retry never passes as a duplicate.
- The non-eager post-success add failure (`:183`) keeps the exchange successful, logs at error level, and increments a metric.
- Duplicate skips set an exchange property and increment a counter — no silent skips.
- DSL: `on_missing_key` exposed in `route_ast` + compile; `schemas/dsl/route-schema.json` + TS types regenerated; `deny_unknown_fields` kept.
- New ADR-0099 on idempotent reservation semantics; `docs/src/` updates for the step and eager semantics.

Out of scope: Redis executor work (mission 587), `namespace:` on the step (deferred, recorded in design.md).

## Acceptance criteria

- `cargo test -p camel-processor --lib` covers every new rule (missing-key fail/process, empty-key parity, eager rollback default, explicit opt-in warning path, failed rollback, pending-rollback retry, duplicate observability, non-eager add failure).
- DSL accepts `on_missing_key: fail|process`; invalid values rejected at parse/compile time with an error naming the field (`OnMissingKey::parse`), unknown fields by `deny_unknown_fields`; both covered by DSL tests.
- `cargo xtask schema --check` passes (schema + TS types regenerated).
- `docs` build + `mdbook test` pass; ADR-0099 exists and cites ADR-0023, ADR-0032, ADR-0090.

## Risk budget

Acceptable: breaking change to step semantics (fail-closed default); `IdempotentConsumerSegment::new` signature change (internal seam, two call sites). Not acceptable: silent fallbacks, repository behavior changes beyond what semantics require, or the infeasible load-time eager+retries rejection (e_opus ruling: do not attempt).

bd: rc-36ujy, rc-v67ye.
