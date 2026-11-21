# Proposal: sqltrans-exec-verify (S2b — runtime semantics)

## Why

Slice 1 (`sqltrans-portmodel`, landed `059aedd7b`) delivered the transaction
port, the route model, and every load-time rejection — landing dark. Per the
e_opus split ruling 604c (2026-11-21), the former S2 is now TWO slices: this
change is **S2b, the runtime semantics** — it makes the block execute: one
owned transaction per block, one connection, lazy begin, block-owned
commit/rollback, and the failure semantics PostgreSQL actually produces
(probe-verified: `23503` at COMMIT → typed `CommitFailed`; backend killed at
COMMIT (`FATAL 57P01`) → `CommitUncertain` + detach; hygiene ROLLBACK proves a
prior borrower's abandoned write cannot be committed — the probe reproduced
the poisoning commit). The load-side half (executor plumbing, req-5 LOAD
rejections, the rc-c3mjp walk, registry registration, `DEFERRED_VARIANTS`
move) is **S2a (`sqltrans-exec-load`, rc-qv2iz.8)**, which S2b depends on and
must land first.

Bd: rc-qv2iz.6 (blocked by rc-qv2iz.8). Source spec:
`openspec/changes/sqltrans/specs/sql-transactions/` (frozen); canonical S1
subset: `openspec/specs/sql-transactions/spec.md`.

## What Changes

- **S2b delta spec**: requirements 2, 3, 4, 8, 9, 10, 11 plus the RUNTIME
  scenarios of req 5 (join channel, missing/mismatch, hygiene fixtures,
  maxconn1, distinct PIDs, sequential split, direct callee) and req 12
  (runtime), req 14, and the req-15 operational remainder (camel-sql
  operational note, docker-suite runtime scenarios, idempotent-key
  disposition tests) delivered against the already-canonical requirement 15 —
  no restatement.
- **Runtime semantics per the 604c ruling**: private `TxPhase` decision state
  (the frozen `TransactionState` cannot express BEGIN-on-the-wire or
  COMMIT-handed-off), R0–R7 cooperative finalization (total, first-match-wins,
  cutoff at DRIVER HANDOFF), `begin()` resolution inside the session
  (timeout/driver-error → typed errors, never cleanup SQL), grace enforced
  inside `commit()`/`rollback()` (one absolute finalize deadline), join
  admission via task-local channel + scope baking.
- **PORT DELTA (explicitly accepted by the 604c ruling)**: the `Any`
  supertrait on `TransactionSession` (trait-object upcast, MSRV 1.96) and the
  additive `transaction::join` task-local module. No signature changes.
- **PG suite** in camel-test (testcontainers, `postgres` feature already
  present) with one scenario per acceptance line and the probe cases as
  regression tests; lane rows; local runs via the documented `--network host`
  workaround.
- **Excluded** (S2a, `sqltrans-exec-load`): the borrowed-connection executor
  refactor (`DbHandle`, `plan_batch_binds`), the req-5 LOAD rejections, the
  rc-c3mjp BuilderStep walk, registry registration, the `DEFERRED_VARIANTS`
  move. Also excluded (S3): `Managed` join-required rework, endpoint creation
  acceptance, Auto-fallback warnings, `txnreject` rewrite. No XA/2PC,
  per-exchange transactions, transaction EIP, or new route surface.

## Acceptance criteria

- Outbox two-write route commits or rolls back as one unit under injected
  failure (PG suite).
- `max_connections=1` join does not self-deadlock; concurrent exchanges get
  distinct backend PIDs; discard leaves the pool usable.
- `CommitFailed` (23503 at COMMIT) and `CommitUncertain` (connection killed)
  distinguishable per the R7 classifier and probe-verified against real
  PostgreSQL.
- All six design probes observed and recorded in `design.md`; no frozen-spec
  contradictions.
- `bash scripts/gates-local.sh` exit 0 including the PG suite run locally;
  `openspec validate sqltrans-exec-verify --type change --json` clean.

## Risk budget

camel-api port signatures stay unchanged EXCEPT the disclosed additive port
delta above (details in `design.md`). New dependencies: none beyond the
sqlx/postgres features already in the tree. Probe contradictions stop the
mission (`blocked: design`), never rewrite the frozen spec.

## Affected crates

camel-api (additive join module + session-port `Any` supertrait), camel-sql,
camel-core (block executor + scope baking only), camel-test (PG suite), docs
(ADR amendment, operational note, sql-transactions page), AGENTS.md (lane
rows).
