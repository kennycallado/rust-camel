# Proposal: sqltrans

## Why

Two `sql:` steps cannot be atomic today, so the transactional-outbox pattern
(aggregate state UPDATE + outbox INSERT) has no supported surface. The bd issue
(rc-qv2iz, RC-46) demanded a design choice; the e_opus ruling of 2026-11-15
(bd comment, verbatim) picked surface (b): an explicit
`transacted { datasource: X }` route block — a transaction's scope and commit
point are route semantics, so they must be lexically visible and must not be
inferred from exchange completion. `txnreject` (`9f5c0bc33`) already rejects
`transactionMode=Managed` at endpoint creation and points here;
`sqlbinds` (`e49494534`) landed the one bind path (`plan_binds`), unblocking
the borrowed-connection executor refactor the ruling requires.

## What Changes

- **ADR 0101** (`docs/adr/0101-*.md`): the ruling, rejected surfaces (a)/(c),
  concurrency and pool cost, the at-least-once window for `idempotent_consumer`.
- **`TransactionManager` port in camel-api**: provider-neutral, object-safe,
  `#[non_exhaustive]` error types, full `///` docs. `camel-sql` implements it
  over sqlx; no core→component dependency in the other direction.
- **Route model + DSL**: a `transacted { datasource }` block composing the
  existing step model — `camel-core` `BuilderStep`, `camel-builder` fluent API,
  `camel-dsl` YAML (strict unknown-key rejection), canonical form in agreement.
- **Load-time validation** (fail-closed, typed, naming offender + datasource):
  parallel split/multicast/wiretap/async/seda reaching a joined sql step
  (distinct cases); StreamList production inside a block unconditionally; sql
  step on another datasource (no 2PC); `Managed` outside any block
  ("join required"); nested blocks; `Auto` inside a block **joins** (decided,
  documented); joined steps are PostgreSQL-only in v1 (`UnsupportedJoinDialect`
  for any other resolved dialect — SQLite attached-database journal state and
  MyISAM writes are not provable from configuration); `Managed` moves from
  endpoint-creation rejection to route-load
  validation; the executor Auto-fallback warnings are removed.
- **Execution**: lazy begin at first joined step; commit on body `Ok`;
  rollback on `Err`/cancel/timeout (cooperative cancellation awaits a bounded
  rollback; a forced drop detaches the connection — DiscardOnDrop — and any
  unacknowledged rollback/commit detaches likewise); `try_lock` busy → typed
  error; block timeout default 30 s; pool exhaustion bounded by
  `acquire_timeout` with a typed error naming datasource, `max_connections`,
  waited time.
- **Spec delta**: new capability `sql-transactions`; `sql-binds` stays
  normative (`plan_binds` remains the only bind path inside a joined step).
- **Tests** (docker Postgres, `camel-test/tests/sql_test.rs`) + docs page +
  `camel-sql` operational note (poisoned statement caches on upgrade;
  PgBouncer `server_reset_query=DISCARD ALL`).

Affected crates: camel-api, camel-sql, camel-core, camel-dsl, camel-builder,
camel-test; docs.

## Acceptance criteria

- Outbox two-write commits or rolls back as one unit under injected failure.
- Concurrent exchanges get separate transactions; no shared connection.
- `max_connections=1` does not self-deadlock (the join proves it).
- Crash/cancel leaves the pool usable: a forced drop or an unacknowledged
  finalization detaches the connection (a cooperative cancel with an
  acknowledged rollback returns the connection to the pool normally).
- Every load rejection has a named typed error and a test (parser-level
  unknown-key rejection asserts the offending key only).
- `bash scripts/gates-local.sh` exits 0; the docker `sql_test` suite passes
  locally before parking; `openspec validate sqltrans --type change` passes.

## Risk budget

Acceptable: refactoring the four producer execute fns onto a borrowed
connection — the transaction-scoped path is already proven by `execute_batch`.
Out of bounds: XA/2PC, per-exchange transactions, hooks/EIP completion
coupling, growing the block into a general unit-of-work EIP, re-opening the
bind planner, touching the unit-test testkit (`camel-cli`, `camel-mock`).
