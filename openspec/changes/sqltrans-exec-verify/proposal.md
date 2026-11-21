# Proposal: sqltrans-exec-verify

## Why

Slice 1 (`sqltrans-portmodel`, landed `059aedd7b`) delivered the transaction
port, the route model, and every load-time rejection — landing dark: no
manager registers, so every `transacted` block still fails at load with
`ManagerMissing`, and no `sql:` step can execute inside a block. The transacted
block therefore does not exist at runtime yet. This slice makes it execute:
one owned transaction per block, one connection, lazy begin, block-owned
commit/rollback, and the failure semantics PostgreSQL actually produces
(probe-verified: `23503` at COMMIT → typed `CommitFailed`; backend killed at
COMMIT (`FATAL 57P01`) → `CommitUncertain` + detach; hygiene ROLLBACK proves a
prior borrower's abandoned write cannot be committed — the probe reproduced the
poisoning commit).

Bd: rc-qv2iz.6 (slice 2 of the rc-qv2iz family, e_opus split ruling
2026-11-20). Source spec: `openspec/changes/sqltrans/specs/sql-transactions/`
(frozen); canonical S1 subset: `openspec/specs/sql-transactions/spec.md`.

## What Changes

- **S2 delta spec**: the ten S2-owned requirements (lazy begin/block-owned
  finalization, cancellation cutoff, rollback-only handling, join semantics,
  busy session, block timeout, uncertain-finalization discard, pool
  exhaustion, bind-path invariance, idempotent composition), worded from the
  frozen source. The req-15 operational remainder (camel-sql operational note,
  docker-suite runtime scenarios, idempotent-key disposition tests) is
  delivered against the already-canonical requirement 15 — no restatement.
- **Phase A** — borrowed-connection executor (no behaviour change): the four
  producer execute fns run on a `&mut AnyConnection` borrowed at the call
  site; batch planning routes through one shared planner entry point.
- **Phase B** — owned transaction + finalization: `SqlTransactionManager`/
  `SqlTransactionSession` in camel-sql (discard guard, hygiene, lazy begin,
  try_lock busy slot, commit classification, 5 s finalize grace); the camel-core
  `Transacted` lowering (structural walk at the canonical fan-in closing
  rc-c3mjp, scope baking, block executor); registry registration at bundle
  boot; dynamic-target in-block rejection (`RecipientList`/`RoutingSlip`);
  `DEFERRED_VARIANTS` resolution for `CamelError::Transaction`; PG
  testcontainers suite in camel-test; ADR 0101 amendment + docs.
- **Excluded** (S3, rc-qv2iz.7): `Managed` join-required rework, endpoint
  creation acceptance, Auto-fallback warning removal, `txnreject` rewrite.
  Also excluded: XA/2PC, per-exchange transactions, a transaction EIP.

## Acceptance criteria

- Outbox two-write route commits or rolls back as one unit under injected
  failure (PG suite).
- `max_connections=1` join does not self-deadlock; concurrent exchanges get
  distinct backend PIDs; discard leaves the pool usable.
- `CommitFailed` (23503 at COMMIT) and `CommitUncertain` (connection killed)
  distinguishable and both probe-verified against real PostgreSQL.
- All six design probes observed and recorded in `design.md` before the first
  bless; no frozen-spec contradictions.
- `bash scripts/gates-local.sh` exit 0 including the PG suite run locally;
  `openspec validate sqltrans-exec-verify --type change --json` clean.

## Risk budget

camel-api port signatures stay unchanged EXCEPT one disclosed additive item:
the `Any` supertrait on `TransactionSession` (S1's port doc promises "the
implementing crate can downcast the trait object to its concrete type", which
is unrealizable without it; trait-object upcasting is stable on the workspace
MSRV 1.96). Details in `design.md`. New dependencies: none beyond the
sqlx/postgres features already in the tree. Probe contradictions stop the
mission (`blocked: design`), never rewrite the frozen spec.

## Affected crates

camel-api (additive join channel + `as_any`), camel-sql (manager/session,
executor refactor, registration), camel-core (lowering walk, block executor,
scope baking), camel-dsl (vocabulary guard), camel-bundles (manager
registration), camel-test (PG suite), docs (ADR amendment, operational note,
sql-transactions page), AGENTS.md (lane rows).
