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
1. Add `pub(crate) async fn plan_batch_binds(tx, items:
   &[(String, Vec<serde_json::Value>)]) -> Result<BatchPlan, CamelError>` —
   `items` carries each batch item's OWN resolved SQL text plus bindings
   (IN-clause expansion yields distinct texts; one shared `sql: &str`
   cannot represent them — bless round 1, probe Q3). The fn performs
   exactly today's sequence internally AND owns the named read-back
   agreement loop that today lives after `plan_batch_items` (probe R2-2:
   producer.rs:781–813): group texts by shape (`group_rows_by_sql`),
   message-wide no-loosening scan (`scan_bind_targets`), probe each
   distinct shape once on the caller's in-flight batch transaction
   (`probe_batch_shapes`), per-item planning (`plan_batch_items`), then per
   item in order: probe-kind lookup, `readback_kinds` on the same
   transaction, `named_kinds_agree` — the FIRST missing/unusable/
   disagreeing read-back flips the WHOLE batch to aliases (short-circuit).
2. Return the FINAL decision as
   `enum BatchPlan { Aliased { targets: Vec<Option<BindTarget>> },
   Named { per_item_targets: Vec<Vec<Option<BindTarget>>> } }` — `Aliased`
   = today's whole-batch fallback (message-wide scan targets on the
   uncached alias text), `Named` = per-item read-back-agreed targets.
3. Rewrite `execute_batch` to resolve items then delegate ALL planning and
   the whole-batch/read-back decision to `plan_batch_binds`; execute per
   the returned `BatchPlan`.
4. Keep `sql-binds` whole-batch rules and `plan_binds`' signature unchanged.

**Tests** (executable):
- name: `batch_planner_accepts_distinct_expansions_together` — arrange: a
  batch whose items expand `in (:list)` to TWO distinct placeholder counts
  (two distinct SQL texts) plus a third item of a different bind shape,
  against the test connection; act: `plan_batch_binds`; assert: one
  `BatchPlan` covering every item, per-shape metadata tracked per distinct
  text (not per batch), whole-batch decision computed before item 0.
- name: `unusable_later_probe_still_sends_whole_batch_to_aliases` — arrange:
  first shape probes usable, a later distinct shape unusable; act: plan;
  assert: `BatchPlan::Aliased` with the message-wide scan targets.
- name: `readback_disagreement_flips_whole_batch` — arrange: at the pure
  decision layer, a described probe whose read-back kinds disagree
  (extend the existing `decide_path_aliases_on_disagreement` coverage
  through the planner contract; bd rc-0eiqm path); act: plan; assert:
  alias path chosen — the disagreement flip survives the refactor.
- name: `existing_bind_rules_unchanged` — arrange: run the existing
  `bind_kinds` suite (31 tests) and the sql-binds rule tests unmodified; act:
  full run; assert: zero failures, zero edits.

**Acceptance**: `- [x] 1.2` when all four tests pass and `execute_batch`
routes through `plan_batch_binds` with `plan_binds` untouched.

## Phase B: load rejections + walk + registration

### 2.1 Shared walk fn + DSL declarative/canonical extension (declarative fan-in)

- [ ] 2.1

**Files**: `crates/camel-core/src/transacted_walk.rs` (new — the shared walk
fn), `crates/camel-dsl/src/transacted_validation.rs` (declarative fan-in:
req-5 guards + dynamic targets + `resolve_url` seam),
`crates/camel-dsl/src/compile.rs` (canonical fan-in: `compile_canonical_step`
`CanonicalStepSpec::Transacted` arm walks before lowering; resolver seams),
`crates/camel-api/src/transaction.rs` (additive
`DynamicTargetUnprovable { step }` variant — non_exhaustive growth, no port
change)

**Steps**:
1. Add the camel-api variant `DynamicTargetUnprovable { step }` (doc: in-block
   runtime routing target, req 5, ruling REJECTS at load).
