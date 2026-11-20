# Tasks: sqltrans

<!-- Whole plan (Phases A–E) blessed once in STAGE 2. Phase grouping per design.md ## Phases. -->

## Phase A: ADR 0101 + port skeleton (no behavior change)

### camel-api

#### Task A1: TransactionManager port, session traits, typed errors

**Files:**
- `crates/camel-api/src/transaction.rs` (new)
- `crates/camel-api/src/error.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified)

**Steps:**
1. Create `transaction.rs` with a module doc stating Contract C1 (failures are `Err(CamelError)`, never silent), the ownership rule (the session's discard guard is the SOLE owner of the pooled connection; borrows are short; a borrowing `Transaction` is never stored beside its owner), the one-time admission hygiene rule, the `CommitFailed`-vs-`CommitUncertain` classification rule, and the observable cancellation cutoff (driver handoff after the final cancellation check).
2. Define `pub struct ScopeId(u64)` (Copy, Debug, PartialEq, Eq, Hash) — compile-time lexical block identity shared core↔component. Plain (NOT `#[non_exhaustive]`: ADR-0049 binds contract ENUMS only, and this struct is constructed cross-crate); constructor `ScopeId::new(u64)` + `pub fn next() -> ScopeId` backed by a static `AtomicU64` in camel-api (single definition, one counter owner).
3. Define `#[non_exhaustive] pub enum TransactionOutcome { Committed, RolledBack, Detached }` — `///` doc per variant: `Committed` only after acknowledged COMMIT; `RolledBack` only after acknowledged ROLLBACK; `Detached` = connection discarded, outcome unconfirmed.
4. Define `#[derive(Debug)] #[non_exhaustive] pub enum TransactionError` with exactly these variants and fields: `Busy { datasource: String }`, `PoolExhausted { datasource: String, max_connections: u32, waited: Duration }`, `Timeout { datasource: String, elapsed: Duration }`, `JoinRequired { datasource: String, endpoint: String }`, `ForeignDatasource { step_datasource: String, block_datasource: String, route_id: String, step_id: String }`, `UnnamedDatasource { route_id: String, step_id: String }`, `ParallelInsideBlock { kind: String, route_id: String, step_id: String, datasource: String }`, `StreamListInsideBlock { route_id: String, step_id: String, datasource: String }`, `NestedBlock { route_id: String, inner_step_id: String }`, `ManagerMissing { datasource: String }`, `RollbackUncertain { datasource: String }`, `CommitUncertain { datasource: String }`, `CommitFailed { datasource: String, source: String }`, `AbortedByHandledError { datasource: String }`, `SessionAborted { datasource: String }`, `DynamicTargetUnprovable { route_id: String, step_id: String }`, `TransactionControlSql { route_id: String, step_id: String, statement_class: String }`, `RuntimeSqlInJoinedStep { route_id: String, step_id: String }`, `JoinSessionMissing { datasource: String }`, `JoinScopeMismatch { datasource: String, expected: ScopeId, found: ScopeId }`, `DialectUnprovable { route_id: String, step_id: String, scheme: String }`, `UnsupportedJoinDialect { dialect: String, route_id: String, step_id: String }`. (Distinguish: resolved non-PostgreSQL dialect → `UnsupportedJoinDialect`; URL scheme that resolves to no known dialect → `DialectUnprovable`.) Implement `Display` (manual or thiserror) so every message names the datasource and/or offender; implement `std::error::Error`.
5. Define object-safe traits (`#[async_trait::async_trait]`, `Send + Sync + Debug + 'static`, ADR-0023 pattern):
   - `pub trait TransactionManager`: `fn datasource(&self) -> &str;` and `async fn begin(&self) -> Result<std::sync::Arc<dyn TransactionSession>, CamelError>;`
   - `pub trait TransactionSession`: `fn datasource(&self) -> &str; fn scope(&self) -> ScopeId; fn try_join(&self) -> Result<TransactionJoinGuard, TransactionError>;` (single admission, `Err(Busy)` when held) `async fn commit(&self) -> Result<TransactionOutcome, CamelError>; async fn rollback(&self) -> Result<TransactionOutcome, CamelError>;`
   - `pub struct TransactionJoinGuard(Box<dyn Any + Send>)` with `fn new(inner: Box<dyn Any + Send>) -> Self` and `fn into_inner(self) -> Box<dyn Any + Send>` — opaque to camel-api; the implementing component downcasts to its concrete slot guard. Plain struct (not `#[non_exhaustive]` — constructed cross-crate in camel-sql).
6. Add `#[non_exhaustive] Transaction(TransactionError)` variant to `CamelError` in `error.rs`; add the match arm in `variant_name()` returning `"Transaction"`; update the exhaustive `variant_name_covers_all_variants` test (error.rs ~L1339) to cover the new variant.
7. Re-export in `lib.rs`: `pub use transaction::{ScopeId, TransactionError, TransactionJoinGuard, TransactionManager, TransactionOutcome, TransactionSession};`

**Tests:**
- `transaction_error_display_names_datasource`: construct `Busy`, `PoolExhausted`, `Timeout`, `JoinRequired`, `RollbackUncertain`, `CommitFailed` → `format!("{e}")` contains the datasource value; `Debug` works for every variant.
- `camel_error_transaction_variant_name`: `CamelError::Transaction(TransactionError::ManagerMissing{..}).variant_name()` returns `"Transaction"`.
- `transaction_session_trait_is_object_safe`: `fn _assert(_: Arc<dyn TransactionManager>) {}; fn _assert2(_: Arc<dyn TransactionSession>) {}` compiles.

