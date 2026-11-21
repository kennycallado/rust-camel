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
  caller's in-flight batch transaction → `plan_batch_items`), THEN the
  named read-back agreement loop that today lives after `plan_batch_items`
  (probe R2-2, round 2: producer.rs:781–813 — per item in order, look up
  the shape's probe kinds, `readback_kinds` on the same transaction, and
  the first missing/unusable/disagreeing read-back flips the WHOLE batch to
  aliases before item 0). The entry point returns the FINAL decision so
  `execute_batch` shrinks to resolve → plan → execute:
  `enum BatchPlan { Aliased { targets: Vec<Option<BindTarget>> },
  Named { per_item_targets: Vec<Vec<Option<BindTarget>>> } }` — `Aliased`
  carries the message-wide scan targets applied on the uncached alias text
  (today's whole-batch fallback), `Named` carries the per-item
  read-back-agreed targets. The `sql-binds` whole-batch no-loosening rules
  and the single-shape `plan_binds` signature stay unchanged.

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
   `canonical_step_to_builder_step`) — the CANONICAL REJECTION BOUNDARY
   (probe R2-1, round 2): this arm runs the walk, then manager resolution,
   then the DARK GATE itself, and in S2a has NO success path — it never
   emits `BuilderStep::Transacted`. It runs inside `handle_register` BEFORE
   `execution.register_route`, so every canonical-path rejection lands in
   the `?` propagation zone (aggregate persisted but unregistered —
   obligation 4's documented constraint — NO compensation, no Failed
   marking, the runtime port never sees the route). Verified boundary
   today: `handle_register` → (aggregate persist, `?` zone) →
   `canonical_to_route_definition(...)?` → `execution.register_route` →
   `controller.add_route` → `build_managed_route` → step compilation, where
   a compile-time error WOULD enter the `runtime_err` compensation branch
   and mark the aggregate Failed. The early gate keeps rejections out of
   that branch;
4. the SHARED `BuilderStep` backstop — the `BuilderStep::Transacted` arm of
   `StepCompilerRegistry::compile_step`, where programmatic and DSL-lowered
   paths land: the same walk → resolution → dark gate sequence. For
   canonical routes this site is unreachable in S2a (the early gate errors
   first); for programmatic/DSL-lowered BuilderSteps it is the boundary, and
   its fires-inside-compensation property is documented and acceptable
   there. It replaces the blanket `ManagerMissing` backstop (which remains
   the fail-closed outcome when no registry is in context).

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
   datasource's URL comes from `resolve_url` (probe R2-3, round 2: with
   ruling 2 §C NOTHING registers in S2a, so production registries are empty
   and NO production path reaches this stage — `ManagerMissing` at (4) fails
   every block first. `resolve_url` is a seam contract: the camel-core sites
   build it from the `DatasourceCatalog` threaded via `CommandDeps`/
   `CompilationContext`; the DSL parse seams stay resolver-parameterized
   test seams — verified, they receive raw text and no `CamelConfig`
   (yaml.rs:110/218/286, json.rs:29)). Unrecognized scheme →
   `DialectUnprovable` naming the scheme; resolved MySQL/SQLite →
   `UnsupportedJoinDialect`; standalone steps outside blocks still load. A
   datasource whose URL cannot be resolved while its manager resolved is
   fail-closed `DialectUnprovable` with the DATASOURCE NAME in the `scheme`
   field (no scheme exists to name; documented on the usage — reachable
   only from misconfigured test fakes in S2a);
6. the DARK terminal gate — the canonical boundary (site 3) and the shared
   backstop (site 4): a walk-clean block with a resolved manager STILL fails
   closed with `TransactionError::JoinSessionMissing { datasource }` because
   the runtime block executor does not exist in S2a (S2b owns everything
   after load; the variant is unused today and joins the ruling's
   admission-error family at runtime in S2b). In production nothing
   registers, so every block fails earlier at (4); the gate is proven by
   the test-registered-fake-manager fixtures. NO transacted block compiles
   or runs in this slice.

