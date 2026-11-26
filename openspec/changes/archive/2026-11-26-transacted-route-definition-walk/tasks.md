# Tasks: transacted-route-definition-walk

## camel-core

### Task 1.1: Add the registration-seam scan to `transacted_walk`

**Files:**
- `crates/camel-core/src/transacted_walk.rs` (modified)

**Steps:**
1. In `crates/camel-core/src/transacted_walk.rs`, import `RouteDefinition`
   from `crate::lifecycle::application::route_definition` alongside the
   existing `BuilderStep` import.
2. Add `pub(crate) fn walk_route_definition_transacted_blocks(definition:
   &RouteDefinition, resolve_manager: &dyn Fn(&str) -> Option<Arc<dyn
   TransactionManager>>, resolve_url: &dyn Fn(&str) -> Option<String>) ->
   Result<(), CamelError>`. It calls the private scan helper over
   `definition.steps()` first, then over
   `definition.circuit_breaker_fallback()`.
3. Add `fn walk_outermost_blocks(steps: &[BuilderStep], resolve_manager:
   &dyn Fn(&str) -> Option<Arc<dyn TransactionManager>>, resolve_url: &dyn
   Fn(&str) -> Option<String>) -> Result<(), CamelError>`: iterate `steps` in
   order; for a `BuilderStep::Transacted { .. }` node call the existing
   `walk_transacted_block(step, resolve_manager, resolve_url)?` and do NOT
   descend (the visitor owns the block subtree and rejects nesting); for any
   other node recurse into each slice returned by the existing
   `child_steps(step)`.
4. Extend the module doc comment: the registration seam
   (`handle_register_internal`) is a third caller of the ONE visitor; the
   scan enters only outermost blocks in the route body and the
   circuit-breaker fallback; document order is preserved and first error
   wins.
5. Keep every existing public function and private helper unchanged.
6. Scenario ownership note ("all entries share one structural walk", seam
   clause): the seam's source-level evidence is that
   `handle_register_internal` calls
   `crate::transacted_walk::walk_route_definition_transacted_blocks`
   (Task 1.2) and that helper calls the SAME `walk_transacted_block` — the
   four `route_definition_*` tests in Task 1.3 exercise it through the real
   port. Do not add a fifth scenario test outside the mission's fixed five.
   Acceptance for this scenario is the two source checks in Acceptance
   below, not a new test.

**Tests:** (in the existing `#[cfg(test)] mod tests`)
- `route_definition_scan_rejects_offence_in_circuit_breaker_fallback`:
  setup a `RouteDefinition::new("direct:start", vec![])` with
  `with_circuit_breaker_fallback(vec![BuilderStep::DeclarativeFilter {
  predicate: LanguageExpressionDef { language: "simple".into(), source:
  "true".into() }, steps: vec![BuilderStep::Transacted { datasource:
  "pg".into(), steps: vec![BuilderStep::DeclarativeRecipientList {
  expression: LanguageExpressionDef { language: "simple".into(), source:
  "${body}".into() }, delimiter: ",".into(), parallel: false,
  parallel_limit: None, stop_on_exception: false, aggregation:
  "last_wins".into() }] }] }])`; act call
  `walk_route_definition_transacted_blocks` with a fake manager for `pg` and
  a resolver returning `"postgres://localhost/pg"`; assert
  `TransactionError::DynamicTargetUnprovable { step: "recipient-list" }`.
- `route_definition_scan_rejects_nested_block_in_fallback`: fallback holds
  one `Transacted { datasource: "pg" }` whose body contains another
  `Transacted { datasource: "pg" }` block; act/scan; assert
  `TransactionError::NestedBlock { inner: "pg" }`.
- `route_definition_scan_plain_definition_is_ok`: steps
  `[BuilderStep::To("direct:next".into())]`, empty fallback; act/scan with
  resolvers that would return `None`/panic if invoked; assert `Ok(())`.
