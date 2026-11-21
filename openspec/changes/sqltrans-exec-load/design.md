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
- **Batch planner (req 12)**: `execute_batch` today plans per-shape via
  separate single-shape calls; batch planning moves to ONE shared entry point
  (`plan_batch_binds`) that accepts all batch shapes together, without
  changing `sql-binds` whole-batch rules or `plan_binds`' signature.

## Structural walk at the fan-in (closes rc-c3mjp; moved verbatim)

S1's `validate_transacted_blocks` walks the declarative AST only. This slice
adds the equivalent walk over `BuilderStep` trees in camel-core — the SHARED
validation point for ALL authoring paths is the `BuilderStep::Transacted` arm
of `StepCompilerRegistry::compile_step` (every path — DSL-lowered,
builder-authored, canonical-lowered, programmatic — compiles through it; it
replaces the `ManagerMissing` backstop). The canonical arm in `commands.rs`
(`canonical_step_to_builder_step`) runs the same walk as an early gate before
aggregate-side effects continue. The walk enforces the FULL rejection set in
two groups:

(a) the S1 structural set: parallel split/multicast/wiretap/async handoff/
seda escape reaching a joined sql step, StreamList inside a block,
foreign/unnamed datasource, nested block, dynamic target unprovable;

(b) the req-5 join-side SQL guards on every joined step's STATIC route SQL:
reject header/body runtime sources and `file:` sources
(`RuntimeSqlInJoinedStep`), reject transaction-ending/implicit-commit
statement text — BEGIN/COMMIT/ROLLBACK/SAVEPOINT and dialect equivalents
(`TransactionControlSql` naming the step and the offending statement), resolve
the dialect from the datasource URL scheme with a registered manager
(`DialectUnprovable` naming the unresolved scheme for unrecognized schemes;
`UnsupportedJoinDialect` naming MySQL/SQLite for resolved non-PostgreSQL
dialects). Registry resolution stays fail-closed (`ManagerMissing` names the
datasource). Each rejection keeps its named unit-tier fixture asserting the
exact variant and offender fields.

## Registry threading and registration (moved verbatim; corrected by ruling 2, 604d)

- **Registry threading (obligation 2)**: `CommandDeps.transaction_registry`
  already reaches the loader; this slice adds the registration entry point on
  `RuntimeBus` (the registry field is private with no setter) and camel-bundles
  registers a `SqlTransactionManager` per catalog datasource at boot, before
  routes load (fail-closed `ManagerMissing` otherwise).
  camel-cli's assembly path registers through the same entry point.
  **RULING-2 CORRECTION (604d §C, binding): S2a adds ONLY the
  `RuntimeBus::register_transaction_manager` entry point; bundle and CLI
  register NOTHING in this slice (no `SqlTransactionManager` exists yet —
  that implementation and the real registration land with S2b,
  rc-qv2iz.6).** S2a instead adds the guard test
  `bundle_boot_leaves_transaction_registry_empty` (zero managers at boot,
  mission 607); S2b replaces that guard with
  `bundle_boot_registers_catalog_datasources`.
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

camel-sql (executor plumbing), camel-core (BuilderStep walk + registration
seam), camel-dsl (vocabulary guard), camel-bundles (registration).

## Architecture boundaries

No camel-api changes in S2a (the port delta — `Any` supertrait + join module
— belongs to S2b). No runtime behaviour change: no manager executes
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
  offender/datasource fields; unregistered datasource still fails at load
  with `ManagerMissing`; vocabulary guard green; openspec validate clean.
