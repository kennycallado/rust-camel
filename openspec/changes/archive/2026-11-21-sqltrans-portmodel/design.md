# Design: sqltrans-portmodel

## Prior rulings (restated, both binding)

- **e_opus surface ruling 2026-11-15:** the transaction surface is the explicit
  `transacted { datasource: X }` route block. Rejected: (a) per-exchange
  implicit transactions (invisible scope, cancellation ambiguity) and
  (c) per-endpoint `Managed` everywhere (no block boundary to own
  commit/rollback, silent auto-commit holes). Dialect scope for joins is
  PostgreSQL-only for v1.
- **e_opus split ruling 2026-11-20:** the 14-round family spec is three slices.
  Slice 1 (this change) lands the port + route model + ALL load rejections
  dark — no manager is registered, so every block fails at load with the typed
  manager-missing error; `Managed` keeps the current endpoint-creation
  rejection; slice 2 adds execution/finalization; slice 3 reworks
  `transactionMode` semantics.
- **Pre-flight: none.** The design was decided twice; no DB/driver probe is
  outstanding. This slice adds no runtime behavior, so no probe can apply.

## Approach

**Port (`crates/camel-api/src/transaction.rs`, new module).** Hand-typed boxed
futures like `DatasourceCatalog` (no `async_trait` on the new surface; the dep
already exists if reviewers prefer it):

```rust
#[non_exhaustive] pub enum TransactionState { Fresh, Active, Finalizing, Done, Detached }
#[non_exhaustive] pub enum TransactionOutcome { Committed, RolledBack, Detached }
#[non_exhaustive] pub enum TransactionError { /* see below */ }

pub trait TransactionManager: Send + Sync + 'static {
    fn datasource(&self) -> &str;
    fn open_session<'a>(&'a self) -> OpenSessionFuture<'a>; // -> Arc<dyn TransactionSession>
    fn acquire_deadline(&self) -> Duration;
}
pub trait TransactionSession: Send + Sync + 'static {
    fn datasource(&self) -> &str;
    fn state(&self) -> TransactionState;
    fn begin(&self) -> BeginFuture<'_>;      // lazy; second begin is an error
    fn set_rollback_only(&self);
    fn is_rollback_only(&self) -> bool;
    fn commit(&self) -> CommitFuture<'_>;    // -> TransactionOutcome
    fn rollback(&self) -> RollbackFuture<'_>; // -> TransactionOutcome
}
```

Why sufficient for slice 2 without signature changes: finalization IS
`commit`/`rollback` returning `TransactionOutcome`, already present; the
rollback-only flag (req 4), lazy begin (req 2), and state exposure (req 2's
`Fresh → Active → Finalizing → Done | Detached`) are present as plain methods;
`#[non_exhaustive]` lets slice 2 grow `TransactionError`/`TransactionOutcome`
variants non-breakingly. `'static` session objects allow slice 2 to downcast to
its concrete session for statement execution — the SQL execution surface stays
in `camel-sql` (its own extension trait over its concrete type), so `camel-api`
never gains a value/row model and signatures here are final. The discard guard,
hygiene, scope ids, and the 5 s finalize grace are implementation details of the
concrete session, not port surface.

**`TransactionError` variants defined now** (all names appear verbatim in the
blessed family spec, so none are invented): `ManagerMissing { datasource }`,
`ParallelInBlock`, `MulticastInBlock`, `WireTapInBlock`, `AsyncInBlock`,
`SedaInBlock`, `StreamListInBlock`, `ForeignDatasource { block, step }`,
`UnnamedDatasource { step }`, `NestedBlock { inner }`, plus the runtime names
slice 2 needs typed today: `SessionAborted`, `AbortedByHandledError`,
`CommitUncertain`, `CommitFailed`, `RollbackUncertain`, `PoolExhausted`,
`Timeout`, `Busy`, `JoinSessionMissing`, `JoinScopeMismatch`,
`RuntimeSqlInJoinedStep`, `TransactionControlSql`, `DialectUnprovable`,
`UnsupportedJoinDialect`. Struct variants carry the offender/datasource fields
the spec asserts. `CamelError::Transaction(TransactionError)` added in
`error.rs`; re-export everything from `camel-api/src/lib.rs`. No sqlx dep.

**Registry (`camel-core/src/registry.rs`).** `NamedRegistry<dyn
TransactionManager>` alias `TransactionManagerRegistry`, matching the existing
`IdempotentRegistry` pattern; `resolve_manager(name)` returns
`TransactionError::ManagerMissing` when absent. A component (slice 2: camel-sql)
registers one manager per datasource name; nothing registers in this slice.

**Route model.** `BuilderStep::Transacted { datasource, steps:
Vec<BuilderStep> }` in `route_definition.rs`, mirroring the composite variants.
YAML: `RouteDslStep::Transacted(TransactedStep { datasource, steps })` in
`route_ast.rs` with `deny_unknown_fields` (parser-level unknown-key rejection,
req 1). Fluent builder method in `camel-builder`. Canonical: `transacted`
object in `CanonicalRouteSpec` serialization (`compile.rs`) and parse-back
(`canonical.rs`) rejecting unknown keys — round-trip preserves datasource and
every nested step; nothing is dropped.

**Load validation.** A transacted-block pass invoked from route compile/load
(`camel-dsl/src/compile.rs`, same site that emits typed per-step errors):
walk the block, classify sql steps by parsing their `sql:` URI (`datasource`
param, `db_url`, StreamList-producing params), and reject: parallel split /
multicast / wiretap / async / seda / StreamList reaching a joined sql step
(each its own variant); foreign datasource; unnamed (inline `db_url`) sql step;
nested `transacted`; datasource with no resolvable manager. Every structural
check runs before the manager lookup, so each rejection is observable
independently of darkness.

**Dark proof.** No production registration exists, so any block fails with
`ManagerMissing`. Unit tests: one fixture per rejection; a model-equivalence
test comparing YAML/builder/canonical compiled step models (uses a
test-only fake manager registration, marked as such — production stays dark);
the dark test asserting `ManagerMissing`; existing `txnreject` tests untouched.

## Affected crates

- `camel-api`: transaction module, `CamelError::Transaction`, re-exports.
- `camel-core`: registry alias, `BuilderStep::Transacted`, resolver.
- `camel-dsl`: YAML/canonical authoring, block validation pass.
- `camel-builder`: fluent authoring.
- `docs`: ADR 0101 + skeleton (operational note lands with slice 2).

## Architecture boundaries

Components→api direction only: the port lives in `camel-api`, the registry in
`camel-core`, no core→component dependency, no sqlx in `camel-api` (hexagonal
boundary suite must stay green). Data/control-plane split untouched: this
slice adds no processor, no executor, no consumer path changes.

## Alternatives considered

- `async_trait` for the port: rejected — the repo's newest catalog port uses
  hand-typed futures; either compiles, reviewer may normalize.
- Putting load rejections in `startup_validation.rs`: rejected — the spec's
  rejection time is route load; compile-site checks keep offender naming exact
  and avoid a second pass.
- Defining only `ManagerMissing` now and deferring other error variants to
  slice 2: rejected — the family spec names the variants; defining them now
  keeps slice 2 free of camel-api edits, which is the point of the split.