- Command: `cargo test -p camel-core --lib transacted_walk`.

**Acceptance:**
- `cargo test -p camel-core --lib transacted_walk` exits 0.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- No existing test in `transacted_walk.rs` is modified.
- Source check A: `rg -n "walk_route_definition_transacted_blocks" crates/camel-core/src/transacted_walk.rs` shows the helper definition and its call to `walk_transacted_block`.
- Source check B: after Task 1.2, `rg -n "walk_route_definition_transacted_blocks" crates/camel-core/src/lifecycle/application/commands.rs` shows exactly the `handle_register_internal` call site.

- [x] 1.1

### Task 1.2: Run the scan at the registration seam and keep the typed error

**Files:**
- `crates/camel-core/src/lifecycle/application/ports/registration_port.rs` (modified)
- `crates/camel-core/src/lifecycle/application/runtime_bus.rs` (modified)
- `crates/camel-core/src/lifecycle/application/commands.rs` (modified)
- `crates/camel-core/src/context.rs` (modified)
- `crates/camel-core/src/context_tests.rs` (modified)

**Steps:**
1. `registration_port.rs`: change `RouteRegistrationPort::register_route`
   to return `Result<(), CamelError>` (import `camel_api::CamelError`;
   drop the now-unused `DomainError` import only if it becomes unused).
2. `runtime_bus.rs::impl RouteRegistrationPort for RuntimeBus`: call
   `handle_register_internal(&deps, def)`; on `Ok` return `Ok(())`; on
   `Err(CamelError::Transaction(err))` return
   `Err(CamelError::Transaction(err))`; on `Err(CamelError::RouteError(msg))`
   return `Err(CamelError::RouteError(format!("invalid state: {msg}")))`; on
   any other `Err(other)` return
   `Err(CamelError::RouteError(format!("invalid state: {other}")))`. This
   preserves today's exact text for non-scan failures. Leave the preceding
   `ensure_journal_recovered().await.map_err(|e| DomainError::InvalidState(e.to_string()))?`
   line as-is: it still converts through `From<DomainError>` to the same
   `invalid state: …` text, and the `DomainError` import stays used.
3. `commands.rs::handle_register_internal`: immediately after
   `let route_id = def.route_id().to_string();` and BEFORE the
   `deps.repo.load(&route_id)` duplicate check, run
   `crate::transacted_walk::walk_route_definition_transacted_blocks(&def,
   &|datasource| deps.transaction_registry.get(datasource), &resolve_url)?`
   where `let catalog = deps.datasource_catalog.as_deref();` and
   `resolve_url = |datasource: &str| catalog.and_then(|c|
   c.get_config(datasource)).map(|config| config.db_url)` (same shape as
   the canonical arm at the `CanonicalStepSpec::Transacted` lowering).
   Remove the two `#[cfg_attr(not(test), allow(dead_code))]` attributes the
   Task 1.1 worker put on `walk_route_definition_transacted_blocks` and
   `walk_outermost_blocks` — the seam call makes them live, and stale
   suppressions must not remain.
4. `context.rs`: in `RuntimeExecutionHandle::add_route_definition` and
   `CamelContext::add_route_definition`, replace the
   `.map_err(Into::into)` on `register_route` with a direct `?`/return (the
   port now yields `CamelError`).
5. `context_tests.rs`: add
   `route_definition_non_scan_failure_keeps_invalid_state_prefix`: build
   `RouteDefinition::new("direct:start", vec![BuilderStep::To("no-such-scheme:probe".into())]).with_route_id("keeps-prefix")`;
   `ctx.add_route_definition`; assert the error is
   `CamelError::RouteError(msg)` with `msg == "invalid state: Component not found: no-such-scheme"`
   (full-payload equality — a prefix check would also accept a doubled
   `invalid state: `).
6. Do NOT touch `lifecycle/adapters/step_compilers/mod.rs` or
   `lifecycle/adapters/runtime_execution.rs`.

