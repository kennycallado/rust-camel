# Tasks: sqltrans-exec (code-first SPIKE)

Nine steps, ONE commit each (review-fix commits may follow a step; the step
boundary is the step's own commit), each green before the next. Workers: w_high for
steps 1/3/6/7, w_fast for the rest; r_glm review after every step; at most 2
worker sessions per step. No expert dispatch during implementation; the single
conformance gate comes after step 9.

## 1. Typed error propagation (fixes R3-1)

- [x] 1

**Files**: `crates/camel-core/src/lifecycle/application/commands.rs`

**Steps**:
1. In `canonical_to_route_definition`, remove the
   `.map_err(|e| CamelError::Config(format!("circuit_breaker fallback: {e}")))`
   on the fallback `canonical_steps_to_builder_steps` call; keep a context
   wrapper that does NOT erase the typed variant (preserve the
   `CamelError::Transaction(..)` variant while naming the fallback site — e.g.
   prefix only the Display message or re-raise the original error).
2. Keep the non-fallback canonical lowering path unchanged.

**Tests** (executable, unit-tier):
- name: `cb_fallback_preserves_typed_transaction_error` — arrange: a canonical
  route with a `circuit_breaker` whose `fallback` contains a transacted block
  on a datasource with NO registered manager; act:
  `canonical_to_route_definition`; assert: the returned error is
  `CamelError::Transaction(TransactionError::ManagerMissing { .. })` (exact
  variant), NOT `CamelError::Config`.

**Acceptance**: `- [x] 1` when the test passes, `cargo test -p camel-core`
is green, and no other canonical lowering behaviour changed.

## 2. Phase A — DbHandle + plan_batch_binds (req-12 planner clause)

- [x] 2

**Files**: `crates/components/camel-sql/src/producer.rs`,
`crates/components/camel-sql/src/bind_kinds.rs`

**Steps**:
1. Add `pub(crate) struct DbHandle<'a> { conn: &'a mut AnyConnection }` with
   `fn lend(&mut self) -> &mut AnyConnection`.
2. Refactor the four execute fns (`fetch_single_rows`, `execute_select`,
   `execute_modify`, `execute_batch`) to take `DbHandle<'_>` instead of
   `&AnyPool`.
3. Adapt the producer `call` site: acquire from the pool exactly as today,
   lend the handle for the statement run, release on error via the existing
   `release_after_error` path. Standalone batch keeps its own pool
   transaction; StreamList keeps deferred pooled acquisition.
4. Add `pub(crate) async fn plan_batch_binds(tx, items:
   &[(String, Vec<serde_json::Value>)]) -> Result<BatchPlan, CamelError>`:
   per-item SQL texts plus bindings (IN-clause expansion yields distinct
   texts); internally performs today's sequence — group texts by shape
   (`group_rows_by_sql`), message-wide no-loosening scan (`scan_bind_targets`),
   probe each distinct shape once (`probe_batch_shapes`), per-item planning
   (`plan_batch_items`), then per item in order: probe-kind lookup,
   `readback_kinds`, `named_kinds_agree` — the FIRST missing/unusable/
   disagreeing read-back flips the WHOLE batch to aliases (short-circuit).
5. Return the FINAL decision as `enum BatchPlan { Aliased { targets:
   Vec<Vec<Option<BindTarget>>> }, Named { per_item_targets:
   Vec<Vec<Option<BindTarget>>> } }` — Aliased = today's whole-batch fallback
   with PER-ITEM targets (each item keeps its own `alias_targets` — described
   guards hybridized with the message-wide scan — so a shared single vector
   that would drop the 2^53 described guard and loosen bd rc-0eiqm is
   forbidden), Named = per-item read-back-agreed targets.
6. Rewrite `execute_batch` to resolve items then delegate ALL planning and
   the whole-batch/read-back decision to `plan_batch_binds`. Keep `sql-binds`
   whole-batch rules and `plan_binds`' signature unchanged.

**Tests** (executable, unit-tier):
- name: `dbhandle_lends_one_mutable_borrow` — arrange: a `DbHandle` over a
  test connection; act: two sequential `lend()` calls; assert: compiles,
  statement via `lend()` executes.
- name: `batch_planner_accepts_distinct_expansions_together` — arrange: a
  batch whose items expand `in (:list)` to two distinct placeholder counts
  plus a third item of a different bind shape; act: `plan_batch_binds`;
  assert: one `BatchPlan` covering every item, per-shape metadata tracked per
  distinct text, whole-batch decision computed before item 0.
- name: `unusable_later_probe_still_sends_whole_batch_to_aliases` — arrange:
  first shape probes usable, a later distinct shape unusable; act: plan;
  assert: `BatchPlan::Aliased` with the message-wide scan targets.
- name: `existing_suites_stay_green` — act: run the existing producer and
  `bind_kinds` suites unmodified (`cargo test -p camel-component-sql`);
  assert: zero failures, zero test edits.

**Acceptance**: `- [x] 2` when the refactor compiles, the full
camel-component-sql suite passes with NO test modifications, `plan_binds`'s
signature is untouched, and `execute_batch` routes through `plan_batch_binds`.

## 3. LOAD walk — ONE recursive visitor at BOTH fan-ins (settles R3-2)

- [x] 3

**Files**: `crates/camel-core/src/transacted_walk.rs` (new),
`crates/camel-api/src/transaction.rs` (add `DynamicTargetUnprovable { step }`),
`crates/camel-dsl/src/compile.rs`, `crates/camel-dsl/src/transacted_validation.rs`,
`crates/camel-core/src/lifecycle/application/commands.rs`,
`crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs`

**Steps**:
1. Add the camel-api variant `DynamicTargetUnprovable { step }` (non_exhaustive
   enum growth; Display names the step).
2. Write ONE recursive visitor over `BuilderStep` in camel-core
   (`transacted_walk.rs`) that enforces, depth-first in document order, at
   each node: the structural checks (S1 set: parallel split/multicast/wiretap/
   async/seda reach, StreamList, foreign/unnamed datasource, nested block)
   BEFORE the per-arm checks (static-SQL-only incl. `file:` →
   `RuntimeSqlInJoinedStep`; BEGIN/COMMIT/ROLLBACK/SAVEPOINT + implicit-commit
   text → `TransactionControlSql`; dynamic targets → `DynamicTargetUnprovable`)
   BEFORE children recurse; first error wins.
3. Call the SAME visitor from BOTH fan-ins — the declarative lowering
   (`DeclarativeStep::Transacted` arm in camel-dsl `compile.rs`) and the
   canonical lowering (`CanonicalStepSpec::Transacted` arm in camel-core
   `commands.rs` and the camel-dsl canonical arm) — never a second pass.
   Manager resolution (`ManagerMissing`) runs after the walk; dialect from
   URL scheme (`DialectUnprovable`/`UnsupportedJoinDialect`) after manager
   resolution (URL via a `resolve_url` seam; production seams keep the
   always-absent resolvers so outcomes stay byte-identical today).
4. Move `"Transaction"` out of `DEFERRED_VARIANTS` in camel-dsl `compile.rs`:
   add `"Transaction"` to `supported_exception_kinds()` with a
   `exception_kind_matches` arm over `CamelError::Transaction(_)`; update the
   rationale comment.
5. `Managed` stays rejected at load — no change to
   `SqlEndpointConfig::validate_transaction_mode` or the S1 Managed suites.

**Tests** (executable, unit-tier; NESTED fixture table — transacted inside
choice/split/fallback/loop asserting the exact variant for each req-5
rejection):
- name: `walk_rejects_transaction_control_sql_nested` — arrange: block nested
  in choice/split/fallback/loop whose joined sql step contains `COMMIT`;
  act: load; assert: `TransactionControlSql` naming step + statement.
- name: `walk_rejects_runtime_sql_in_joined_step_nested` — arrange: joined
  step with a header-provided statement (and a `file:` source twin); act:
  load; assert: `RuntimeSqlInJoinedStep` naming the step.
- name: `walk_rejects_dialect_unprovable` — arrange: datasource URL
  `opaque://host/db` with fake manager + URL resolvers through the seam; act:
  load; assert: `DialectUnprovable` naming `opaque`.
- name: `walk_rejects_unsupported_join_dialect_mysql_and_sqlite` — arrange:
  `mysql://` and `sqlite://` datasources with registered fake manager; act:
  load; assert: `UnsupportedJoinDialect` naming the dialect; standalone steps
  outside blocks still load.
- name: `walk_rejects_dynamic_targets_in_block` — arrange: block containing
  dynamic-routing steps; act: load; assert: `DynamicTargetUnprovable` naming
  the step.
- name: `managed_stays_rejected_at_load` — arrange: the existing S1 Managed
  rejection fixtures; act: full camel-component-sql + camel-dsl suites
  unmodified; assert: `Managed` still rejected at load (regression: the spike
  must not open S3's path).
- name: `transaction_kind_is_supported_and_matches` — arrange:
  `CamelError::Transaction(TransactionError::CommitFailed { .. })`; act:
  `exception_kind_matches("Transaction", &err)`; assert: true;
  `supported_exception_kinds()` contains `"Transaction"` and
  `DEFERRED_VARIANTS` does not.

**Acceptance**: `- [x] 3` when every fixture asserts the exact variant and
offender fields, all S1 declarative fixtures pass unmodified, and both
fan-ins route through the ONE visitor.

## 4. Registry — RuntimeBus entry point (test-exercised only)

- [x] 4

**Files**: `crates/camel-core/src/lifecycle/application/runtime_bus.rs`,
`crates/camel-bundles/src/lib.rs` (registry-empty test only)

**Steps**:
1. Add `RuntimeBus::register_transaction_manager(name, Arc<dyn
   TransactionManager>) -> Result<(), RegistryError>` (duplicate-rejecting,
   delegating to the `NamedRegistry`). NO production caller registers yet.
2. Add a TEST-ONLY fake `TransactionManager` in camel-core beside the bus
   tests. Bundle and CLI register NOTHING: boot leaves the transaction
   registry EMPTY.

**Tests** (executable):
- name: `runtime_bus_registers_and_resolves_manager` — arrange: `RuntimeBus`
  + the test-only fake manager for `pg`; act: `register_transaction_manager`,
  then resolve via `resolve_transaction_manager`; assert: resolves to the
  same Arc; duplicate registration errors.
- name: `unregistered_datasource_still_fails_closed` — arrange: registry
  without `audit`; act: resolve; assert: `ManagerMissing` naming `audit`.
- name: `bundle_boot_leaves_transaction_registry_empty` — arrange: bundle
  config with datasources `pg` + `audit`; act: boot; assert: the transaction
  registry has ZERO keys (`is_empty()`/`len()==0`), not two specific misses.

**Acceptance**: `- [x] 4` when all three tests pass, boot leaves the registry
key-empty, and no production path registers a manager.

## 5. Port delta — Any supertrait + join module + docs-only contract

- [x] 5

**Files**: `crates/camel-api/src/transaction.rs`,
`crates/camel-api/src/transaction/join.rs` (new module)

**Steps**:
1. Add `Any` supertrait: `pub trait TransactionSession: Any + Send + Sync +
   'static` (trait already requires `'static`; no method changes; no
   implementor burden — `Any` is auto-implemented).
2. Add the `transaction::join` module: `task_local!` channel `BLOCK_SESSION`
   (per-block `{ datasource, scope: u64, session: Arc<dyn TransactionSession> }`)
   and `JOIN_SCOPE` (`u64` baked per joined sql step), plus `JoinScope`
   newtype helpers. No sqlx types in camel-api.
3. Paste the 604d docs-only `rollback()` contract VERBATIM onto
   `TransactionSession::rollback`, and add to `commit`: "Precondition
   `Active`. Otherwise no SQL is sent and the call returns
   `Err(SessionAborted)`. The owner SHALL call `commit` only when
   `state() == Active`." No `TransactionState` variants.

**Tests** (executable):
- name: `session_port_upcasts_to_any_and_downcasts` — arrange: a test
  `Arc<dyn TransactionSession>` over a concrete test session; act: upcast to
  `Arc<dyn Any + Send + Sync>` and `Arc::downcast`; assert: the concrete type
  round-trips.
- name: `join_task_local_scopes_isolate` — arrange: two nested
  `BLOCK_SESSION.scope` values; act: read `JOIN_SCOPE`/`BLOCK_SESSION` inside
  each scope; assert: inner scope shadows within its task only.
- name: `rollback_doc_carries_total_contract` — arrange: read the source;
  act: assert the rustdoc carries the four 604d contract bullets verbatim
  (doc-text assertion); assert: `commit` doc names the `Active` precondition
  and `SessionAborted`.

**Acceptance**: `- [x] 5` when the port compiles workspace-wide, no
`TransactionState` variant changed, and the docs-only contract is verbatim.

## 6. SqlTxSession — private TxPhase, total rollback, fixtures

- [x] 6

**Files**: `crates/components/camel-sql/src/session.rs` (new),
`crates/components/camel-sql/src/lib.rs`, `pool_factory.rs` (pool access)

**Steps**:
1. Implement `SqlTxSession` (`impl TransactionSession`, the
   `Arc::downcast` target): owns the guarded pooled connection (sole owner),
   one admission slot (`tokio::sync::Mutex<()>`; `try_lock` → `Busy`),
   `rollback_only` as the port's `AtomicBool`. BEGIN/COMMIT/ROLLBACK issued
   as plain statements on the borrowed connection — no sqlx `Transaction`
   wrapper stored.
2. Private `TxPhase` (Clean/BeginPending/Open/CommitInFlight/RollbackInFlight/
   Done/Detached) projected onto the frozen enum exactly per the 604c §A
   table. Guard armed from checkout until `Done` or the R2 clean return;
   `Detached` terminal.
3. `begin()` per 604c §B verbatim (precondition Clean; `BeginPending` set
   before the write; ACK → Open; own timeout → detach + `PoolExhausted`;
   driver error → detach + `RollbackUncertain`; NO cleanup SQL; dropped
   future stays `BeginPending`).
4. Total `rollback()` per the 604d contract (Clean → R2 no SQL;
   BeginPending → R3 detach no SQL + `Err(RollbackUncertain)`; Open → ROLLBACK
   within the finalize grace; Detached → R1 no SQL
   `Err(RollbackUncertain)`; defensive `Finalizing`/`Done` entry →
   `Err(RollbackUncertain)` with an `error!` log and NO detach claim).
5. `commit()` requires `Open` else `Err(SessionAborted)`, no SQL; R7
   classifier: `Database` ∧ severity exactly `Error` ∧ SQLSTATE class 23|40 →
   `CommitFailed` + one cleanup ROLLBACK within remaining grace; everything
   else (FATAL/PANIC incl. 57P01, Io, Protocol, PoolClosed, grace elapsed) →
   `CommitUncertain`, detach immediately, no ROLLBACK, no rollback claim.
6. One absolute finalize deadline `finalize_start + 5 s` enforced INSIDE
   `commit()`/`rollback()` (injected timer/clock seam for virtual-clock
   fixtures).

**Tests** (executable, unit-tier virtual clock):
- name: `transacted_begin_pending_deadline` — arrange: session in
  `BeginPending` (BEGIN write left pending via the injected clock); act:
  `rollback()`; assert: detach, no SQL, `Err(RollbackUncertain)` (R3 via the
  total rollback).
- name: `transacted_commit_classifier_table` — arrange: fake driver errors
  severity `Error` class `23` and class `40`, plus `57P01` (Fatal), `57014`,
  `08006`, and an Io error at COMMIT; act: `commit()` from `Open`; assert:
  class 23/40 → `CommitFailed`; 57P01/57014/08006/Io → `CommitUncertain` +
  detach, no ROLLBACK, no rollback claim.

**Acceptance**: `- [x] 6` when both fixtures pass, the phase projection
matches the 604c table row for row, and the defensive entry logs `error!`
without claiming detachment.

## 7. TransactedSegment — 604d pseudocode literally

- [x] 7

**Files**: `crates/camel-core/src/transacted_segment.rs` (new)

**Steps**:
1. Implement the segment runtime: compute `block_deadline` at block entry
   (default 30 s, per-block override); open the session (checkout + hygiene
   race `min(block_abs, acquire_abs)`; guard arms at checkout; expiry drops
   the open future → R0; primary `PoolExhausted` if acquire-first else
   `Timeout`, tie included; unacknowledged hygiene →
   `Err(RollbackUncertain)`); set `BLOCK_SESSION` for the body task; body
   race `biased select { sleep_until(block_deadline), CANCEL_TOKEN.cancelled(),
   body }` (tie → deadline wins → `Timeout`).
2. Finalization = the 604d pseudocode LITERALLY (precedence in the segment;
   R7 gate on `state() == Active` with the cutoff check; else total
   `rollback()`; the four `(primary, r)` outcome rows; a cleanup result never
   replaces an existing primary).

**Tests** (executable, fake session):
- name: `transacted_fresh_rollback_precedence` — arrange: fake session
  returning Ok/Err from `rollback()` under Deadline, Err, and Ok bodies, plus
  the NO-PRIMARY row: body Ok + a step-local drop leaving `BeginPending` +
  clear flag; act: run the segment; assert: primary error and cleanup context
  per the pseudocode table — the no-primary row reports
  `Err(RollbackUncertain)`.

**Acceptance**: `- [x] 7` when the fixture passes including the no-primary
row and the pseudocode is recognizable line-for-line in the source.

## 8. PG suite — P1–P6 as the frozen fixture names

- [x] 8

**Files**: `crates/camel-test/tests/sql_test.rs` (or the change's PG suite
file per the existing lane), `crates/components/camel-sql` (SQLTRANS_PG_URL
override), `AGENTS.md` lane row if a new suite file is added

**Steps**:
1. Turn probes P1–P6 into the frozen fixture names: `transacted_commit_deferred_fk`
   (23503 → `CommitFailed`), backend-kill `CommitUncertain` + pool
   replacement (57P01), cancel-before/after cutoff, prior-borrower discard +
   poisoning regression (hygiene mandatory), pool exhaustion typed naming,
   busy-session determinism. Keep ALL frozen fixture names from the family
   spec.
2. Add the `SQLTRANS_PG_URL` override: when set, the suite runs the same test
   bodies against that URL (a `--network host` container locally) instead of
   testcontainers mapped ports; the testcontainers mapped-port path stays the
   CI-owned default, unmodified.

**Tests** (executable): the frozen fixtures above, gated on PG availability
(`SQLTRANS_PG_URL` locally; the docker lane in CI).

**Acceptance**: `- [x] 8` when the suite compiles, the local `--network host`
run's exact command and result are recorded in the park report, and the
default testcontainers path is untouched.

## 9. Bundle + CLI registration (LAST commit, separately revertible)

- [x] 9

**Files**: `crates/camel-bundles/src/lib.rs`, `crates/camel-cli` (assembly
path)

**Steps**:
1. Bundle and CLI register ONE `SqlTransactionManager` per catalog datasource
   at boot.
2. Replace `bundle_boot_leaves_transaction_registry_empty` with
   `bundle_boot_registers_catalog_datasources`.
3. Steps 1–8 stay dark: no other behaviour change in this commit.

**Tests** (executable):
- name: `bundle_boot_registers_catalog_datasources` — arrange: bundle config
  with datasources `pg` + `audit`; act: boot; assert: the transaction
  registry resolves exactly those datasources to
  `SqlTransactionManager` instances.

**Acceptance**: `- [x] 9` when the guard replacement passes, the registry
resolves every catalog datasource, and the commit is independently revertible
without breaking steps 1–8.