2. Add `pub(crate) fn validate_transacted_block_builder(step: &BuilderStep,
   resolve_manager: &dyn Fn(&str) -> Option<Arc<dyn TransactionManager>>,
   resolve_url: &dyn Fn(&str) -> Option<String>) -> Result<(), CamelError>`
   in camel-core `transacted_walk.rs`, enforcing the design's precedence
   chain (1)–(5): S1 structural set → dynamic targets → req-5 SQL guards
   (static-SQL-only incl. `file:` → `RuntimeSqlInJoinedStep`; BEGIN/COMMIT/
   ROLLBACK/SAVEPOINT + implicit-commit text → `TransactionControlSql`) →
   manager resolution (`ManagerMissing`) → dialect from URL scheme
   (`DialectUnprovable`/`UnsupportedJoinDialect`). Manager resolution BEFORE
   dialect checks keeps the DSL production seams (always-absent resolver) on
   S1's `ManagerMissing` contract.
3. Extend the DSL declarative walk (`validate_transacted_blocks`) with the
   same guards (2)–(3) and the `resolve_url` parameter; production parse
   seams keep the ALWAYS-ABSENT manager resolver and the ALWAYS-ABSENT URL
   resolver (probe R2-3: parse seams receive raw text and no `CamelConfig`
   — yaml.rs:110/218/286, json.rs:29; with nothing registered, dialect is
   unreachable there anyway, and `ManagerMissing` keeps the S1 contract).
   The resolver parameters exist for the parameterized test seam.
4. Wire the same walk into the camel-dsl canonical arm
   (`compile_canonical_step` `Transacted`) before child lowering, via the
   same always-absent-in-production resolver parameters (the dark contract
   holds at both DSL seams; production canonical-DSL blocks fail
   `ManagerMissing` at the DSL seam).

**Tests** (executable, unit-tier, DECLARATIVE fan-in — S1 structural fixtures
in `crates/camel-dsl/tests/transacted_load_rejections.rs` stay green
unmodified; add the req-5 declarative twins):
- name: `runtime_sql_source_in_joined_step_rejected` — arrange: declarative
  block whose joined sql step takes its statement from a header; act:
  `compile_declarative_route_with`; assert: `RuntimeSqlInJoinedStep` naming
  the step.
- name: `file_sql_source_in_joined_step_rejected` — arrange: joined step with
  `file:` source; act: load; assert: `RuntimeSqlInJoinedStep` naming the step.
- name: `transaction_control_sql_in_joined_step_rejected` — arrange: joined
  step whose statement text contains `COMMIT`; act: load; assert:
  `TransactionControlSql` naming step + statement.
- name: `dialect_unprovable_rejected` — arrange: datasource URL
  `opaque://host/db`, fake manager + URL resolvers passed through the seam;
  act: load; assert: `DialectUnprovable` naming `opaque`.
- name: `unsupported_join_dialect_mysql_and_sqlite_rejected` — arrange:
  `mysql://` and `sqlite://` datasources with fake resolvers; act: load;
  assert: `UnsupportedJoinDialect` naming the dialect; a standalone sql step
  outside any block on the same datasource still loads.
- name: `dynamic_targets_in_block_rejected` — arrange: block containing
  `RecipientList` and `RoutingSlip` steps; act: load; assert:
  `DynamicTargetUnprovable` naming the step.
- name: `unregistered_datasource_still_manager_missing` — arrange: clean-SQL
  block, always-absent resolver (S1 fixture
  `unregistered_datasource_fails_at_load` covers it); act: load; assert:
  still `ManagerMissing` (precedence: manager before dialect).

**Acceptance**: `- [x] 2.1` when every new declarative fixture asserts the
exact variant and offender fields, and all S1 fixtures pass unmodified.

### 2.2 camel-core wiring: walk at compile_step + commands.rs (BuilderStep fan-in, dark gate)

- [ ] 2.2

**Files**: `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs`,
`crates/camel-core/src/lifecycle/application/commands.rs`,
`crates/camel-core/src/lifecycle/application/commands_tests.rs` (if the
canonical fixtures live there)

**Steps**:
1. Add optional registry/catalog access to `CompilationContext`
   (`transaction_registry: Option<&TransactionManagerRegistry>`,
   `datasource_catalog: Option<&dyn DatasourceCatalog>`); absent ⇒
   always-absent closures ⇒ `ManagerMissing` — outcomes byte-identical to
   today's backstop for every existing caller.