**Tests:**
- `route_definition_non_scan_failure_keeps_invalid_state_prefix` (new,
  `context_tests.rs`): unknown component route → action register → assert
  `RouteError` whose message begins `invalid state: ` and names the missing
  component.
- `registered_manager_dark_gate_no_block_compiles` (existing): must pass
  unchanged — run it explicitly.
- Command: `cargo test -p camel-core --lib`.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo test -p camel-core --lib registered_manager_dark_gate_no_block_compiles` exits 0 with no diff to that test.
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` exits 0 (the port file is edited; the test asserts its `pub(crate)` visibility).
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `rg -n "map_err\(Into::into\)" crates/camel-core/src/context.rs` shows no hit on the two `register_route` call sites.

- [x] 1.2

### Task 1.3: Registration-seam context tests (mission-named)

**Files:**
- `crates/camel-core/src/context_tests.rs` (modified)

**Steps:**
1. Add `use camel_api::transaction::TransactionError;`,
   `use camel_api::circuit_breaker::CircuitBreakerConfig;`,
   `use crate::lifecycle::adapters::InMemoryRuntimeStore;`, and
   `use crate::lifecycle::application::ports::RouteRepositoryPort;` (or the
   equivalent paths that are actually visible from `context_tests.rs`).
2. Add the four tests below verbatim by name, using
   `CamelContext::builder().build().await.unwrap()` (same pattern as
   `add_route_definition_produces_registered_state`). Fixtures:
   - block fixture: `BuilderStep::Transacted { datasource: "pg".into(),
     steps: vec![BuilderStep::To("sql:select 1?datasource=pg".into())] }`.
   - parallel split fixture: the block body is one
     `BuilderStep::DeclarativeSplit { expression:
     LanguageExpressionDef { language: "simple".into(), source:
     "${body}".into() }, aggregation:
     camel_api::splitter::AggregationStrategy::LastWins, parallel: true,
     parallel_limit: None, trace_item_threshold: None, stop_on_exception:
     false, steps: vec![BuilderStep::To("sql:select
     1?datasource=pg".into())] }`.
   - dynamic fixture: `RouteDefinition::new("direct:start", vec![])` with
     `.with_circuit_breaker(CircuitBreakerConfig::new())` and
     `.with_circuit_breaker_fallback(vec![BuilderStep::DeclarativeFilter {
     predicate: LanguageExpressionDef { language: "simple".into(), source:
     "true".into() }, steps: vec![BuilderStep::Transacted { datasource:
     "pg".into(), steps: vec![BuilderStep::DeclarativeRecipientList {
     expression: LanguageExpressionDef { language: "simple".into(), source:
     "${body}".into() }, delimiter: ",".into(), parallel: false,
     parallel_limit: None, stop_on_exception: false, aggregation:
     "last_wins".into() }] }] }])` (align field names/types with the enum
     definition; do not invent fields).
3. Name and assert exactly:
   - `route_definition_block_rejects_parallel_split`: build the route as
     `RouteDefinition::new("direct:start", vec![parallel block]).with_route_id("rd-parallel")`
     (the id is required — `RouteDefinition::new` leaves it empty, so the
     status assertion below would pass vacuously without it); build the
     context as `let store = InMemoryRuntimeStore::default(); let ctx =
     CamelContext::builder().runtime_store(store.clone()).build().await.unwrap();`
     (the store is `Clone` over `Arc`); no manager registered; register the
     block route; match `CamelError::Transaction(TransactionError::ParallelInBlock
     { step, datasource })`, assert `step == "split"`, `datasource == "pg"`;
     then assert `ctx.runtime_route_status("rd-parallel").await.unwrap().is_none()`
     (projection) AND `RouteRepositoryPort::load(&store, "rd-parallel").await.unwrap().is_none()`
     (aggregate repository — the projection alone cannot prove no aggregate
     was persisted).
   - `route_definition_block_rejects_dynamic_uri`: no manager registered;
     register the circuit-breaker-fallback definition; match
     `TransactionError::DynamicTargetUnprovable { step }`, assert `step ==
     "recipient-list"`.
   - `route_definition_block_reports_missing_manager`: no manager
     registered; register the clean block route; match
     `TransactionError::ManagerMissing { datasource }`, assert `datasource
     == "pg"` (never `CamelError::RouteError`).
   - `route_definition_plain_route_still_registers`: plain
     `RouteDefinition::new("direct:start", vec![]).with_route_id("rd-plain")`
     (empty steps — same shape as `add_route_definition_produces_registered_state`;
     a `To` step would need a registered component and is not part of the
     seam contract); register succeeds; `runtime_route_status` returns
     `Some("Registered")`.
