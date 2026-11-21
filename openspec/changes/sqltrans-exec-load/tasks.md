# Tasks: sqltrans-exec-load (S2a)

## Phase A: borrowed-connection executor (own commit, no behaviour change)

### 1.1 DbHandle borrowed-connection surface

- [ ] 1.1

**Files**: `crates/components/camel-sql/src/producer.rs`, `crates/components/camel-sql/src/lib.rs`

**Steps**:
1. Add `pub(crate) struct DbHandle<'a> { conn: &'a mut AnyConnection }` with
   `fn lend(&mut self) -> &mut AnyConnection`.
2. Refactor the four execute fns (`fetch_single_rows`, `execute_select`,
   `execute_modify`, `execute_batch`) to take `DbHandle<'_>` instead of
   `&AnyPool`.
3. Adapt the producer `call` site: acquire from the pool exactly as today,
   lend the handle for the statement run, release on error via the existing
   `release_after_error` path.
4. Preserve unchanged: standalone batch keeps its own pool transaction;
   StreamList keeps deferred pooled acquisition.

**Tests** (executable):
- name: `existing_suites_stay_green` — arrange: run the existing in-file
  producer suites unmodified (`cargo test -p camel-component-sql`); act: all
  146+ existing tests run; assert: zero failures, zero test edits.
- name: `dbhandle_lends_one_mutable_borrow` — arrange: build a `DbHandle`
  over a test connection; act: take `lend()` twice sequentially (not
  concurrently — compile-level); assert: compiles, statement via `lend()`
  executes.

**Acceptance**: `- [x] 1.1` when the refactor compiles, the full
camel-component-sql suite passes with NO test modifications, and
`plan_binds`' signature is untouched.

### 1.2 Shared batch-planner entry point (req 12 planner clause)

- [ ] 1.2

**Files**: `crates/components/camel-sql/src/bind_kinds.rs`, `crates/components/camel-sql/src/producer.rs`

**Steps**:
1. Add `pub(crate) async fn plan_batch_binds(conn: &mut AnyConnection, sql:
   &str, batch: &[&[serde_json::Value]]) -> Result<BatchBindPlan, CamelError>`
   that scans ALL batch shapes together in one entry point.
2. Rewrite `execute_batch` to call `plan_batch_binds` instead of per-shape
   calls into the single-shape planner.
3. Keep `sql-binds` whole-batch rules and `plan_binds`' signature unchanged.

**Tests** (executable):
- name: `batch_planner_accepts_all_shapes_together` — arrange: a batch with
  three distinct bind shapes (float, integer, string) against the test
  connection; act: `plan_batch_binds`; assert: one plan covering all shapes,
  no per-shape planner calls (existing per-shape helper not invoked —
  compile/refactor evidence plus plan assertion).
- name: `existing_bind_rules_unchanged` — arrange: run the existing
  `bind_kinds` suite (31 tests) and the sql-binds rule tests unmodified; act:
  full run; assert: zero failures, zero edits.

**Acceptance**: `- [x] 1.2` when both tests pass and `execute_batch` routes
through `plan_batch_binds`.

## Phase B: load rejections + walk + registration

### 2.1 BuilderStep structural walk at both fan-ins (rc-c3mjp)

- [ ] 2.1

**Files**: `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs`,
`crates/camel-core/src/lifecycle/application/commands.rs`,
`crates/camel-core/src/transacted_walk.rs` (new)

**Steps**:
1. Add `pub(crate) fn validate_transacted_block_builder(step:
   &BuilderStep, resolve_manager: &dyn Fn(&str) ->
   Option<Arc<dyn TransactionManager>>) -> Result<(), CamelError>` walking
   the `BuilderStep` tree: the S1 structural set (parallel split, multicast,
   wiretap, async handoff, seda escape reaching a joined sql step;
   StreamList inside a block; foreign/unnamed datasource; nested block) plus
   the req-5 join-side SQL guards (static-SQL-only incl. `file:` →
   `RuntimeSqlInJoinedStep`; BEGIN/COMMIT/ROLLBACK/SAVEPOINT +
   implicit-commit text → `TransactionControlSql`; dialect from URL scheme →
   `DialectUnprovable`/`UnsupportedJoinDialect`) plus dynamic-target
   rejection (`DynamicTargetUnprovable` for `RecipientList`/`RoutingSlip`
   in-block).
2. Wire it into the `BuilderStep::Transacted` arm of
   `StepCompilerRegistry::compile_step`, replacing the `ManagerMissing`
   backstop (keep the backstop behavior for `ManagerMissing` after the walk).
3. Run the same walk in the canonical arm
   (`canonical_step_to_builder_step`) before child lowering.
4. Keep the manager resolution fail-closed (`ManagerMissing` names the
   datasource).

**Tests** (executable, unit-tier, both authoring paths — declarative fixtures
already exist from S1; add the BuilderStep/canonical twins):
- name: `builder_walk_rejects_parallel_split_reaching_joined_sql` — arrange:
  programmatic `BuilderStep::Transacted` containing a parallel split whose
  fragments reach a sql step on the block datasource; act: compile through
  `StepCompilerRegistry`; assert: `TransactionError::ParallelInBlock` naming
  step + datasource.
