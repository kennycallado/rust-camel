# Proposal: sqltrans-exec-load (S2a — dark load + plumbing)

## Why

The e_opus split ruling 604c (2026-11-21) carved the former S2 into two
slices so the load-side plumbing lands dark first and the runtime-semantics
slice (S2b, `sqltrans-exec-verify`, rc-qv2iz.6) lands on a proven base. This
change is **S2a**: the borrowed-connection executor refactor with NO
behaviour change, the req-5 LOAD-side rejections, the rc-c3mjp structural
walk extended to the canonical/BuilderStep fan-ins, transaction-manager
registration, and the `DEFERRED_VARIANTS` vocabulary move. It lands DARK:
no manager is registered by runtime code paths that would let a block
execute, and every load rejection has a named unit-tier fixture.

Bd: rc-qv2iz.8 (blocks rc-qv2iz.6). Source spec:
`openspec/changes/sqltrans/specs/sql-transactions/` (frozen); canonical S1
subset: `openspec/specs/sql-transactions/spec.md`.

## What Changes

- **Phase A executor plumbing** (no behaviour change): the four producer
  execute fns move onto a borrowed-connection surface `DbHandle<'a>`; batch
  planning routes through ONE shared entry point (`plan_batch_binds`) — the
  req-12 planner clause — without changing `sql-binds` whole-batch rules or
  `plan_binds`' signature.
- **req-5 LOAD rejections**: static-SQL-only for joined steps
  (`RuntimeSqlInJoinedStep` incl. `file:`), transaction-ending SQL rejected
  (`TransactionControlSql`), dialect from the datasource URL scheme
  (`DialectUnprovable`, `UnsupportedJoinDialect`), dynamic targets rejected
  (`DynamicTargetUnprovable` for `RecipientList`/`RoutingSlip` in-block).
- **rc-c3mjp closed**: the S1 structural walk extended to `BuilderStep` trees
  at BOTH fan-ins (the `compile_step` `Transacted` arm — the shared point for
  ALL authoring paths — and the canonical arm in `commands.rs`).
- **Registry + registration**: the `RuntimeBus` registration entry point;
  camel-bundles registers a `SqlTransactionManager` per catalog datasource at
  boot; camel-cli's assembly path registers through the same entry point.
- **`DEFERRED_VARIANTS` move**: `CamelError::Transaction` becomes a supported
  in-pipeline kind; the vocabulary-guard test asserts the moved bucket.
- **Excluded** (S2b, `sqltrans-exec-verify`): manager/session implementation,
  join channel, block executor, PG suite, docs. Also excluded (S3):
  `Managed` join-required rework, endpoint creation acceptance, Auto-fallback
  warnings, `txnreject` rewrite. No XA/2PC, no per-exchange transactions, no
  transaction EIP, no new route surface.

## Acceptance criteria

- Every load rejection (S1 structural set + req-5 join-side SQL guards +
  dynamic targets) has a named unit-tier fixture asserting the exact variant
  and offender/datasource fields, passing on BOTH authoring paths
  (declarative and canonical/BuilderStep).
- Phase A refactor changes plumbing only: `cargo test -p camel-component-sql`
  green unmodified; `plan_binds` signature unchanged.
- Managers register at bundle boot; a transacted block on an unregistered
  datasource still fails at load with typed `ManagerMissing`.
- The vocabulary guard classifies `Transaction` in its new bucket.
- `openspec validate sqltrans-exec-load --type change --json` clean.

## Risk budget

No behaviour change beyond load-time rejections (which are the deliverable).
No new dependencies. The camel-api port is NOT touched by S2a (the port delta
— `Any` supertrait + join module — belongs to S2b and was accepted by the
604c ruling there).

## Affected crates

camel-sql (executor plumbing), camel-api (additive
`TransactionError::DynamicTargetUnprovable` variant only), camel-core (walk +
registration seam), camel-dsl (vocabulary guard + canonical fan-in),
camel-bundles (registration; camel-cli registers transitively through
`camel_bundles::boot`), AGENTS.md (no lane changes expected).