`CompilationContext` gains optional registry/catalog access (absent ⇒
always-absent closures ⇒ `ManagerMissing`, byte-identical outcomes to
today's backstop for every existing compile_step caller). `CommandDeps`
gains the optional datasource catalog exactly the way S1 threaded
`transaction_registry`.

Fixture matrix (probe R2-4, round 2 — class × seam, `N/A` where a seam
cannot express the class; `CanonicalStepSpec` has NO dynamic-target
variants, `BuilderStep` HAS the dynamic-router/routing-slip/recipient-list
family, so the canonical-lowering seam is `N/A` for class 2):

| rejection class | declarative DSL walk | canonical lowering (`commands.rs` / `CanonicalStepSpec`) | BuilderStep / `compile_step` |
|---|---|---|---|
| S1 structural set (parallel/multicast/wiretap/async/seda, StreamList, foreign/unnamed datasource, nested block) | S1 fixtures stay green; canonical twins added where expressible (`Split` expressible; multicast/wiretap via their canonical variants) | twins for expressible classes | full twin set (every class) |
| dynamic targets (`DynamicTargetUnprovable`) | fixture (`RecipientList`/`RoutingSlip`) | **N/A** — `CanonicalStepSpec` cannot author one | fixture (`BuilderStep` dynamic-target variants) |
| `RuntimeSqlInJoinedStep` (header/body + `file:`) | fixtures (both sources) | fixture (header source) | fixtures (both sources) |
| `TransactionControlSql` | fixture (`COMMIT` text) | fixture | fixture |
| `DialectUnprovable` / `UnsupportedJoinDialect` | fixtures (fake resolvers; standalone-outside-block still loads) | fixture (fake resolvers) | fixtures (fake resolvers) |
| `ManagerMissing` (unregistered) | S1 fixture stays green | fixture | fixture |
| DARK gate (`JoinSessionMissing`, fake manager registered) | N/A — DSL seams keep the always-absent resolver (S1 contract) | fixture asserting the boundary: error propagates from the early gate, `execution.register_route` never invoked, aggregate NOT marked Failed | fixture (compiles to the gate) |
| `Managed` untouched | direct assertions: `validate_transaction_mode` URI rejection suite stays green; producer/consumer with `transaction_mode = Managed` execute the Auto path unchanged (new direct behavior assertions at producer.rs:251 / consumer.rs:611) | | |

Every fixture asserts the exact variant and offender/datasource fields.

## Registry threading and registration (ruling 2 §C: S2a registers NOTHING)

- **Registry threading (obligation 2)**: `CommandDeps.transaction_registry`
  already reaches the loader (S1). This slice adds ONLY the registration
  entry point on `RuntimeBus`: `register_transaction_manager(name, Arc<dyn
  TransactionManager>) -> Result<(), RegistryError>` (duplicate-rejecting,
  delegating to the `NamedRegistry`). Per ruling 2 §C, **camel-bundles and
  camel-cli register NOTHING in S2a**: no `SqlTransactionManager` is built
  (the manager type is S2b), boot leaves the registry empty, and
  `bundle_boot_leaves_transaction_registry_empty` asserts the registry has NO
  KEYS after boot. Production resolves zero managers at every site, so every
  transacted block fails `ManagerMissing` at precedence (4) — blocks stay
  dark without depending on the dark gate. The dark gate stays reachable
  through TEST-registered fake managers only, proving S2b's base.
- **Test-only fake manager (camel-core)**: a `#[cfg(test)]`-visible fake
  `TransactionManager` lives beside the bus/walk tests; it exercises the
  entry point and the dark gate without touching camel-sql.
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
2. **Registry threading**: the `RuntimeBus::register_transaction_manager`
   entry point lands in S2a; per ruling 2 §C bundle/CLI boot registers
   NOTHING in this slice (registry stays key-empty; S2b wires boot
   registration). Resolution at the canonical lowering seam already lands
   in `commands.rs`.
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

camel-sql (executor + batch planner plumbing), camel-api (additive
`TransactionError::DynamicTargetUnprovable` variant only), camel-core
(walk + canonical boundary + registration entry point + test fake),
camel-dsl (vocabulary guard + DSL fan-in walks), camel-bundles (registry-
empty boot test only; camel-cli untouched in S2a — nothing registers).

## Architecture boundaries

No camel-api PORT changes in S2a (the `Any` supertrait + `join` module delta
belongs to S2b). One ADDITIVE camel-api change is in scope:
`TransactionError::DynamicTargetUnprovable { step }` joins the
`#[non_exhaustive]` family (its doc comment invites slice-2 growth without a
breaking change) — the 604c ruling's req-5 rejection set names it, no shipped
variant carries the semantics, and the fixtures assert it. No runtime
behaviour change: NOTHING registers a manager in S2a, so no manager exists
to execute transactions; every transacted block fails at load
(`ManagerMissing` in production, `JoinSessionMissing` under a
test-registered fake), and S2b owns everything after load. `plan_binds`
remains the only single-shape bind path. No new dependencies.

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

### Phase B: load rejections + walk + registration entry point
- **Goal**: the walk at all four sites with every rejection named and
  fixture-covered per the class × seam matrix; the `RuntimeBus` registration
  entry point (test-exercised only, per ruling 2 §C); `DEFERRED_VARIANTS`
  move.
- **Dependencies**: Phase A.
- **Deliverable**: named unit-tier fixtures for every rejection (S1
  structural set + req-5 SQL guards + dynamic targets) per the fixture
  matrix; entry point + test-only fake manager + registry-empty boot test.
- **Exit-criteria**: every fixture asserts the exact variant and
  offender/datasource fields per the matrix; unregistered datasource fails
  at load with `ManagerMissing`; a test-registered fake manager block fails
  the DARK gate (`JoinSessionMissing`) at the canonical boundary (`?` zone,
  no compensation) and at `compile_step` — no block compiles or runs;
  bundle boot leaves the registry with NO KEYS; `Managed`
  endpoint-creation rejection and Auto-fallback warnings proven untouched
  with direct assertions; vocabulary guard green; openspec validate clean.