- name: `canonical_arm_rejects_transacted_with_runtime_sql_source` —
  arrange: canonical `Transacted` step whose joined sql step uses a
  header-provided statement; act: `canonical_step_to_builder_step`; assert:
  `TransactionError::RuntimeSqlInJoinedStep` naming the step.
- name: `builder_walk_rejects_file_sql_source` — arrange: joined step with
  `file:` source; act: compile; assert: `RuntimeSqlInJoinedStep` naming the
  step.
- name: `builder_walk_rejects_transaction_control_sql` — arrange: joined
  step whose statement text contains `COMMIT`; act: compile; assert:
  `TransactionControlSql` naming step + statement.
- name: `builder_walk_rejects_unprovable_dialect` — arrange: datasource URL
  `opaque://host/db` with a registered manager (fake); act: compile; assert:
  `DialectUnprovable` naming `opaque`.
- name: `builder_walk_rejects_mysql_and_sqlite_joins` — arrange: `mysql://`
  and `sqlite://` datasources with registered manager; act: compile; assert:
  `UnsupportedJoinDialect` naming the dialect, standalone steps outside
  blocks still load.
- name: `builder_walk_rejects_dynamic_targets_in_block` — arrange: block
  containing `RecipientList` and `RoutingSlip` steps; act: compile; assert:
  `DynamicTargetUnprovable` naming the step.
- name: `manager_missing_still_fails_closed_after_walk` — arrange: block on
  an unregistered datasource; act: compile; assert:
  `TransactionError::ManagerMissing` naming the datasource.

**Acceptance**: `- [x] 2.1` when every fixture passes on the canonical/
programmatic path and the existing DSL-path fixtures still pass.

### 2.2 RuntimeBus registration entry point + bundle/CLI registration

- [ ] 2.2

**Files**: `crates/camel-core/src/lifecycle/application/runtime_bus.rs`,
`crates/camel-bundles/src/lib.rs`

**Steps**:
1. Add `RuntimeBus::register_transaction_manager(name, Arc<dyn
   TransactionManager>) -> Result<(), RegistryError>` (duplicate-rejecting,
   delegating to the `NamedRegistry`).
2. In bundle boot, after the datasource catalog is built, register a
   `SqlTransactionManager` per catalog datasource (constructor from
   camel-sql; fail-loud on duplicate).
3. Register through the same entry point on the camel-cli assembly path.
4. Keep routes failing closed with `ManagerMissing` when nothing registers.

**Tests** (executable):
- name: `runtime_bus_registers_and_resolves_manager` — arrange: `RuntimeBus`
  + fake manager for `pg`; act: `register_transaction_manager("pg", m)`,
  then `resolve_transaction_manager(deps.transaction_registry, "pg")`;
  assert: resolves to the same Arc; duplicate registration errors.
- name: `unregistered_datasource_still_fails_closed` — arrange: registry
  without `audit`; act: resolve; assert: `ManagerMissing` naming `audit`.
- name: `bundle_boot_registers_catalog_datasources` — arrange: bundle config
  with datasources `pg` + `audit`; act: boot; assert: both resolvable via the
  bus registry.

**Acceptance**: `- [x] 2.2` when all three tests pass and bundle boot
registers every catalog datasource before routes load.

### 2.3 DEFERRED_VARIANTS move (vocabulary decision)

- [ ] 2.3

**Files**: `crates/camel-dsl/src/compile.rs`

**Steps**:
1. Add `"Transaction"` to `supported_exception_kinds()` with a match arm in
   `exception_kind_matches(kind, err)` over `CamelError::Transaction(_)`
   (any `TransactionError` variant matches the kind).
2. Remove `"Transaction"` from `DEFERRED_VARIANTS`.
3. Update the rationale comment (runtime raise path lands with S2b; the kind
   is now decided).

**Tests** (executable):
- name: `transaction_kind_is_supported_and_matches` — arrange:
  `CamelError::Transaction(TransactionError::CommitFailed { .. })`; act:
  `exception_kind_matches("Transaction", &err)`; assert: true; and
  `supported_exception_kinds()` contains `"Transaction"`.
- name: `vocabulary_guard_asserts_moved_bucket` — arrange: run the existing
  `test_exception_kind_vocabulary_classification_guard`; act: full
  classification sweep; assert: every variant in exactly one bucket with
  `Transaction` in the supported set and absent from `DEFERRED_VARIANTS`.

**Acceptance**: `- [x] 2.3` when both tests pass.

### 2.4 ADR amendment line + dynamic-target fixture cross-ref

- [ ] 2.4

**Files**: `docs/adr/0101-transacted-block-surface.md`

**Steps**:
1. Add one amendment line (Accepted): in-block dynamic routing targets
   (`RecipientList`/`RoutingSlip`) are REJECTED at load
   (`DynamicTargetUnprovable`) — membership cannot be proven statically.

**Tests** (executable):
- name: `adr_amendment_names_the_rejection` — arrange: read the ADR file in
  a docs-lint style test or assert in the 2.1 fixture comment; act: grep for
  `DynamicTargetUnprovable`; assert: present in the amendment section.

**Acceptance**: `- [x] 2.4` when the amendment line exists and the 2.1
dynamic-target fixture references it.