**Acceptance:**
- `cargo build -p camel-api` exits 0
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api --no-deps` exits 0 (full rustdoc on every public item)
- `cargo test -p camel-api --lib` passes

- [ ] A1

### docs

#### Task A2: ADR 0101

**Files:**
- `docs/adr/0101-sql-transacted-blocks.md` (new)

**Steps:**
1. Verify `docs/adr/` has no 0101 (`ls docs/adr | grep 0101` must be empty) and follow the header format of `docs/adr/0100-confined-json-schema-reference-resolution.md` (authoritative `Status: Accepted` + date line).
2. Write the ADR with these sections: Context (RC-46/rc-qv2iz, outbox needs two atomic sql steps, txnreject `9f5c0bc33` rejects Managed, sqlbinds `e49494534` landed one bind path); Decision (explicit `transacted { datasource }` block — scope and commit point are route semantics, lexically visible, never inferred from exchange completion); Rejected surfaces ((a) per-exchange: pooled connection held across every later step incl. slow HTTP, clones share it invisibly, core completion coupled to a component resource; (c) per-endpoint: insufficient for the outbox); Semantics (lazy begin, block-owned commit/rollback, join semantics — sequential split joins, `direct:` callees do not inherit, `Auto` inside a block joins, Managed = join required); Concurrency and pool cost (every joined step shares one connection — max_connections=1 safe via join, pool exhaustion bounded by `acquire_timeout` default 30 s, block timeout default 30 s with 5 s finalize grace); PostgreSQL-only joins in v1 and why (SQLite attached-database journal state and MyISAM writes are not provable from configuration); Fresh-transaction hygiene (one-time hygiene ROLLBACK before BEGIN; abandoned writes discarded); Commit classification (`CommitFailed` = definitive rejection e.g. PG deferred FK `23503`; `CommitUncertain` = connection-fatal e.g. `FATAL 57P01`, detach, no rollback claim); Cancellation cutoff (driver handoff after the final cancellation check; forced drop detaches via the armed guard); `idempotent_consumer` at-least-once window (block inside the idempotent body; remove_on_failure/pending-set key dispositions per ADR-0099; outbox rows need a unique message id; relay/downstream dedupe); Operational notes (restart on upgrade clears poisoned statement caches; PgBouncer session mode keeps `server_reset_query=DISCARD ALL`).
3. Keep the prose in Simplified Technical English (ste-writing discipline); no Spanish; no conformance claims to Apache Camel (ADR-0046).

**Tests:** (none — prose deliverable)

**Acceptance:**
- `ls docs/adr/0101-*` shows exactly one file
- ADR header carries authoritative `Status` metadata like 0100
- `docs` guide builds: `cd docs && mdbook build` exits 0

- [ ] A2

## Phase B: route model + DSL + load-time validation

### camel-component-api

#### Task B1: TransactedJoin endpoint capability + statement-source classification

**Files:**
- `crates/components/camel-component-api/src/endpoint.rs` (modified)

**Steps:**
1. Add `#[derive(Clone, Copy, Debug, PartialEq, Eq)] #[non_exhaustive] pub enum SqlStatementSource { Static, Runtime, File }` (Static = statement text in the endpoint URI; Runtime = header/body expression; File = `file:`-resolved).
2. Add `#[derive(Clone, Debug)] #[non_exhaustive] pub struct TransactedJoinInfo` — non_exhaustive ENUMS only per ADR-0049; this struct is constructed cross-crate (camel-sql), so keep it plain: `pub struct TransactedJoinInfo { pub datasource: Option<String>, pub mode: TransactionModeKind, pub source: SqlStatementSource, pub statement: Option<String>, pub dialect: DialectKind }` with `#[non_exhaustive] pub enum TransactionModeKind { Auto, Managed }` and `#[non_exhaustive] pub enum DialectKind { Postgres, Sqlite, Mysql, Unresolved { scheme: String } }` (dialect from the resolved URL scheme; `Unresolved` carries the scheme for `DialectUnprovable`) in the same module.
3. Add a default method to the `Endpoint` trait: `fn transacted_join(&self, uri: &str) -> Option<TransactedJoinInfo> { let _ = uri; None }` with a `///` doc: components whose endpoints can join a `transacted` block override it; `None` = not a transaction participant. Additive default — no existing implementor breaks.
4. Rustdoc every new public item.

**Tests:**
- `transacted_join_default_is_none`: a minimal `Endpoint` test double without an override returns `None` for any URI.

**Acceptance:**
- `cargo build -p camel-component-api` exits 0
- `cargo test -p camel-component-api --lib` passes
- `cargo clippy -p camel-component-api --all-targets -- -D warnings` exits 0

- [ ] B1

### camel-sql

#### Task B2: SqlEndpoint transacted_join info + Managed endpoint-acceptance migration

**Files:**
- `crates/components/camel-sql/src/endpoint.rs` (modified)
- `crates/components/camel-sql/src/lib.rs` (modified)
- `crates/components/camel-sql/src/config.rs` (modified)
- `crates/components/camel-sql/src/producer.rs` (modified)
- `crates/components/camel-sql/src/consumer.rs` (modified)

**Steps:**
1. Implement `Endpoint::transacted_join` on `SqlEndpoint`: parse the stored `SqlEndpointConfig`; return `Some(TransactedJoinInfo { datasource: config.datasource_name.clone(), mode: config.transaction_mode into TransactionModeKind, source, statement: Some(text), dialect: DialectKind from the resolved URL scheme (postgres/sqlite/mysql → known; anything else → Unresolved { scheme }) })` where `source` is `Static` when the statement came from the URI, `Runtime` when it resolves from header/body expression params, `File` when from a `file:` source (classification helper `fn statement_source(params) -> SqlStatementSource` in config.rs next to the existing param parsing).
2. Delete the SQL-002 rejection: remove `SqlEndpointConfig::validate_transaction_mode` and its call in `SqlComponent::create_endpoint` (lib.rs L95). Endpoint creation now accepts `transactionMode=Managed` (validity is route-level).
3. Add to `SqlEndpointConfig` a retained `fn transaction_mode_error_hint(uri: &str) -> String` producing the redacted join-required guidance text (used by the producer/consumer safety-net error messages only; the core load rejection message comes from camel-api's `TransactionError::JoinRequired` Display). Messages keep `camel_api::redact::redact_url` treatment.
4. Producer safety net: in `SqlProducer`, add field `managed_join_state: ManagedJoinState` (enum `Unjoined | Joined(ScopeId)` — `ScopeId` from camel-api), default `Unjoined`; in `Service<Exchange>::call` BEFORE any pool access: if `config.transaction_mode == Managed && managed_join_state == Unjoined` → return `CamelError::Transaction(TransactionError::JoinRequired { datasource, endpoint })` (zero pool access, zero writes). Remove the L251-253 "transactionManager not yet implemented; using Auto mode" warn and its comment.
5. Consumer safety net: same pattern in `Consumer::start` (before pool init at the former L611-613 warn site): Managed + Unjoined → `JoinRequired` error; remove the warn.
6. Rewrite the txnreject tests deliberately: `create_endpoint_rejects_managed_transaction_mode` → rename to `create_endpoint_accepts_managed_transaction_mode` (asserts creation succeeds and `transacted_join().mode == Managed`); `managed_reject_is_config_error_not_parse_error` → replace with `managed_join_state_defaults_to_unjoined` (asserts the safety-net precondition); keep every redaction assertion in place.

**Tests:**
- `create_endpoint_accepts_managed_transaction_mode`: build `SqlComponent::new()`, `create_endpoint("sql:SELECT 1?transactionMode=Managed")` → `Ok`; `endpoint.transacted_join(uri).unwrap().mode == TransactionModeKind::Managed`.
- `transacted_join_classifies_static_runtime_file`: three URIs (static SQL in URI / body-expression param / `file:` param) → `SqlStatementSource::{Static, Runtime, File}`.
- `transacted_join_datasource_from_uri`: `sql:SELECT 1?datasource=pg` → `datasource == Some("pg")`; without `datasource` → `None`.
- `managed_producer_unjoined_fails_before_pool_access`: `SqlProducer` with Managed config, `managed_join_state == Unjoined`, pool OnceCell still un-initialized → `call(exchange)` returns `CamelError::Transaction(JoinRequired)`; pool cell remains un-initialized (zero side effects).
- `consumer_managed_unjoined_fails_before_pool_access`: same shape for `Consumer::start` — error before pool init.
- `auto_producer_unjoined_runs_normally`: Auto config unchanged — no JoinRequired (regression pin for the migration).

**Acceptance:**
- `cargo test -p camel-component-sql` passes (txnreject tests rewritten, coverage not deleted)
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` exits 0
- `grep -n "not yet implemented" crates/components/camel-sql/src/*.rs` returns nothing

- [ ] B2

### camel-api

#### Task B3: SQL text classifier for transaction-control statements

**Files:**
- `crates/camel-api/src/transaction.rs` (modified)

**Steps:**
1. Add `#[non_exhaustive] pub enum StatementClass { Begin, Commit, Rollback, Savepoint, JournalMode }`.
2. Add `pub fn classify_transaction_control(sql: &str) -> Option<StatementClass>`: strip `--` line comments, `/* */` block comments, and string literals first; then word-boundary, case-insensitive match of a leading statement keyword or any top-level occurrence of `BEGIN`(transaction), `COMMIT`, `ROLLBACK`, `SAVEPOINT`, `RELEASE SAVEPOINT`, `START TRANSACTION`, `PRAGMA journal_mode` (and `PRAGMA journal_mode=` assignment form). Multi-statement text (batch items split later by the planner) is checked per item by the caller.
3. Full `///` docs with the ruling rationale (user SQL must not defeat the block's commit/rollback).

**Tests:**
- `classifier_flags_each_control_class`: `COMMIT`, `commit;`, `BEGIN WORK`, `START TRANSACTION`, `ROLLBACK TO SAVEPOINT x`, `SAVEPOINT s`, `PRAGMA journal_mode=WAL` → the matching `StatementClass`.
- `classifier_ignores_innocuous_sql`: `SELECT * FROM commit_log WHERE note = 'COMMIT'`, `INSERT INTO t (a) VALUES (1)`, `-- COMMIT\nSELECT 1`, `/* ROLLBACK */ SELECT 1` → `None`.
- `classifier_handles_batch_text`: `UPDATE a SET x=1; COMMIT;` → `Some(Commit)`.

**Acceptance:**
- `cargo test -p camel-api --lib` passes
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0

- [ ] B3

### camel-core

#### Task B4: BuilderStep::Transacted + manager resolution + skeleton compile path

**Files:**
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)