2. Rewrite the `CanonicalStepSpec::Transacted` arm of
   `canonical_step_to_builder_step` as THE CANONICAL REJECTION BOUNDARY
   (probe R2-1): walk → manager resolution → DARK GATE, with NO success
   path in S2a — the arm never emits `BuilderStep::Transacted`. It runs
   inside `handle_register` BEFORE `execution.register_route`, so
   rejections land in the `?` propagation zone: no compensation, no Failed
   marking, the runtime port never sees the route (obligation 4 intact).
3. Replace the `BuilderStep::Transacted` blanket backstop in
   `StepCompilerRegistry::compile_step` with the same walk → resolution →
   dark-gate sequence (shared backstop for programmatic/DSL-lowered
   BuilderSteps; for canonical routes unreachable in S2a since the early
   gate errors first; fires-inside-compensation property documented).
4. Thread the optional datasource catalog through `CommandDeps` (same
   pattern as S1's `transaction_registry`). Keep manager resolution
   fail-closed everywhere.

**Tests** (executable, unit-tier, BUILDERSTEP/CANONICAL fan-in):
- name: `builder_walk_rejects_parallel_split_reaching_joined_sql` — arrange:
  programmatic `BuilderStep::Transacted` containing a parallel split whose
  fragments reach a sql step on the block datasource; act: compile through
  `StepCompilerRegistry`; assert: `TransactionError::ParallelInBlock` naming
  step + datasource.
- name: `canonical_arm_rejects_transacted_with_runtime_sql_source` —
  arrange: canonical `Transacted` step whose joined sql step uses a
  header-provided statement; act: `canonical_step_to_builder_step`; assert:
  `RuntimeSqlInJoinedStep` naming the step.
- name: `builder_walk_rejects_file_sql_source` — arrange: joined step with
  `file:` source; act: compile; assert: `RuntimeSqlInJoinedStep` naming the
  step.
- name: `builder_walk_rejects_transaction_control_sql` — arrange: joined
  step whose statement text contains `COMMIT`; act: compile; assert:
  `TransactionControlSql` naming step + statement.
- name: `builder_walk_rejects_unprovable_dialect` — arrange: datasource URL
  `opaque://host/db`, fake manager registered in the context registry; act:
  compile; assert: `DialectUnprovable` naming `opaque`.
- name: `builder_walk_rejects_urlless_registered_datasource` — arrange:
  fake manager registered, `resolve_url` returns `None`; act: compile;
  assert: `DialectUnprovable` naming the DATASOURCE in the `scheme` field
  (probe R2-3 rule: no scheme exists to name).
- name: `builder_walk_rejects_mysql_and_sqlite_joins` — arrange: `mysql://`
  and `sqlite://` datasources with registered fake manager; act: compile;
  assert: `UnsupportedJoinDialect` naming the dialect; standalone steps
  outside blocks still compile.
- name: `builder_walk_rejects_dynamic_targets_in_block` — arrange: block
  containing BuilderStep dynamic-target steps (`DeclarativeDynamicRouter`/
  `DeclarativeRoutingSlip`/`DynamicRouter`/`RoutingSlip` — probe R2-4:
  `CanonicalStepSpec` cannot author one, this seam can); act: compile;
  assert: `DynamicTargetUnprovable` naming the step.
- name: `manager_missing_still_fails_closed_after_walk` — arrange: block on
  an unregistered datasource (no context registry); act: compile; assert:
  `ManagerMissing` naming the datasource.
- name: `registered_manager_dark_gate_no_block_compiles` — arrange: clean
  PostgreSQL block, fake manager REGISTERED in the context registry; act:
  compile through `StepCompilerRegistry`; assert: `JoinSessionMissing`
  naming the datasource — no transacted block compiles or runs in S2a.
- name: `canonical_dark_gate_propagates_before_runtime_registration` —
  arrange: `CommandDeps` with a fake manager registered + a stub
  `RuntimeExecutionPort` recording invocations; act:
  `handle_register` (or `canonical_to_route_definition`) with a clean
  PostgreSQL `CanonicalRouteSpec` transacted block; assert:
  `JoinSessionMissing` propagates as the command error, the stub execution
  port was NEVER invoked, and the aggregate projection is NOT Failed
  (probe R2-1 boundary: rejection in the `?` zone, no compensation).

**Acceptance**: `- [x] 2.2` when every canonical/BuilderStep fixture asserts
the exact variant + offender fields, the dark gate holds at BOTH the
canonical boundary (`?` zone, port never invoked) and `compile_step`, and
all pre-existing compile_step behavior (non-transacted steps, no-registry
contexts) is unchanged.

### 2.3 RuntimeBus registration entry point (test-exercised only — ruling 2 §C)

- [ ] 2.3

**Files**: `crates/camel-core/src/lifecycle/application/runtime_bus.rs`,
`crates/camel-bundles/src/lib.rs` (registry-empty test only)

**Steps**:
1. Add `RuntimeBus::register_transaction_manager(name, Arc<dyn
   TransactionManager>) -> Result<(), RegistryError>` (duplicate-rejecting,
   delegating to the `NamedRegistry`). NO production caller registers in
   S2a.
2. Add a TEST-ONLY fake `TransactionManager` in camel-core (beside the
   bus/walk tests); it exercises the entry point and resolution without
   touching camel-sql. NO `SqlTransactionManager` is built in S2a (S2b
   owns the manager type — ruling 2 §C).
3. camel-bundles and camel-cli register NOTHING: boot leaves the
   transaction registry EMPTY.
4. `CommandDeps.transaction_registry` (exists from S1) stays threaded end
   to end, resolving zero managers — every block fails `ManagerMissing`.

**Tests** (executable):
- name: `runtime_bus_registers_and_resolves_manager` — arrange: `RuntimeBus`
  + the test-only fake manager for `pg`; act:
  `register_transaction_manager("pg", fake)`, then
  `resolve_transaction_manager(deps.transaction_registry, "pg")`;
  assert: resolves to the same Arc; duplicate registration errors.
- name: `unregistered_datasource_still_fails_closed` — arrange: registry
  without `audit`; act: resolve; assert: `ManagerMissing` naming `audit`.
- name: `bundle_boot_leaves_transaction_registry_empty` — arrange: bundle
  config with datasources `pg` + `audit` (any datasources); act: boot;
  assert: the transaction registry has NO KEYS (ruling 2 §C).
- name: `managed_behaviour_untouched` — arrange: run the existing S1
  `Managed` rejection and Auto-fallback suites unmodified
  (`SqlEndpointConfig::validate_transaction_mode` still rejects `Managed` at
  URI-surface endpoint creation) PLUS new direct assertions: producer call
  with `transaction_mode = Managed` executes the Auto path on the test
  connection (succeeds, no transaction semantics) and consumer poll with
  `Managed` proceeds as Auto (probe R2-4: direct behavior assertions at
  producer.rs:251 / consumer.rs:611, not just a green rerun); act: full
  camel-component-sql run; assert: zero failures, zero test edits.

**Acceptance**: `- [x] 2.3` when all four tests pass, boot leaves the
registry key-empty, and no production path registers a manager.

### 2.4 DEFERRED_VARIANTS move (vocabulary decision)

- [ ] 2.4

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

**Acceptance**: `- [x] 2.4` when both tests pass.

### 2.5 ADR amendment line + dynamic-target fixture cross-ref

- [ ] 2.5

**Files**: `docs/adr/0101-transacted-block-surface.md`

**Steps**:
1. Add one amendment line (Accepted): in-block dynamic routing targets
   (`RecipientList`/`RoutingSlip`) are REJECTED at load
   (`DynamicTargetUnprovable`) — membership cannot be proven statically.

**Tests** (executable):
- name: `adr_amendment_names_the_rejection` — arrange: read the ADR file in
  a docs-lint style test or assert in the 2.1 fixture comment; act: grep for
  `DynamicTargetUnprovable`; assert: present in the amendment section.

**Acceptance**: `- [x] 2.5` when the amendment line exists and the 2.1
dynamic-target fixture references it.
