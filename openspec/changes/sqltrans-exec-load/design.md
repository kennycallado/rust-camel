# Design: sqltrans-exec-load (S2a — dark load + plumbing)

Split ruling 604c (e_opus 2026-11-21): this is the DARK load+plumbing slice
that `sqltrans-exec-verify` (S2b, runtime semantics) depends on. Parent
family design: frozen `openspec/changes/sqltrans/design.md` (Phases A–E);
S1 landed Phases A+B (`sqltrans-portmodel`, 059aedd7b). This slice carries
the sections moved verbatim from the former S2 design, plus the load-side
obligations S1 recorded as blocking.

## Executor plumbing (Phase A — no behaviour change; moved verbatim)

- **Executor refactor (Phase A, no behaviour change)**: the four execute fns
  (`fetch_single_rows`, `execute_select`, `execute_modify`, `execute_batch`)
  move onto a borrowed-connection surface `DbHandle<'a>` over `&mut
  AnyConnection`; the producer call site acquires from the pool exactly as
  today and lends the handle. Joined steps lend the session's guarded
  connection instead — same downstream code. Preserved unchanged in Phase A:
  standalone (non-joined) batch execution keeps its own pool transaction for
  the statement batch, and StreamList keeps its deferred pooled acquisition
  (StreamList production inside a block is already rejected at load, so the
  streaming path never borrows a session connection).