**Steps:**
1. Add `BuilderStep::Transacted { datasource: String, timeout_secs: Option<u64>, steps: Vec<BuilderStep> }` next to `BuilderStep::IdempotentConsumer` (same struct-variant style, doc citing ADR-0101).
2. Add to the component harvest: `#[async_trait] pub trait TransactionManagerSource: Send + Sync` with `fn transaction_manager_for(&self, datasource: &str) -> Option<Arc<dyn TransactionManager>>`; harvest it in the component Registry at registration (ADR-0041 metadata-harvest precedent), and expose `CompilationContext::transaction_manager(&self, datasource: &str) -> Option<Arc<dyn TransactionManager>>` reading the harvested sources.
3. Extend `CompilationContext` with `pub transacted: Option<TransactedCompileCtx>` where `TransactedCompileCtx { scope_id: ScopeId, datasource: String, manager: Arc<dyn TransactionManager> }` — captured at step resolution like `intercept` (ADR-0064); default `None`.
4. Add the `BuilderStep::Transacted` arm to the step compiler: resolve `ctx.transaction_manager(datasource)` → fail-closed `CamelError::Transaction(TransactionError::ManagerMissing { datasource })`; generate a fresh `ScopeId`; recurse into children WITH `ctx.transacted` set; compile children into an outcome-aware segment; return a `TransactedProcessor` that (this task) executes the children segment sequentially with no transaction semantics — the transactional execution path is task C3's deliverable, and the processor is the exact struct C3 fills (name it `TransactedProcessor` now, fields `datasource`, `timeout_secs`, `children: CompiledSegment`, `manager`, `scope_id`).
5. `BuilderStep::span_kind_hint`/step-name mapping for `Transacted` (label "transacted").

