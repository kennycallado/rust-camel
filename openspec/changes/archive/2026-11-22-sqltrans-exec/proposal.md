# Proposal: sqltrans-exec — code-first SPIKE

## Why

Eleven expert spec dispatches across three attempts produced zero lines of code;
every rejection named a property of code (where the walk runs; how errors
propagate through the aggregate-compensation branch). The design is fully
specified in binding rulings. This change implements it and lets tests settle
what prose could not.

## What changes

Implements the transacted-block runtime end to end on the S1 port, in nine
ordered commits (one per step, each green before the next):

1. Typed error propagation (fixes the `commands.rs` circuit-breaker fallback
   `Config` erasure).
2. Phase A: `DbHandle` + `plan_batch_binds` (req-12 planner clause; unit tests
   only, no behaviour change).
3. The LOAD walk: ONE recursive `BuilderStep` visitor at BOTH fan-ins
   (declarative and canonical lowering), depth-first in document order, req-5
   rejection fixture table, `DEFERRED_VARIANTS` move, `Managed` still rejected
   at load.
4. Registry entry point: `RuntimeBus::register_transaction_manager` + two bus
   tests + `bundle_boot_leaves_transaction_registry_empty` (zero keys).
5. Port delta (explicitly accepted by ruling 604c): `Any` supertrait on
   `TransactionSession`, the `join` task-local module, docs-only
   `rollback()`/`commit()` contract.
6. `SqlTxSession`: private `TxPhase`, total `rollback()`, `commit()` requires
   `Open`, defensive `Finalizing`/`Done` entry.
7. `TransactedSegment`: the 604d pseudocode literally, with the no-primary row
   fixture.
8. PG suite: probes P1–P6 as the frozen fixture names;
   `SQLTRANS_PG_URL` host-network override for local runs.
9. Last, separately revertible: bundle + CLI register one
   `SqlTransactionManager` per catalog datasource; replaces the empty guard.

## Normative sources (binding)

- `.opencode/fleet/orders/604c-sqltrans-s2-ruling.md` §A (R0–R7 over the
  private `TxPhase`) and §B (`begin()` resolution).
- `.opencode/fleet/orders/604d-sqltrans-s2-ruling2.md` (option ii: total
  `rollback()` in the session; segment pseudocode; docs-only port delta; S2a
  registration scope).
- `openspec/changes/sqltrans/specs/sql-transactions/spec.md` — the frozen
  family spec. It wins over every ruling.
- `/home/shared/tmp/sqltrans-exec-verify/probe-digest.md` — six observed
  PostgreSQL behaviours (P1 23503 definitive; P2 57P01 uncertain; P4 pool
  poisoning → mandatory hygiene; P5 PoolTimedOut; P6 try_lock deterministic).

## Out of scope

- `Managed` join-required (S3, `rc-qv2iz.7`) — `Managed` must stay rejected at
  load; a regression test pins this (the spike must not open S3's path).
- Any change to the frozen family spec.
- Publishing `camel-api` (the `Any` supertrait breaks external
  `TransactionSession` impls; no release in this change).

## Capabilities

### New Requirements

- `sql-transactions`: ADDs the runtime requirements — lazy begin and
  block-owned commit/rollback, cancellation cutoff, handled joined DB errors
  are rollback-only, join semantics (runtime half), busy block session, block
  timeout, uncertain finalization detaches the connection, pool exhaustion,
  bind path invariance, idempotent consumer composition.