4. Each route id must be unique per test.

**Tests:**
- `route_definition_block_rejects_parallel_split` — fixture above →
  `add_route_definition` → `ParallelInBlock { step: "split", datasource:
  "pg" }` and no persisted aggregate.
- `route_definition_block_rejects_dynamic_uri` — fallback/container
  fixture → `add_route_definition` → `DynamicTargetUnprovable { step:
  "recipient-list" }`.
- `route_definition_block_reports_missing_manager` — clean block, empty
  registry → `add_route_definition` → `ManagerMissing { datasource: "pg" }`.
- `route_definition_plain_route_still_registers` — plain route →
  register → status `Registered`.
- Command: `cargo test -p camel-core --lib route_definition_`.

**Acceptance:**
- `cargo test -p camel-core --lib route_definition_` exits 0 with all four
  tests present and matching the exact `TransactionError` variants.
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.

- [x] 1.3

## docs

### Task 2.1: Update ADR-0101 §h to the exact new truth

**Files:**
- `docs/adr/0101-transacted-block-surface.md` (modified)

**Steps:**
1. In §h's S1 paragraph, replace the sentences that say a pre-lowered
   `RouteDefinition` "passes no lowering entry and does not run the
   registry-backed walk" and that the executor slice "may not remove that
   backstop before the walk runs there" with the new truth:
   - the internal registration seam
     (`RuntimeBus::register_route` → `handle_register_internal`) now runs
     the same shared walk over `RouteDefinition::steps` and
     `circuit_breaker_fallback` with registry-backed manager resolution
     and catalog-backed dialect resolution, before the runtime side effect
     and before the duplicate-route check;
   - the typed `TransactionError` from the scan crosses the internal
     registration port (non-scan registration failures keep their existing
     `invalid state: …` text);
   - the `JoinSessionMissing` route-add backstop remains in force until
     `rc-ggo3d` removes it deliberately (its surface stays `RouteError` via
     the unchanged execution adapter).
2. Cite bd `rc-ltfyp` as the slice that landed the seam walk.
3. Touch no other section of the ADR.

**Tests:** no Rust test; verification is textual.
- Command: `git diff docs/adr/0101-transacted-block-surface.md` and read the
  full hunk (a `--stat` cannot show which paragraph changed, and a negative
  `rg` can miss old wording split across a line break).
- Command: `rg -n "runs the same shared walk" docs/adr/0101-transacted-block-surface.md` → exactly one hit in §h (this phrase exists only after the rewrite).
- Command: `cargo xtask lint-context-citations` (exit 0 if the ADR is in scope; otherwise record "N/A — ADR not cited by CONTEXT files").

**Acceptance:**
- The full ADR diff shows only §h's S1 paragraph changed; the stale claims
  ("passes no lowering entry and does not run the registry-backed walk",
  "may not remove that backstop before the walk runs there") are gone.
- ADR §h names the registration seam as a walk seat and keeps the dark-gate
  ordering promise (JoinSessionMissing stays until `rc-ggo3d`).

- [x] 2.1