**Tests:**
- `transacted_step_compiles_children`: fake `TransactionManagerSource` returning a `NoopManager` for `"pg"` registered in a test context → compile `RouteDefinition` with `Transacted { datasource: "pg", steps: [two To steps] }` → compiles; produced pipeline runs both children in order (assert via mock components' invocation order).
- `transacted_unknown_datasource_fails_manager_missing`: no source provides `"audit"` → compile fails with `CamelError::Transaction(TransactionError::ManagerMissing { datasource: "audit" })` (exact variant asserted).
- `transacted_scope_ids_differ_per_block`: compile two Transacted blocks in one route (allowed at compile; nesting rejection is B5's structural rule applied to INNER blocks — two SIBLING blocks are legal) → their processors carry distinct `scope_id`s.

**Acceptance:**
- `cargo test -p camel-core --lib` passes
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0
- `cargo xtask lint-non-exhaustive` exits 0 (new public enums conform to ADR-0049)

- [ ] B4

### camel-core

#### Task B5: structural load rejections (fail-closed, typed)

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/transacted_validation.rs` (new)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/transacted_validation_test_support.rs` (new)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs` (modified — module declaration)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified — call validator in the Transacted arm)
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified — Managed from-URI consumer load rejection)

**Steps:**
1. Create `transacted_validation.rs` with `pub(crate) fn validate_block(steps: &[BuilderStep], block: &TransactedBlockInfo) -> Result<(), CamelError>` where `TransactedBlockInfo { datasource: &str, route_id: &str }`. Recursive walk with a `Reach` context: an "escape" construct only rejects when its subtree reaches a JOINED sql step (an endpoint whose `transacted_join(uri)` reports `datasource == block.datasource`), via `ctx.resolve_component(scheme)` + `create_endpoint` on a probe URI — reuse the same endpoint-resolution path the Send compiler uses; cache probe results per (uri) in a local map to avoid duplicate endpoint creation.
2. Implement the escape checks (each distinct, each naming offender + datasource in `ParallelInsideBlock { kind }` / `DynamicTargetUnprovable`): parallel-model `Split`, `Multicast`, `WireTap` (`DivertCopyTo` semantics included), `SEDA`/async handoff (`to:` URI scheme `seda:` — rejected when the target route is resolvable in this context and reaches a joined sql step, and ALSO when the target is not resolvable at load: fail closed), `DynamicRouter` — rejected UNCONDITIONALLY inside a block (a runtime-resolved target cannot prove membership; no static subtree exists) → `DynamicTargetUnprovable`.
3. Implement the unconditional checks: `StreamList`-producing sql step anywhere inside the block (`StreamListInsideBlock`, regardless of reachability); nested `BuilderStep::Transacted` child (`NestedBlock` naming the inner block).
4. Implement the joined-step checks for each joined sql step (reuse Task B3's `classify_transaction_control`): control statement (`TransactionControlSql { statement_class }`), non-Static source (`RuntimeSqlInJoinedStep`), foreign datasource (`ForeignDatasource` naming both), unnamed datasource (`UnnamedDatasource`), dialect (from `TransactedJoinInfo.dialect`: `Postgres` → allowed; `Sqlite`/`Mysql`/any other resolved dialect → `UnsupportedJoinDialect` naming the dialect; `Unresolved { scheme }` → `DialectUnprovable` naming the scheme).
5. Wire `validate_block` into the Transacted compile arm in `core.rs` BEFORE child compilation; error paths short-circuit the load.
6. Managed-outside-block route-load rejection, BOTH compile sites: (a) the Send-step compile path — when `endpoint.transacted_join(uri)` reports `mode == Managed` and the ambient `ctx.transacted` is `None` → `CamelError::Transaction(TransactionError::JoinRequired { datasource, endpoint: uri redacted })`; when `ctx.transacted` is `Some(_)` the step joins (Auto or Managed both legal — the join wiring is C3); (b) the route consumer-construction path (from-URI endpoints, `crates/camel-core/src/lifecycle/adapters/route_compiler.rs`) — `from: sql:...?transactionMode=Managed` has no ambient block ever → `JoinRequired` at load. Tests need a fake `DatasourceCatalog` fixture for `datasource=`-named URIs (named datasources without a catalog are rejected at endpoint creation, lib.rs L97–99) — provide a test-owned stub catalog serving config-only entries.
7. Add the fixture file `crates/camel-core/src/lifecycle/adapters/step_compilers/transacted_validation_test_support.rs` (#[cfg(test)]) with the fake `DatasourceCatalog` + fake `TransactionManagerSource`.

**Tests:** (each asserts the EXACT variant and offender/datasource fields; all load-time, no Docker)
- `parallel_split_reaching_joined_sql_rejected`: block{pg} containing parallel Split whose fragment has `to: sql:INSERT ...?datasource=pg` → `TransactionError::ParallelInsideBlock { kind: "split", .. }` naming route+step+datasource.
- `multicast_reaching_joined_sql_rejected`: same via Multicast → `ParallelInsideBlock { kind: "multicast" }`.
- `wiretap_reaching_joined_sql_rejected`: same via WireTap → `ParallelInsideBlock { kind: "wiretap" }`.
- `seda_escape_reaching_joined_sql_rejected`: block{pg} with `to: seda:x`, the seda consumer route registered in the same context, its sql step joining → rejected at the BLOCK load (`ParallelInsideBlock { kind: "seda" }` naming the seda step); and a second test `seda_unresolvable_target_fail_closed`: `to: seda:never-registered` inside block{pg} → still rejected (fail closed).
- `dynamic_router_inside_block_rejected`: DynamicRouter anywhere inside a block → `DynamicTargetUnprovable` (runtime-resolved targets cannot prove membership).
- `dialect_unprovable_rejected`: joined step on a datasource with an unrecognized URL scheme (`opaque://host/db`), fake manager registered → `DialectUnprovable` naming the scheme.
- `mysql_join_rejected`: joined step on `provider = "sqlx"` + `mysql://` URL → `UnsupportedJoinDialect` naming MySQL; standalone MySQL step outside blocks still loads.
- `sqlite_join_rejected`: joined step on `sqlite://` datasource → `UnsupportedJoinDialect` naming SQLite; standalone SQLite step outside blocks still loads.
- `stream_list_inside_block_rejected_unconditionally`: streaming sql SELECT inside block, no downstream consumer → `StreamListInsideBlock`.
- `nested_transacted_rejected`: block inside block → `NestedBlock` naming the inner step.
- `foreign_datasource_rejected`: block{pg} with sql step `datasource=audit` → `ForeignDatasource { step_datasource: "audit", block_datasource: "pg" }`.
- `unnamed_datasource_rejected`: joined db_url-only sql step → `UnnamedDatasource`.
- `transaction_control_sql_rejected`: joined step text `UPDATE t SET x=1; COMMIT` → `TransactionControlSql { statement_class: "commit" }`.
- `runtime_sql_source_rejected`: joined step with body-expression SQL param → `RuntimeSqlInJoinedStep`.
- `file_sql_source_rejected`: joined step with `file:` SQL → `RuntimeSqlInJoinedStep`.
- `managed_producer_outside_block_join_required_at_load`: route with `to: sql:...?transactionMode=Managed` outside any block → `JoinRequired` naming endpoint+datasource; message preserves redaction.
- `managed_consumer_from_uri_join_required_at_load`: `from: sql:...?transactionMode=Managed` route → `JoinRequired` at load.
- `sequential_split_inside_block_allowed`: block{pg} with sequential Split fragments both joining → loads OK.
- `wiretap_without_joined_sql_allowed`: wiretap whose copy goes to `log:` only → loads OK.
- `auto_inside_block_allowed`: default-mode sql steps inside block → loads OK.
- `sibling_transacted_blocks_allowed`: two sibling blocks on different datasources → loads OK.

**Acceptance:**
- `cargo test -p camel-core --lib` passes with all 22 named tests
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0

- [ ] B5

### camel-dsl

#### Task B6: YAML `transacted` step (strict keys) + lowering

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/contract.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)

**Steps:**
1. `route_ast.rs`: add `Transacted(TransactedStep)` to the untagged `RouteDslStep` enum; `#[serde(deny_unknown_fields)] pub struct TransactedStep { transacted: TransactedData }`; `#[serde(deny_unknown_fields)] pub struct TransactedData { datasource: String, #[serde(default)] timeout_secs: Option<u64>, steps: Vec<RouteDslStep> }` (snake_case per ADR-0017; shape mirrors `DoTryStep`/`DoTryData`).
2. `contract.rs`: add `DeclarativeStepKind::Transacted`.
3. `model.rs`: add `Transacted(TransactedStepDef)` to `DeclarativeStep`; `pub struct TransactedStepDef { datasource: String, timeout_secs: Option<u64>, steps: Vec<DeclarativeStep> }` with contract docs.
4. `yaml.rs` `route_step_to_declarative_step`: new arm — reject empty `datasource` with `CamelError::RouteError("transacted: 'datasource' must not be empty")`; recursively lower `steps`.
5. `compile.rs`: `compile_transacted_step` → `BuilderStep::Transacted { datasource, timeout_secs, steps: compile_declarative_steps(children) }`.
6. Regenerate schema artifacts if the DSL schema is generated (`cargo xtask schema` ownership per xtask main) so `cargo xtask schema --check` stays green.

**Tests:**
- `transacted_yaml_parses_two_steps`: YAML with `transacted: { datasource: pg, steps: [ {to: sql:...}, {to: sql:...} ] }` → `DeclarativeStep::Transacted` with 2 children, `timeout_secs == None`.
- `transacted_yaml_timeout_secs_parsed`: `timeout_secs: 5` → `Some(5)`.
- `transacted_yaml_unknown_key_rejected`: `transacted: { datasource: pg, stepz: [] }` → load error naming `stepz` (parser rejection; no datasource fields asserted).
- `transacted_yaml_empty_datasource_rejected`: `datasource: ""` → `RouteError` "must not be empty".
- `transacted_children_lowered_recursively`: child `do_try` inside transacted lowers to nested `DeclarativeStep::DoTry`.

**Acceptance:**
- `cargo test -p camel-dsl` passes
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0
- `cargo xtask schema --check` exits 0

- [ ] B6

### camel-builder

#### Task B7: fluent `transacted` scope builder

**Files:**
- `crates/camel-builder/src/transacted.rs` (new)
- `crates/camel-builder/src/lib.rs` (modified)
- `crates/camel-builder/src/do_try.rs` (reference only — pattern source)

**Steps:**
1. Create `transacted.rs` mirroring `do_try.rs`'s builder shape: `RouteBuilder::transacted(datasource: impl Into<String>) -> TransactedBuilder { parent, datasource, timeout_secs: None, steps: Vec<BuilderStep> }`; `.to(...)`, `.process(...)` push via the shared `StepAccumulator`; `.timeout_secs(u64)` setter; `.end_transacted()` packages `BuilderStep::Transacted { datasource, timeout_secs, steps }` and returns the parent builder.
2. `lib.rs`: register `BuilderStep::Transacted { .. } => "transacted"` in the step-name mapping; re-export `TransactedBuilder` at the crate root alongside `DoTryBuilder`.
3. Rustdoc with a usage example matching the YAML form.

**Tests:**
- `transacted_builder_produces_step_with_children`: build `from(...).transacted("pg").to(sql step).to(sql step).end_transacted()` → `RouteDefinition.steps[0]` is `BuilderStep::Transacted { datasource: "pg", timeout_secs: None, steps.len() == 2 }`.
- `transacted_builder_timeout_secs_set`: `.timeout_secs(5)` → `Some(5)`.
- `transacted_builder_yaml_canonical_agreement`: the builder-produced `BuilderStep` lowers to the same canonical representation as the YAML from Task B6's parse test (agreement requirement).

**Acceptance:**
- `cargo test -p camel-builder` passes
- `cargo clippy -p camel-builder --all-targets -- -D warnings` exits 0

- [ ] B7

### camel-dsl

#### Task B8: canonical-form lowering + agreement

**Files:**
- `crates/camel-api/src/runtime.rs` (modified — `CanonicalStepSpec::Transacted` variant; ADR-0049 `#[non_exhaustive]` versioned contract enum, so downstream matches gain `_` arms)
- `crates/camel-dsl/src/canonical.rs` (modified — canonical lowering for the new variant)
- `crates/camel-dsl/src/compile.rs` (modified)
- schema artifacts (regenerated via the xtask generator if the canonical schema is generated)

**Steps:**
1. Add `CanonicalStepSpec::Transacted { datasource, timeout_secs, steps }` to the versioned contract enum in camel-api `runtime.rs` (ADR-0011/0016: the canonical form is the stable route contract — the block MUST be canonically expressible, not rejected; unknown canonical fields stay strictly rejected per ADR-0016).
2. Extend the canonical lowering in camel-dsl to emit/parse the new variant (builder, YAML, and canonical input all converge on it; note: `idempotent_consumer` is canonically REJECTED today per ADR-0016 strictness — the transacted block is the FIRST structural step added to canonical v2, which is why the contract enum lives in camel-api).
3. Regenerate schema artifacts via the xtask generator so `cargo xtask schema --check` stays green.

**Tests:**
- `transacted_canonical_roundtrip`: canonical input with a transacted block → lowered `BuilderStep::Transacted` → canonical output identical to input (modulo field order).
- `transacted_canonical_unknown_field_rejected`: canonical block with an unrecognized field → strict rejection naming the field.
- `three_authoring_paths_agree`: builder output (B7 test), YAML parse (B6 test), canonical input → byte-identical canonical serialization.

**Acceptance:**
- `cargo test -p camel-dsl` and `cargo test -p camel-api --lib` pass
- `cargo xtask schema --check` exits 0
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0

- [ ] B8

## Phase C: borrowed-connection executor + join

### camel-sql

#### Task C1: DbHandle borrowed-connection refactor

**Files:**
- `crates/components/camel-sql/src/producer.rs` (modified)
- `crates/components/camel-sql/src/consumer.rs` (modified)

**Steps:**
1. Add `pub(crate) struct DbHandle<'a> { conn: &'a mut AnyConnection }` with `Deref/DerefMut` to `AnyConnection` and `fn new(conn: &'a mut AnyConnection) -> Self`.
2. Refactor the producer execute fns — `fetch_single_rows` (L425), `execute_select` (L458), `execute_modify` (L604), `execute_batch` (L676) — to take `conn: &mut AnyConnection` instead of `pool: &AnyPool`; callers (`Service::call`) acquire the `PoolConnection` and pass `&mut *conn`.
3. `execute_batch`: replace `pool.begin()` (L749) with `conn.begin()` on the borrowed connection — the batch transaction then lives on the caller's connection (joined or pooled), preserving the existing whole-batch probe/readback semantics.
4. Refactor the consumer execute sites — `poll_database` (L92), `poll_database_stream` (L264), `execute_post_query` (L402) — the same way; keep the explicit `drop(conn)` before onConsume post-processing (single-conn pool note).
5. Keep `plan_binds`, `readback_kinds`, `probe_batch_shapes` signatures untouched (`&mut AnyConnection` already).

**Tests:**
- existing `cargo test -p camel-component-sql` suite passes unchanged (behavior-preserving refactor; the sqlbinds suite is the regression net)

**Acceptance:**
- `grep -n "pool: &AnyPool" crates/components/camel-sql/src/producer.rs` returns nothing for the four execute fns
- `cargo test -p camel-component-sql` passes
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` exits 0

- [ ] C1

### camel-sql

#### Task C2: SqlTransactionManager + SqlTransactionSession

**Files:**
- `crates/components/camel-sql/src/transaction.rs` (new)
- `crates/components/camel-sql/src/producer.rs` (modified — joined execution path)
- `crates/components/camel-sql/src/consumer.rs` (modified — joined execution path)
- `crates/components/camel-sql/src/lib.rs` (modified — TransactionManagerSource impl)

**Steps:**
1. Create `transaction.rs` with:
   - `pub struct SqlTransactionManager { datasource: String, pool: Arc<OnceCell<Arc<AnyPool>>>, catalog: Option<Arc<dyn DatasourceCatalog>> }` implementing `TransactionManager` (`begin()` resolves the pool exactly like `SqlProducer::get_or_try_init` L174–242 — catalog downcast path or direct connect with existing defaults) and `TransactionManagerSource` (on-demand per-name construction, cached in `Mutex<HashMap<String, Arc<SqlTransactionManager>>>`).
   - `TxState` private enum: `Fresh | Active | Finalizing | Done | Detached`.
   - `pub struct SqlTransactionSession { datasource, scope: ScopeId, state: TxState, slot: tokio::sync::Mutex<Option<GuardedConn>>, joined_error: Option<TransactionError> }` where `GuardedConn { conn: Option<DiscardOnDrop<PoolConnection<Any>>>, armed: bool }` — the session is the SOLE owner; the sqlx `DiscardOnDrop` guard from producer.rs L367 is moved here (or re-exported/shared) so drop-while-armed detaches.
2. `TransactionManager::begin()`: `pool.acquire()` bounded by the pool's acquire timeout; on acquire timeout → `TransactionError::PoolExhausted { datasource, max_connections: pool size, waited }`; construct the session with the checked-out connection (guard armed). `begin()` is invoked LAZILY (by the C3 `LazyBlockSession` at the first join) — a block with no joined step never reaches it.
3. `TransactionSession::try_join()` single admission: `slot.try_lock()` → held = `Err(Busy)`; then by state — `Detached` → `Err(SessionAborted)`; `joined_error.is_some()` → `Err(SessionAborted)` (admission closure BEFORE any connection I/O); `Fresh` → admission sequence: defensive dialect re-check from the resolved URL (postgres only; else `UnsupportedJoinDialect` — load-time enforcement is B5's, this is defense-in-depth), one-time hygiene ROLLBACK (unacknowledged transport failure → `RollbackUncertain` outcome + detach + `Err(RollbackUncertain)`; hygiene bounded by the acquire deadline → deadline error), `BEGIN` → `Active`; `Active` → join. In every success path, lend `&mut **guard` connection as `TransactionJoinGuard::new(JoinedSlotGuard(guard))` — the downcast-able concrete guard that derefs to `&mut AnyConnection`.
4. Joined execution path in the PRODUCER (consumers never join: `from:` endpoints are always outside blocks per B5 — no consumer join path exists): when compiled with `managed_join_state == Joined(scope)` (the C3 `ProducerContext` snapshot), `Service::call` reads the component-api task-local (`current_transacted_session()`), begins the session lazily via the manager on first join, verifies `session.scope() == baked scope` (mismatch → `JoinScopeMismatch`; absent task-local → `JoinSessionMissing`), calls `try_join`, downcasts the guard, and executes on the borrowed connection through the C1 `DbHandle` path — `plan_binds` signature unchanged. No task-local → `JoinSessionMissing`; NEVER auto-commit fallback.
5. Promote the executor seam to an explicit production step: introduce `#[async_trait] pub(crate) trait ConnExecutor { async fn execute_unprepared(&mut self, sql: &str) -> Result<ExecOutcome, CamelError>; async fn execute_plan(&mut self, plan: &BindPlan) -> Result<ExecOutcome, CamelError>; }` and convert the session-adjacent call sites (hygiene ROLLBACK, BEGIN/COMMIT/ROLLBACK finalization, joined statement execution via `plan_binds`) to it; prod impl wraps `&mut AnyConnection`; the test impl is scripted. This is the session's only I/O surface — the C1-refactored fns keep their `&mut AnyConnection` signatures and a thin adapter routes them through the trait.
6. `commit()`/`rollback()` finalization on the borrowed connection: state `Fresh` → release connection, outcome `Committed`/`RolledBack` (nothing began); `Active` → issue COMMIT/ROLLBACK through the ConnExecutor seam and classify: definitive rejection (`23503` and kin) → `CommitFailed { datasource, source }`, state `Done`, connection returned clean-known state; connection-fatal (`FATAL 57P01`, closed transport) → `CommitUncertain` + detach (guard drops without pool return), state `Detached`; acknowledged → `Committed`/`RolledBack`, disarm, state `Done`. Finalization runs under the slot lock (no concurrent join mid-finalize).
6. `lib.rs`: `impl TransactionManagerSource for SqlComponent` delegating to the resolver (names from `config.datasource_name` at endpoint creation are served on demand — no eager connect).

**Tests:** (unit-tier, scripted `ConnExecutor` mock — the production trait was introduced in step 5)
- `session_begin_issues_hygiene_then_begin`: scripted executor records `["ROLLBACK", "BEGIN"]` order on first join.
- `session_try_join_second_concurrent_is_busy`: hold the slot guard, second `try_join` → `Busy`.
- `session_aborted_admission_closes_joins`: inject statement error → `joined_error` set → next `try_join` → `SessionAborted` with ZERO executor calls recorded.
- `join_session_missing_rejected`: producer join path with NO task-local session → `JoinSessionMissing` naming the datasource, never auto-commit, zero writes.
- `session_commit_definitive_rejection_maps_commit_failed`: scripted COMMIT returns `23503` class → `CommitFailed`, state `Done`.
- `session_commit_connection_fatal_maps_commit_uncertain`: scripted COMMIT returns fatal/closed → `CommitUncertain`, state `Detached`, connection dropped not returned (assert via `DiscardOnDrop` drop count).
- `session_rollback_ack_disarms_guard`: scripted ROLLBACK ack → `RolledBack`, disarm (drop is a plain pool return).
- `session_fresh_commit_is_noop`: no joined step ever ran → commit returns `Committed` with zero writes issued.
- `join_scope_mismatch_rejected`: baked scope ≠ session scope → `JoinScopeMismatch` with both ids.

**Acceptance:**
- `cargo test -p camel-component-sql` passes (new transaction tests + existing suite)
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` exits 0
- `cargo xtask lint-unwrap` exits 0

- [ ] C2

### camel-core

#### Task C3: transacted block execution wiring (lazy session holder, scope baking, commit/rollback orchestration)

**Files:**
- `crates/components/camel-component-api/src/transacted_session.rs` (new — task-local + accessors; component-api already depends on tokio; core has NO reverse dep)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified — fill `TransactedProcessor`)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs` (modified — lazy holder plumbing)
- `crates/components/camel-component-api/src/producer_context.rs` (modified — `ProducerContext` gains the ambient transacted context)

**Steps:**
1. Create `transacted_session.rs` in camel-component-api: `task_local! { static TRANSACTED_SESSION: RefCell<Option<Arc<LazyBlockSession>>> }` where `LazyBlockSession` holds `Option<Arc<dyn TransactionManager>>` + block metadata and begins the session LAZILY on first join (task-local set before children runs, but `manager.begin()`/connection checkout happens inside the session at the first `try_join` — lazy begin per the ruling; a block with no joined sql step never checks out a connection). Public accessors: `pub fn current_transacted_session() -> Option<Arc<LazyBlockSession>>` and `#[doc(hidden)] pub fn set_current_transacted_session(Option<Arc<LazyBlockSession>>)`. camel-core sets it; camel-sql reads it (component-api has no core dep — direction is core→component-api→sql).
2. `ProducerContext` (component-api, passed at compile/producer creation) gains `pub transacted: Option<TransactedCompileCtxSnapshot>` (scope_id + datasource + manager handle snapshot — the consumer-facing side never joins: `from:` endpoints are always outside blocks per B5, so only producers bake scope). `SqlEndpoint::create_producer` (endpoint.rs L48) copies the ambient snapshot into `SqlProducer.managed_join_state = Joined(scope_id)` when the endpoint's `transacted_join` datasource matches the snapshot datasource; mismatch → compile-time `ForeignDatasource` (B5 already rejects; this is the enforcement point).
3. Fill `TransactedProcessor::run`: set the task-local lazy holder BEFORE children; run the children segment; finalize by outcome — `Ok(ex)` → if a session was begun, `commit()` (outcome mapped: `CommitFailed`/`CommitUncertain` fail the exchange; `Committed` passes through); clean `PipelineOutcome::Stopped(ex)` → commit + propagate `Stopped` unchanged (ADR-0025); `Failed(err)` / error → `rollback()` if a session was begun (body error primary; rollback uncertainty carried in the outcome, never substituted); clear the task-local in all paths (including on unwind, via a guard). If no session was ever begun (no joined step), finalize is a no-op.
4. Block timeout plumbed as a field but not yet enforced (D2 owns enforcement).

**Tests:** (unit-tier with the C2 scripted session wrapped in a `ScriptedManager`; session begin is lazy — the scripted manager records when `begin()` is called)
- `transacted_block_commits_on_ok`: children succeed → `commit()` observed, exchange passes through.
- `transacted_block_lazy_begin_no_joined_step_no_checkout`: block whose children contain NO sql step → `begin()` never called (zero connection checkout recorded).
- `transacted_block_rolls_back_on_err`: child fails → `rollback()` observed, body error preserved as primary.
- `transacted_block_stop_commits_and_propagates`: child returns `Stopped` → `commit()` observed, outcome `Stopped` propagates, steps after the block (in a longer route) do not run.
- `transacted_session_cleared_after_block`: task-local is `None` after the block in all three outcome paths.
- `transacted_join_scope_baked_into_sql_steps`: compile a block whose child sql step endpoint reports `datasource: pg` with ambient snapshot → producer constructed with `Joined(scope)`; a non-matching-datasource child (foreign) is rejected at compile (B5 pin).

**Acceptance:**
- `cargo test -p camel-core --lib`, `cargo test -p camel-component-sql`, and `cargo test -p camel-component-api --lib` pass
- `cargo clippy -p camel-core --all-targets -- -D warnings`, `-p camel-component-api`, exit 0

- [ ] C3

## Phase D: finalization semantics + typed errors

### camel-sql

#### Task D1: rollback-only closure + AbortedByHandledError

**Files:**
- `crates/components/camel-sql/src/transaction.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)

**Steps:**
1. Session: classify EVERY joined execution failure — statement execution errors AND bind/prepare probe errors surfaced from `plan_binds` (preserve the failure classification BEFORE mapping to `ProcessorError`; the session sees the classified error) — set `joined_error` (rollback-only) BEFORE the error returns to the pipeline (C2 already gates admission; this task guarantees the marker is set in the error path of every execute site).
2. `TransactedProcessor`: on finalization, if `session` reports rollback-only (expose `fn aborted(&self) -> Option<&TransactionError>` on `TransactionSession`, camel-api addition) and the body ended `Ok` or `Stopped` → force rollback and fail the block with `TransactionError::AbortedByHandledError { datasource }` (never `Committed`).
3. camel-api: add `fn aborted(&self) -> Option<&TransactionError> { None }` default to `TransactionSession`; `SqlTransactionSession` overrides.

**Tests:** (scripted executor, unit-tier)
- `joined_statement_error_marks_rollback_only`: scripted statement error → `aborted()` is `Some(_)`, next join `SessionAborted`.
- `joined_prepare_error_marks_rollback_only`: scripted prepare/probe failure (BindPlan stage) with non-empty binds → same closure; classification preserved (the surfaced `ProcessorError` still names item/parameter).
- `ok_body_after_abortion_fails_aborted_by_handled`: abort then `Ok` body → `AbortedByHandledError`, `rollback()` observed, no commit call recorded.

**Acceptance:**
- `cargo test -p camel-component-sql` and `cargo test -p camel-core --lib` pass
- clippy `-D warnings` on both crates

- [ ] D1

### camel-core + camel-sql

#### Task D2: block timeout, deadline precedence, bounded finalization

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified — `TransactedProcessor` timeout)
- `crates/components/camel-sql/src/transaction.rs` (modified — finalize grace)

**Steps:**
1. `TransactedProcessor`: bound the body with an absolute-deadline timeout — compute the block deadline instant at block start (default 30 s, `timeout_secs` override) and wrap the body in a `tokio::time::timeout` computed from that instant, so the body is ABORTED at the deadline (an after-the-fact elapsed comparison cannot enforce the Docker pg_sleep scenario); the instant comparison only selects the error branch when both deadlines are armed (acquire-first → `PoolExhausted`; block-first or tie → `Timeout { datasource, elapsed }`). On expiry → rollback (bounded by the 5 s finalize grace).
2. Acquire-deadline precedence: the session's pool acquire uses the datasource acquire timeout; when both deadlines are armed, the first-elapsed instant wins and an exact tie reports the block `Timeout` (implement by comparing absolute instants in the processor before delegating to the session error).
3. Finalize grace: session commit/rollback paths run under a `tokio::time::timeout(5 s)`; stalling past it → `RollbackUncertain`/`CommitUncertain` + detach with the primary error preserved (body/timeout error primary, uncertainty in outcome).
4. Deadline-error selection applies during admission hygiene too: the `transacted_hygiene_deadline_expiry` semantics (hygiene ROLLBACK left pending + deadline expiry → deadline error primary: `PoolExhausted` acquire-first, `Timeout` block-first/tie) and the `transacted_hygiene_unacknowledged` semantics (observable transport failure injected before either deadline → `RollbackUncertain` + `Detached`, exactly one ROLLBACK attempt, no BEGIN, detach, pool usable) are covered by the named fixtures below.

**Tests:** (tokio `start_paused` virtual clock, scripted session/executor — the named fixtures from the blessed spec)
- `transacted_block_deadline_first`: block deadline 30 s, acquire deadline 60 s, body stalls past 30 s (virtual advance) → `Timeout { datasource, elapsed }` exact fields.
- `transacted_acquire_deadline_first`: acquire deadline 5 s, block deadline 60 s → `PoolExhausted { datasource, max_connections, waited }` exact fields.
- `transacted_deadline_tie`: both deadlines at 30 s → `Timeout` (block wins the tie).
- `transacted_stalled_rollback`: body error, rollback stalls past the 5 s grace → body error primary + `RollbackUncertain`/`Detached` carried in outcome.
- `transacted_stalled_commit`: body Ok, commit stalls past the 5 s grace → `CommitUncertain`, detach, no rollback claim.
- `transacted_hygiene_unacknowledged`: observable transport failure injected on the hygiene ROLLBACK before either deadline → exactly one ROLLBACK attempt, BEGIN never sent, detach, `RollbackUncertain` + `Detached` surfaced, pool usable.
- `transacted_hygiene_deadline_expiry`: hygiene ROLLBACK left pending, virtual clock advanced through acquire-first (→ `PoolExhausted`), block-first (→ `Timeout`), and tie (→ `Timeout`) cases; each asserts one ROLLBACK attempt, no BEGIN, detach, pool usable.

**Acceptance:**
- `cargo test -p camel-core --lib` and `cargo test -p camel-component-sql` pass
- clippy `-D warnings` both crates

- [ ] D2

### camel-sql

#### Task D3: commit classification + cancellation cutoff + forced-drop detach

**Files:**
- `crates/components/camel-sql/src/transaction.rs` (modified)

**Steps:**
1. `fn classify_commit_outcome(db_err: &CamelError) -> CommitClass { Definitive, ConnectionFatal }`: definitive (constraint/integrity SQLSTATE classes, e.g. PG `23503`) → `CommitFailed { datasource, source }` with the connection's state known (release per policy); connection-fatal (`FATAL 57P01`, transport closed) → `CommitUncertain` + detach + NO rollback issue.
2. Cancellation cutoff: the session's `commit()` marks `Finalizing` BEFORE handing COMMIT to the driver, after one final cancellation check — past that point the block deadline can no longer trigger a rollback; post-cutoff resolution: acknowledged → `Committed`; transport failure/lost ack → `CommitUncertain` + detach; definitive rejection → `CommitFailed`.
3. Forced-drop detach: verify the armed `DiscardOnDrop` path detaches when the pipeline future is dropped mid-body (guard drops with `armed == true`); no await possible.
4. Rollback path: cancellation before COMMIT handoff → bounded (5 s) rollback with `RolledBack` on ack.

**Tests:** (scripted executor + tokio pause, unit-tier — the spec's named fixtures)
- `transacted_commit_rejected`: scripted COMMIT returns `23503` → `CommitFailed`, no write persists (executor log shows no ROLLBACK claim), connection state resolved.
- `transacted_commit_deferred_fk`: scripted pre-COMMIT write Ok + COMMIT `23503` → `CommitFailed` naming datasource + constraint.
- `transacted_cancel_before_commit`: cancel token fires before finalization → bounded rollback, `RolledBack`, no partial write.
- `transacted_cancel_after_commit_ack`: cancel fires after commit handed off, ack received → outcome `Committed`, no ROLLBACK issued (executor log empty of ROLLBACK).
- `transacted_cancel_after_commit_lost`: cancel fires after handoff, ack lost (transport failure) → `CommitUncertain`, no rollback claim, detach.
- `transacted_forced_drop_inline`: drop the session future mid-body (guard armed) → connection detached (drop count), pool accepts a later checkout.

**Acceptance:**
- `cargo test -p camel-component-sql` passes
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` exits 0
- `cargo xtask lint-cancel-tokens` exits 0

- [ ] D3

## Phase E: docker tests, docs, lanes

### camel-test

#### Task E1: docker suite — outbox, join proof, concurrency, prior borrower

**Files:**
- `crates/camel-test/tests/sql_test.rs` (modified)
- `crates/camel-test/tests/support/postgres.rs` (modified only if a helper is missing)

**Steps:**
1. Use the existing shared Postgres container (`support/postgres.rs::shared_postgres`) + `setup_test_table`; each scenario creates its own table to avoid cross-test interference.
2. Scenario `sql_transacted_outbox_commit`: block{pg} with UPDATE + INSERT via two joined steps, both succeed → both rows visible (spec: outbox two-write commit).
3. Scenario `sql_transacted_outbox_rollback_on_failure`: same route, second step forced to fail (bad SQL) → neither row persisted, exchange failed (outbox rollback).
4. Scenario `sql_transacted_join_proof_uncommitted_visible`: INSERT then SELECT on the same joined tx → SELECT observes the uncommitted row.
5. Scenario `sql_transacted_join_no_deadlock_maxconn1`: pool `max_connections=1`, two joined steps → route completes within 10 s, BOTH writes persist.
6. Scenario `sql_transacted_prior_borrower_not_committed`: `max_connections=1`; check out a raw pool connection, `BEGIN` + INSERT, return it (abandoned open tx); run the block → block row persists, borrower row ABSENT, same backend PID both phases (`pg_backend_pid()`). Mechanism: a test-owned `AnyPool` registered via `RuntimeDatasourceCatalog` + `SqlComponent::with_config_and_catalog` (precedent: `surrealdb_test.rs:58-91`; catalog path `producer.rs:176-181`) so the test controls checkout/release directly.
7. Scenario `sql_transacted_concurrent_exchanges_distinct_pids`: two concurrent exchanges through the same transacted route, both held at a barrier → distinct `pg_backend_pid()`, both commit.
8. Scenario `sql_transacted_sequential_split_joins`: block{pg} containing a sequential-mode split whose fragments are two joined sql steps → the second fragment observes the first's uncommitted write (runtime join through splits, per the ruling).
9. Scenario `sql_transacted_direct_callee_does_not_inherit`: block{pg} containing `to: direct:callee`; the callee route (loaded in the same context) runs an Auto sql step on pg writing a row → the callee row PERSISTS even though the block later rolls back (callee ran on its own auto-commit connection, never joined).

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test sql_test sql_transacted` passes locally under Docker
- no new `unwrap()` outside tests (`cargo xtask lint-unwrap`)

- [ ] E1

### camel-test

#### Task E2: docker suite — finalization semantics, pool bounds, idempotent dispositions

**Files:**
- `crates/camel-test/tests/sql_test.rs` (modified)

**Steps:**
1. Scenario `sql_transacted_block_timeout_rolls_back`: joined step with `pg_sleep(2)` inside a block with `timeout_secs: 1` → typed Timeout error, no partial write.
2. Scenario `sql_transacted_commit_deferred_fk_commit_failed`: table pair with `DEFERRABLE INITIALLY DEFERRED` FK; block writes the violating child then commits → `CommitFailed` naming datasource + constraint, no row persists.
3. Scenario `sql_transacted_backend_terminated_commit_uncertain`: deferred trigger `pg_terminate_backend(pid)` fires at COMMIT → `CommitUncertain`, connection discarded, pool serves a later exchange on a different backend PID.
4. Scenario `sql_transacted_discard_under_abort_maxconn1`: `max_connections=1`, block aborts after first write → no write persists, next exchange succeeds on a different backend PID.
5. Scenario `sql_transacted_pool_exhausted_named`: `max_connections=1`, one exchange holds the tx (barrier) while a second block waits → typed `PoolExhausted` naming datasource + max_connections + waited (within acquire bound).
6. Scenario `sql_transacted_mixed_shape_batch_rolls_back`: marker write, then joined batch with the cross-shape SQL (NUMERIC target + variable-length IN list; item 0 float `1.5` at param 0, item 1 integer `9007199254740993` at param 0) → `ProcessorError` naming item 1 param 0, marker + batch rows absent.
7. Scenarios `sql_transacted_idempotent_*` (four): eager default `remove_on_failure=true` → rollback removes key, redelivery possible; explicit `false` → reservation retained; failed removal → pending-set recovery (per ADR-0099); `on_missing_key: process` with an ABSENT key → child failure performs NO repository add or remove (key stays absent).

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test sql_test sql_transacted` passes locally under Docker
- each spec runtime scenario of the blessed spec maps to one named test above

- [ ] E2

### docs

#### Task E3: component page + operational note

**Files:**
- `docs/src/components/database.md` (modified)

**Steps:**
1. Add a `## Transacted blocks` section: the `transacted { datasource, timeout_secs, steps }` YAML form + builder form; PostgreSQL-only joins in v1; join semantics (sequential split joins, direct: callees do not inherit); Auto-inside-joins decision; Managed = join required; the load rejections list.
2. Add the operational note: restart on upgrade clears poisoned statement caches; PgBouncer session mode keeps `server_reset_query=DISCARD ALL`; the at-least-once window for `idempotent_consumer` (unique outbox message id + relay/downstream dedupe); pool exhaustion bounded by `acquire_timeout` (default 30 s); block timeout default 30 s.
3. Cross-link ADR 0101; ste-writing prose discipline.

**Tests:**
- `cd docs && mdbook test` passes (link check included)

**Acceptance:**
- `cd docs && mdbook build` exits 0
- section references resolve (mdbook test link pass)

- [ ] E3

### AGENTS.md + gates

#### Task E4: component lanes + local gate sweep

**Files:**
- `AGENTS.md` (modified — `## COMPONENT LANES` table only)

**Steps:**
1. Add lane rows (byte-identical command forms per the mission order):
   - `camel-component-sql | local | cargo test -p camel-component-sql`
   - `camel-api | local | cargo test -p camel-api`
2. Run `bash scripts/gates-local.sh` in the worktree; fix any gate failure attributable to this change (loop back into the owning task); record exit codes for the park report.

**Tests:** (none — infra)

**Acceptance:**
- `bash scripts/gates-local.sh` exits 0
- the lane sweep prints no `NO LANE` line for any crate this change touches

- [ ] E4