- **Batch planner (req 12)**: `execute_batch` today resolves every item up
  front (IN-clause expansion can give DISTINCT resolved SQL texts per item),
  groups texts by shape, probes each distinct shape once on the batch
  transaction, runs the message-wide no-loosening bind scan, and plans per
  item with a whole-batch alias rule decided before item 0. Batch planning
  moves behind ONE shared entry point,
  `plan_batch_binds(tx, items: &[(String, Vec<serde_json::Value>)]) ->
  Result<Vec<BatchItemPlan>, CamelError>`: `items` carries each item's OWN
  resolved SQL text plus bindings — never one shared `sql: &str`, which
  cannot represent distinct IN expansions (bless round 1, probe Q3).
  `plan_batch_binds` internally performs exactly today's sequence
  (`group_rows_by_sql` → `scan_bind_targets` → `probe_batch_shapes` on the
  caller's in-flight batch transaction → `plan_batch_items`); `execute_batch`
  shrinks to resolve → plan → execute. The `sql-binds` whole-batch
  no-loosening rules and the single-shape `plan_binds` signature stay
  unchanged.

## Structural walk at the fan-ins (closes rc-c3mjp)

S1's `validate_transacted_blocks` walks the declarative AST with the S1
structural set only. This slice extends that walk with the req-5 guards and
adds the equivalent walk over `BuilderStep` trees. ONE shared walk function,
`validate_transacted_block_builder(step, resolve_manager, resolve_url)`, in
camel-core `transacted_walk.rs`, wired at FOUR sites:

1. the DSL declarative walk (camel-dsl `transacted_validation.rs`) — the
   DECLARATIVE fan-in, extended with guards (2)–(3) below and the
   `resolve_url` seam; its production seams KEEP S1's always-absent manager
   resolver, so declarative/canonical-DSL blocks still fail `ManagerMissing`
   after the static checks (S1's dark contract unchanged);
2. the camel-dsl canonical arm (`compile.rs` `compile_canonical_step`,
   `CanonicalStepSpec::Transacted`) — the second DSL fan-in, same seam
   pattern;
3. the canonical lowering fan-in in camel-core (`commands.rs`
   `canonical_step_to_builder_step`) — early gate before child lowering, with
   the LIVE registry (param, threaded in S1) and the datasource catalog
   threaded through `CommandDeps`;
4. the SHARED `BuilderStep` point — the `BuilderStep::Transacted` arm of
   `StepCompilerRegistry::compile_step`, where every authoring path lands;
   it replaces the blanket `ManagerMissing` backstop (which stays as the
   fail-closed outcome when no registry is in context).

camel-cli drives `camel_bundles::boot` at every entry (run/job/run_tests), so
the bundle-boot registration covers the CLI path transitively — no separate
CLI site.

Rejection precedence (total order, every site — bless round 1, probe Q2):

1. S1 structural set: parallel split/multicast/wiretap/async handoff/seda
   escape reaching a joined sql step, StreamList inside a block,
   foreign/unnamed datasource, nested block;
2. dynamic targets: `RecipientList`/`RoutingSlip` (runtime-target steps)
   inside a block → `DynamicTargetUnprovable` naming the step (new additive
   camel-api variant — the ruling REJECTS them at load);
3. req-5 join-side SQL guards on every joined step's STATIC route SQL:
   header/body/`file:` runtime sources → `RuntimeSqlInJoinedStep` naming the
   step; BEGIN/COMMIT/ROLLBACK/SAVEPOINT and dialect implicit-commit
   statement text → `TransactionControlSql` naming the step and statement;
4. manager resolution → `ManagerMissing` naming the datasource
   (fail-closed; with the DSL seams' always-absent resolver every DSL-loaded
   block fails here — the S1 contract);
5. dialect identity — reachable only once a manager resolved: the block
   datasource's URL comes from `resolve_url` (production: the
   `DatasourceCatalog` threaded like the registry; the DSL parse seams build
   it from `config.datasources` `db_url`s); unrecognized scheme →
   `DialectUnprovable` naming the scheme; resolved MySQL/SQLite →
   `UnsupportedJoinDialect`; standalone steps outside blocks still load;
6. the DARK terminal gate — compile_step's arm ONLY: a walk-clean block with
   a resolved manager STILL fails closed with
   `TransactionError::JoinSessionMissing { datasource }` because the runtime
   block executor does not exist in S2a (S2b owns everything after load; the
   variant is unused today and joins the ruling's admission-error family at
   runtime in S2b). NO transacted block compiles or runs in this slice; the
   registered-manager fixture proves it.

`CompilationContext` gains optional registry/catalog access (absent ⇒
always-absent closures ⇒ `ManagerMissing`, byte-identical outcomes to
today's backstop for every existing compile_step caller). `CommandDeps`
gains the optional datasource catalog exactly the way S1 threaded
`transaction_registry`. Each rejection keeps its named unit-tier fixture
asserting the exact variant and offender fields on BOTH authoring paths
(declarative and canonical/BuilderStep).

## Registry threading and registration (moved verbatim + round-1 fixes)

- **Registry threading (obligation 2)**: `CommandDeps.transaction_registry`
  already reaches the loader; this slice adds the registration entry point on
  `RuntimeBus` (`register_transaction_manager(name, Arc<dyn
  TransactionManager>) -> Result<(), RegistryError>`, duplicate-rejecting via
  the `NamedRegistry`) and camel-bundles registers a `SqlTransactionManager`
  per catalog datasource at boot, before routes load (fail-closed
  `ManagerMissing` otherwise). camel-cli's run/job/test entries all drive
  `camel_bundles::boot`, so boot-site registration covers the CLI path
  transitively.
- **`SqlTransactionManager` (camel-sql, new)**: constructed per catalog
  datasource from the datasource name with lazy pool resolution through the
  existing `DatasourceCatalog`/`OnceCell` path — no eager connect. In S2a its
  `open_session`/`acquire_deadline` are dark stubs (the session machinery is
  S2b); registration + resolution are the deliverable, not session opening.
  Registered-manager blocks die at the compile_step dark gate
  (`JoinSessionMissing`), never at a session call.
- **Obligation 4 (documented constraint)**: `handle_register` persists the
  aggregate first (`repo.save`), then propagates lowering errors — including
  typed `ManagerMissing` from `canonical_to_route_definition` — via `?`
  BEFORE the runtime registration; only `execution.register_route` errors
  enter the pre-existing `runtime_err` compensation branch (Failed marking).
  We do not restructure the aggregate in this slice.

## Obligations carried in from S1 (moved verbatim — all blocking)

1. **rc-c3mjp**: covered by the new `BuilderStep`-tree structural walk at both
   fan-ins (above); canonical/BuilderStep-authored blocks get exactly the DSL
   path's rejections. Named unit-tier fixtures assert each rejection.
2. **Registry threading**: bundle-boot registration via the new `RuntimeBus`
   entry point; resolution at the canonical lowering seam already lands in
   `commands.rs`.
3. **`DEFERRED_VARIANTS` decision**: `CamelError::Transaction` becomes a
   supported in-pipeline kind — `"Transaction"` enters
   `supported_exception_kinds()` with a match arm over `TransactionError`
   variants, and is removed from `DEFERRED_VARIANTS`. Rationale: block
   finalization failures must be catchable by `on_exceptions` (an operator can
   route transaction failures without flattening them into
   `ProcessorError`), while a `do_try` catch inside a block still cannot
   produce a commit (`AbortedByHandledError`). The vocabulary-guard test
   asserts the moved bucket.
4. **handle_register constraint**: documented above; no restructuring.
5. **Dynamic targets (req 5)**: REJECT at load. `RecipientList`/`RoutingSlip`
   (and other runtime-target steps) inside a block fail closed with
   `DynamicTargetUnprovable` — membership cannot be proven statically, and
   silent auto-commit inside a block is the degrade class this family exists
   to remove. One line in the ADR amendment, one named fixture.

## Affected crates

camel-sql (executor plumbing), camel-api (additive
`TransactionError::DynamicTargetUnprovable` variant only), camel-core
(BuilderStep walk + registration seam), camel-dsl (vocabulary guard +
canonical-fan-in walk), camel-bundles (registration; camel-cli's run/job/test
entries register transitively through `camel_bundles::boot`).

## Architecture boundaries

No camel-api PORT changes in S2a (the `Any` supertrait + `join` module delta
belongs to S2b). One ADDITIVE camel-api change is in scope:
`TransactionError::DynamicTargetUnprovable { step }` joins the
`#[non_exhaustive]` family (its doc comment invites slice-2 growth without a
breaking change) — the 604c ruling's req-5 rejection set names it, no shipped
variant carries the semantics, and the fixtures assert it. No runtime
behaviour change: no manager executes
transactions here; the block still fails at load unless a manager is
registered, and S2b owns everything after load. `plan_binds` remains the only
bind path. No new dependencies.

## Phases

### Phase A: borrowed-connection executor (own commit, no behaviour change)
- **Goal**: the four execute fns run on a borrowed connection; one shared
  batch-planner entry point. Zero behaviour change; every existing suite green
  unmodified.
- **Dependencies**: none.
- **Deliverable**: `DbHandle<'a>` + refactored fns + `plan_batch_binds`;
  existing unit + lane suites pass untouched. Standalone batch keeps its own
  transaction; StreamList keeps deferred pooled acquisition.
- **Exit-criteria**: `cargo test -p camel-component-sql` green;
  `cargo test -p camel-test --features integration-tests --no-run` compiles;
  `plan_binds` signature unchanged.

### Phase B: load rejections + walk + registration
- **Goal**: the BuilderStep walk at both fan-ins with every rejection named
  and fixture-covered; `RuntimeBus` registration entry point; bundle/CLI
  registration; `DEFERRED_VARIANTS` move.
- **Dependencies**: Phase A.
- **Deliverable**: named unit-tier fixtures for every rejection (S1
  structural set + req-5 SQL guards + dynamic targets), passing on both
  authoring paths; managers register at boot.
- **Exit-criteria**: every fixture asserts the exact variant and
  offender/datasource fields on BOTH authoring paths (declarative and
  canonical/BuilderStep); unregistered datasource still fails at load with
  `ManagerMissing`; a registered-manager block fails the DARK gate
  (`JoinSessionMissing`) — no block compiles or runs; `Managed`
  endpoint-creation rejection and Auto-fallback warnings proven untouched;
  vocabulary guard green; openspec validate clean.
