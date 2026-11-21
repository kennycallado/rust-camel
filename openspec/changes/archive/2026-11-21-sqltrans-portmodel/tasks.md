# Tasks: sqltrans-portmodel

Single-phase slice (S1 of the sqltrans family; see `partition.md`). New
symbols introduced here: `TransactionManager`, `TransactionSession`,
`TransactionError`, `TransactionOutcome`, `TransactionState`,
`TransactionManagerRegistry`, `resolve_transaction_manager`,
`is_async_producer_scheme`, `BuilderStep::Transacted`,
`DeclarativeStep::Transacted`, `TransactedStepDef`, `TransactedStep` (serde),
`CanonicalStepSpec::Transacted`, `SqlStepFacts`, `validate_transacted_blocks`,
`compile_declarative_route_with`, builder method `transacted`,
`OpenSessionFuture`/`BeginFuture`/`FinalizeFuture`, `CamelError::Transaction`.
All other referenced symbols exist on `main` @ `448e91421`.

## camel-api: TransactionManager port

### Task 1.1: transaction module — port, errors, CamelError variant, re-exports

**Files:**
- `crates/camel-api/src/transaction.rs` (new)
- `crates/camel-api/src/error.rs` (modified — add one variant)
- `crates/camel-api/src/lib.rs` (modified — module + re-exports)

**Steps:**
1. Create `transaction.rs` with `#[non_exhaustive]` enums:
   - `TransactionState` — `Fresh`, `Active`, `Finalizing`, `Done`, `Detached` (req: block session lifecycle, family spec req 2 names this machine).
   - `TransactionOutcome` — `Committed`, `RolledBack`, `Detached`.
   - `TransactionError` — struct variants exactly (fields `String` unless noted):
     `ManagerMissing { datasource }`, `ParallelInBlock { step, datasource }`,
     `MulticastInBlock { step, datasource }`, `WireTapInBlock { step, datasource }`,
     `AsyncInBlock { step, datasource }`, `SedaInBlock { step, datasource }`,
     `StreamListInBlock { step, datasource }`,
     `ForeignDatasource { step, block_datasource, step_datasource }`,
     `UnnamedDatasource { step }`, `NestedBlock { inner }`,
     `SessionAborted { datasource }`, `AbortedByHandledError { datasource }`,
     `CommitUncertain { datasource }`, `CommitFailed { datasource, reason }`,
     `RollbackUncertain { datasource }`, `PoolExhausted { datasource, max_connections: u32, waited_ms: u64 }`,
     `Timeout { datasource, elapsed_ms: u64 }`, `Busy { datasource }`,
     `JoinSessionMissing { datasource }`,
     `JoinScopeMismatch { datasource, expected_scope: u64, found_scope: u64 }`,
     `RuntimeSqlInJoinedStep { step }`, `TransactionControlSql { step, statement }`,
     `DialectUnprovable { scheme }`, `UnsupportedJoinDialect { dialect, datasource }`.
     Every variant carries a doc comment naming the family-spec requirement it serves. Display strings name the offender and the datasource (e.g. `no TransactionManager for datasource '{datasource}'`, `sql step '{step}' uses outputType=StreamList inside a transacted block on datasource '{datasource}'`).
2. Add object-safe traits in the same file, hand-typed boxed futures like `DatasourceCatalog` (`crates/camel-api/src/datasource.rs:122-154`):
   - `pub type OpenSessionFuture<'a> = Pin<Box<dyn Future<Output = Result<Arc<dyn TransactionSession>, TransactionError>> + Send + 'a>>` and analogues `BeginFuture<'a>` (`Result<(), TransactionError>`) and `FinalizeFuture<'a>` (`Result<TransactionOutcome, TransactionError>`).
   - `pub trait TransactionManager: Send + Sync + 'static { fn datasource(&self) -> &str; fn open_session(&self) -> OpenSessionFuture<'_>; fn acquire_deadline(&self) -> std::time::Duration; }`
   - `pub trait TransactionSession: Send + Sync + 'static { fn datasource(&self) -> &str; fn state(&self) -> TransactionState; fn begin(&self) -> BeginFuture<'_>; fn set_rollback_only(&self); fn is_rollback_only(&self) -> bool; fn commit(&self) -> FinalizeFuture<'_>; fn rollback(&self) -> FinalizeFuture<'_>; }`
   - Full rustdoc on every public item: what slice 2 relies on (lazy begin, finalization outcome, `'static` for downcast to the concrete session).
3. In `error.rs`, add `#[error(transparent)] Transaction(#[from] TransactionError)` to `CamelError` (enum already `#[non_exhaustive]`), placed after the last typed-payload variant, following the existing `ConfigValidation(ConfigValidationError)` style.
4. In `lib.rs`: `pub mod transaction;` + `pub use transaction::{TransactionError, TransactionManager, TransactionOutcome, TransactionSession, TransactionState};` next to the `IdempotentRepository` re-export.
5. No new Cargo.toml dependency; no sqlx anywhere in camel-api.

**Tests:**
- `port_traits_are_object_safe`: declare `fn _assert(_m: &dyn TransactionManager, _s: &dyn TransactionSession) {}` in the test → file compiles → traits usable as trait objects.
- `transaction_error_display_names_offender_and_datasource`: build `ManagerMissing { datasource: "pg" }`, `ForeignDatasource { step: "s1", block_datasource: "pg", step_datasource: "audit" }`, `StreamListInBlock { step: "s2", datasource: "pg" }` → `format!("{e}")` each → each Display contains the offender string and the datasource string.
- `non_exhaustive_enums_accept_catch_all`: `match` on a `TransactionOutcome` and a `TransactionState` value with only a catch-all arm → compiles (run as part of the same test file's compilation).
- command: `cargo test -p camel-api --lib transaction`

**Acceptance:**
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0
- `cargo xtask lint-missing-docs-ratchet` exits 0 (every new public item documented)
- `cargo test -p camel-api --lib` passes
- no new sqlx references introduced by this change (`rg -n "sqlx" crates/camel-api/` shows only the pre-existing datasource.rs doc-comment hit)

- [x] 1.1

## camel-core: registry + step model

### Task 2.1: TransactionManagerRegistry + BuilderStep::Transacted with all lowering arms

**Files:**
- `crates/camel-core/src/registry.rs` (modified)
- `crates/camel-core/src/lib.rs` (modified — re-export)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified — variant + classifier wrapper + arms)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs` (modified — fail-closed lowering arm)
- `crates/camel-core/src/startup_validation.rs` (modified — recursive walk)

**Steps:**
1. In `registry.rs`: promote `NamedRegistry` itself to `pub struct` — a one-line change; its methods are already `pub`, and the module doc at `registry.rs:3` already contrasts it with the auth crate's *public* `NamedRegistry` (precedent). Keeping it `pub(crate)` under a `pub` alias/fn trips `private_interfaces` under `-D warnings`. Sibling aliases (`IdempotentRegistry` etc.) stay `pub(crate)`. Then: `pub type TransactionManagerRegistry = NamedRegistry<dyn camel_api::TransactionManager>;` and `pub fn resolve_transaction_manager(registry: &TransactionManagerRegistry, datasource: &str) -> Result<Arc<dyn camel_api::TransactionManager>, camel_api::TransactionError>` returning `Err(TransactionError::ManagerMissing { datasource: datasource.to_owned() })` when `registry.get(datasource)` is `None`. The two new items are `pub` (deliberately NOT the sibling `pub(crate)` alias pattern: the transaction registry is part of this change's public port surface) and are re-exported from `camel-core/src/lib.rs` next to the existing `pub use` block, with rustdoc on the promotion and the re-exports stating: "Dark in slice 1 — nothing registers a manager; camel-sql registers per datasource in slice 2 (rc-qv2iz.6)."
2. In `route_definition.rs`, add to `BuilderStep`: `/// A transacted route block...` `Transacted { datasource: String, steps: Vec<BuilderStep> }` and update every exhaustive match the compiler names: `Debug`/`Display`/description arms, and `span_kind_hint` → `Internal` for the block itself.
3. In the same file, next to the existing producer-scheme table (~L691): `pub fn is_async_producer_scheme(scheme: &str) -> bool` wrapping the existing `PRODUCER_SCHEMES` list (kafka, jms, activemq, artemis, mqtt) — a `pub` wrapper so camel-dsl can reuse the table without duplicating it. Note: `seda` is deliberately NOT in that table and is handled by its own dedicated check in Task 4.1; do not route the seda check through this fn.
4. In `step_compilers/mod.rs` (`compile_step`/`compile_steps`, ~L287/316): the `BuilderStep::Transacted` lowering arm returns a typed fail-closed error (`CamelError::Transaction(TransactionError::ManagerMissing { datasource })`) naming the block's datasource. Dark rationale: the DSL load path never reaches the lowering with a block (validation rejects first); a programmatic user constructing `BuilderStep::Transacted` directly must fail closed, never silently skip or panic.
5. In `startup_validation.rs` (URI walk, ~L138-190): recurse into `Transacted` children exactly like the existing composite recursion does for `DeclarativeSplit`.
6. No registry population anywhere — the dark contract requires zero production registrations.

**Tests:**
- `registry_missing_manager_fails_closed`: empty `TransactionManagerRegistry` → `resolve_transaction_manager(&r, "pg")` → `Err(TransactionError::ManagerMissing { datasource })` with `datasource == "pg"`.
- `registry_round_trip_with_fake_manager`: register a test-local `FakeManager` (impl `TransactionManager`, `open_session` panics "not implemented in slice 1") under `"pg"` → resolve → `Ok(m)` with `m.datasource() == "pg"`.
- `builder_step_transacted_constructs`: build `BuilderStep::Transacted { datasource: "pg".into(), steps: vec![BuilderStep::To("sql:select 1?datasource=pg".into())] }` → Debug output contains `"Transacted"` and `datasource: "pg"`.
- `programmatic_transacted_fails_closed`: register a route whose steps contain `BuilderStep::Transacted` via the programmatic route-add path (no DSL load) → route start fails with `CamelError::Transaction(TransactionError::ManagerMissing { .. })` (exercises the Task 2.1 step-4 arm) (exercised directly via `compile_step` with a fully populated `CompilationContext` — the code path route start funnels through; route-add wiring itself is covered by existing route-controller tests).
- `is_async_producer_scheme_table`: `is_async_producer_scheme("kafka")` is true, `is_async_producer_scheme("seda")` is false, `is_async_producer_scheme("sql")` is false.
- command: `cargo test -p camel-core --lib`

**Acceptance:**
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0 (no dead_code: the registry items are pub and re-exported)
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` passes (no core→component dependency introduced)
- no production code registers a manager (`rg -n "register.*TransactionManager" crates/ --type rust -g '!*/tests/*'` has zero hits)

- [x] 2.1

## camel-api + camel-dsl + camel-builder: authoring (YAML / builder / canonical)

### Task 3.1: transacted block authoring — parse, builder, canonical round-trip, schemas

**Files:**
- `crates/camel-dsl/src/model.rs` (modified — `TransactedStepDef` + `DeclarativeStep::Transacted`)
- `crates/camel-dsl/src/route_ast.rs` (modified — serde `TransactedStep` + `RouteDslStep::Transacted` + raw-key check)
- `crates/camel-api/src/runtime.rs` (modified — `CanonicalStepSpec::Transacted` + `validate_contract` recursion)
- `crates/camel-dsl/src/compile.rs` (modified — declarative + canonical compile arms, canonical serialization, `compile_declarative_route_with` seam)
- `crates/camel-dsl/src/transacted_validation.rs` (new)
- `crates/camel-dsl/src/lib.rs` (modified — `mod transacted_validation;`)
- `crates/camel-dsl/src/canonical.rs` (modified — parse-back)
- `crates/camel-builder/src/lib.rs` (modified — fluent `transacted` method)
- `schemas/canonical-route-spec.json` + `crates/camel-lint/schema/route-schema.json` (regenerated, not hand-edited)
- `crates/camel-dsl/tests/transacted_authoring_test.rs` (new)

**Steps:**
1. `model.rs`: `#[derive(Debug, Clone, PartialEq)] pub struct TransactedStepDef { pub datasource: String, pub steps: Vec<DeclarativeStep> }` + `DeclarativeStep::Transacted(TransactedStepDef)`, placed after `Multicast`.
2. `route_ast.rs`: `#[derive(Debug, Deserialize)] #[serde(deny_unknown_fields)] pub struct TransactedStep { datasource: String, steps: Vec<RouteDslStep> }` and `RouteDslStep::Transacted(TransactedStep)` in the untagged enum (~L582). Unknown-key naming needs an explicit mechanism: `RouteDslStep` is `#[serde(untagged)]`, which swallows per-variant `unknown_field` messages (the repo documents this at `yaml.rs:6881` — "the specific unknown_field message is swallowed"). So in the `transacted` mapping path, first deserialize the block entry as a raw YAML map, collect its keys, subtract the known set `{ datasource, steps }`, and fail with an error naming each unknown key before typed conversion runs (same strictness the canonical parser already gets from `deny_unknown_fields` at `canonical.rs:8`).
3. `runtime.rs`: add `Transacted { datasource: String, steps: Vec<CanonicalStepSpec> }` to the closed `#[non_exhaustive]` enum `CanonicalStepSpec` (~L119); extend `validate_contract` (~L367) to recurse into the children; keep the schemars/ts_rs derives on the enum working (derive fallout is mechanical).
4. `compile.rs`: (a) `compile_declarative_route` gains the `DeclarativeStep::Transacted` → `BuilderStep::Transacted` arm (recursive); (b) `compile_canonical_route` (~L448) gains the `CanonicalStepSpec::Transacted` → `BuilderStep::Transacted` arm; (c) `compile_declarative_route_to_canonical(route, allow_loss) -> Result<(CanonicalRouteSpec, Option<CanonicalLossReport>), CamelError>` emits the `transacted` object mirroring the existing composite-step emission so every field survives (loss report stays `None` for blocks — nothing is dropped).
5. `canonical.rs` parse-back: accept the `transacted` object into `CanonicalStepSpec::Transacted`; unknown keys inside `transacted` are a parse error naming the key (already enforced by the parser's `deny_unknown_fields`; add the `transacted` struct there following the existing pattern). Parse-back's output type is the compiled route (`RouteDefinition`/`BuilderStep`), not `DeclarativeStep`.
6. `camel-builder/src/lib.rs`: fluent method `pub fn transacted(self, datasource: impl Into<String>, steps: impl IntoIterator<Item = impl Into<BuilderStep>>) -> Self` on the route builder. Deliberate deviation from the sibling pattern: `split`/`multicast` return dedicated sub-builders (`SplitBuilder`, `MulticastBuilder` ~L807/841); a block has no sub-builder config surface beyond `datasource` + `steps`, so `transacted` returns `Self` — do not introduce a `TransactedBuilder`.
7. Introduce the validation core + the resolver-parameterized seam (they land here so Task 3.1's own tests can use them — this was the W1 sequencing fix): new `transacted_validation.rs` with `pub struct SqlStepFacts { pub datasource: Option<String>, pub uses_db_url: bool, pub stream_list: bool }`, `pub fn sql_step_facts(uri: &str) -> Option<SqlStepFacts>` (returns `Some` only for `sql:` scheme URIs; parses `datasource` (`config.rs:990`), `db_url` presence (`config.rs:987`), `outputType=StreamList` (`config.rs:520`)), and `pub fn validate_transacted_blocks(steps: &[DeclarativeStep], resolve_manager: &dyn Fn(&str) -> Option<Arc<dyn camel_api::TransactionManager>>) -> Result<(), camel_api::CamelError>` with this walk semantics, in evaluation order per block:
   - Nested block checked first: a `Transacted` inside a `Transacted` → `NestedBlock { inner }` (inner datasource).
   - Unconditional: any sql step anywhere inside the block with `stream_list == true` → `StreamListInBlock { step, datasource }` (no reachability condition).
   - Reachability = static presence anywhere in the same block body (syntactic, fail-closed — no flow analysis):
     - `Split` with `parallel == true` whose fragments contain a sql step on the block datasource → `ParallelInBlock { step, datasource }`.
     - `Multicast` whose recipients contain a joined sql step → `MulticastInBlock { step, datasource }`.
     - `WireTap` whose tap/side steps contain a joined sql step → `WireTapInBlock { step, datasource }`.
     - `To` step whose URI scheme satisfies `camel_core::is_async_producer_scheme` (the Task 2.1 pub wrapper over `PRODUCER_SCHEMES`) AND a joined sql step exists in the block → `AsyncInBlock { step, datasource }`.
     - `To` step with scheme `seda:` (handled separately — `seda` is not in the producer table) AND a joined sql step exists in the block → `SedaInBlock { step, datasource }`.
   - Datasource identity: sql step with `Some(ds)` where `ds != block.datasource` → `ForeignDatasource { step, block_datasource, step_datasource }`; sql step with `uses_db_url` and no `datasource` → `UnnamedDatasource { step }`.
   - Last, after the structural walk passes: `resolve_manager(block.datasource)` → `None` → `ManagerMissing { datasource }`; `Some` → block accepted (this is the seam slice 2 extends). Then in `compile.rs` add `pub fn compile_declarative_route_with(route: DeclarativeRoute, resolve_manager: &dyn Fn(&str) -> Option<Arc<dyn TransactionManager>>) -> Result<RouteDefinition, CamelError>` — validation first, then the existing compile logic. (Production wiring of this fn is Task 4.1; nothing in production calls it yet, which keeps this task's diff behavior-neutral.)
8. Regenerate committed schemas: run the repo's schema generation (`cargo xtask schema` in write mode, then `cargo xtask schema --check` exits 0) so `schemas/canonical-route-spec.json` and `crates/camel-lint/schema/route-schema.json` include the `transacted` object — the camel-lint route schema gates YAML authoring of the new key.

**Tests:**
- `yaml_transacted_block_parses`: YAML route with `transacted: { datasource: pg, steps: [ {to: "sql:select 1?datasource=pg"}, {to: "sql:insert ...?datasource=pg"} ] }` → `parse_yaml_to_declarative` → route contains `DeclarativeStep::Transacted(TransactedStepDef { datasource: "pg", steps len 2 })`.
- `transacted_unknown_key_rejected`: same YAML plus `bogus: 1` inside `transacted` → parse fails, error text contains `bogus` (guaranteed by the step-2 raw-key check, not by serde's untagged message).
- `canonical_transacted_round_trip`: YAML → `parse_yaml_to_declarative` → `compile_declarative_route_with(route, test_resolver)` → canonical JSON of that route → `parse_canonical_json` + canonical compile → `RouteDefinition`. Assert `format!("{:?}", canonical_def.steps()) == format!("{:?}", direct_def.steps())` — `BuilderStep` carries no `PartialEq` (`OpaqueProcessor` forbids the derive), so debug-format equality of the `steps()` accessor is the pinned comparison mechanism for all three authorings.
- `builder_matches_yaml_model`: builder-authored route compiled via `RouteBuilder::build()` (it constructs `BuilderStep::Transacted` directly — camel-builder does not depend on camel-dsl, so the builder leg needs no resolver) → debug-format equality of `steps()` with the YAML-compiled `RouteDefinition`; the fake resolver is load-bearing only on the YAML/canonical legs.
- `sql_step_facts_parses_sql_uris`: `sql_step_facts("sql:select 1?datasource=pg")` → `Some(SqlStepFacts { datasource: Some("pg"), uses_db_url: false, stream_list: false })`; `sql_step_facts("sql:select 1?db_url=postgres://x&outputType=StreamList")` → `Some(.. uses_db_url: true, stream_list: true)`; `sql_step_facts("kafka:topic")` → `None`.
- `validation_passes_clean_block_then_manager_missing`: block with two joined sql steps on `pg`, resolver returning `None` → `Err(CamelError::Transaction(TransactionError::ManagerMissing { datasource: "pg" }))`; same input with resolver returning `Some(fake)` → `Ok(())`.
- command: `cargo test -p camel-dsl --test transacted_authoring_test`, `cargo test -p camel-dsl --lib transacted_validation`, and `cargo test -p camel-builder --test canonical_spec_test`

**Acceptance:**
- `cargo clippy -p camel-api -p camel-dsl -p camel-builder --all-targets -- -D warnings` exits 0
- all four tests pass; round-trip debug-format equality holds across YAML/builder/canonical
- `cargo xtask schema --check` exits 0 with both committed schema files regenerated

- [x] 3.1

## camel-dsl + camel-core: production load-path wiring

### Task 4.1: hook the production choke points — threshold loaders and canonical runtime lowering

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified — always-absent resolver inside `compile_declarative_route_with_stream_cache_threshold`)
- `crates/camel-core/src/lifecycle/application/commands.rs` (modified — explicit `Transacted` arm in `canonical_step_to_builder_step` resolving via the registry)
- `crates/camel-core/src/lifecycle/application/commands_tests.rs` (modified — canonical runtime fixtures)

**Steps:**
1. `compile.rs`: `compile_declarative_route_with_stream_cache_threshold` (the PRODUCTION YAML/JSON/template choke point — called from `yaml.rs:324` and `:350`, `json.rs:161`, `template/materializer.rs:341`, and by bundle boot through `camel_dsl::parse_yaml_with_threshold_and_security`) now delegates to `compile_declarative_route_with(route, |_| None)` instead of the unvalidated path: the always-absent resolver IS the dark contract on every production YAML/JSON load. `compile_declarative_route` keeps its signature and keeps delegating to the threshold fn exactly as it does today (`compile.rs:238-245`), so the non-test-caller grep in the acceptance stays true and existing tests keep exercising validation. `compile_declarative_route_with` remains the resolver-parameterized seam introduced in Task 3.1 step 7 — it is the test entry, not a production entry.
2. `commands.rs`: replace the wildcard outcome for the new variant in `canonical_step_to_builder_step` (~L1039) with an explicit `CanonicalStepSpec::Transacted` arm: resolve `resolve_transaction_manager(&registry, datasource)` (the Task 2.1 pub helper; the registry is threaded into the lowering as a new `CommandDeps` field or parameter — command handlers have no registry access today, and `canonical_to_route_definition`'s only caller, `commands.rs:171`, is in scope) → `Err` maps to the typed `TransactionError::ManagerMissing { datasource }` (via `CamelError::Transaction`), `Ok(manager)` lowers to `BuilderStep::Transacted { datasource, steps: children lowered recursively }` (today always `Err` in production — nothing registers; the arm is slice 2's real registry seam). The `#[non_exhaustive]` wildcard arm (~L1256) stays for genuinely future variants and its fail-closed `Config` behavior is untouched for them. This is the W2 prose fix: the load graph is — canonical route registration and journal replay at boot go `handle_register`/recovery → `canonical_to_route_definition` (~L1007) → `canonical_step_to_builder_step` (~L1039), which NOW returns the typed error naming the datasource; YAML/JSON/template loads go through `compile_declarative_route_with_stream_cache_threshold` (step 1). `commands.rs:125` was never the lowering site — that stale claim from the rejected draft is gone.
3. Both hooks run validation (or resolution) BEFORE any lowering, matching `design.md`: structural checks strictly before the manager lookup, manager lookup strictly last.

**Tests:**
- `yaml_production_loader_rejects_block`: `camel_dsl::parse_yaml_with_threshold_and_security` (the public loader bundle boot calls) with a clean `transacted` block on `pg` → `Err(CamelError::Transaction(TransactionError::ManagerMissing { datasource: "pg" }))`.
- `json_production_loader_rejects_block`: the JSON loader whose body calls the threshold fn (`json.rs:161`) with the same route as JSON → the same typed `ManagerMissing { datasource: "pg" }`.
- `canonical_runtime_registration_rejects_block`: in `commands_tests.rs`, register a `CanonicalRouteSpec` containing a `transacted` block through the public register-command entry (`handle_register`, `commands.rs:125` — the same lowering journal replay at boot uses) → `CamelError::Transaction(TransactionError::ManagerMissing { datasource: "pg" })`, NOT `CamelError::Config`.
- command: `cargo test -p camel-dsl --lib transacted` and `cargo test -p camel-core --lib commands`

**Acceptance:**
- `cargo clippy -p camel-dsl -p camel-core --all-targets -- -D warnings` exits 0
- every existing camel-dsl and camel-core test still passes (`cargo test -p camel-dsl`, `cargo test -p camel-core --lib`) — non-transacted routes behave identically on all loaders
- `rg -n "compile_declarative_route_with\\(" crates/ --type rust -g '!*/tests/*' -g '!*transacted_validation*'` shows no production caller of the public seam — the threshold fn delegates through the private validate-then-lower helper (`compile_declarative_route_with_resolver`), so the seam stays a test entry strictly (line refs in this section reflect the rejected draft's file state: yaml.rs loaders now at :329/:355, canonical lowering entry at commands.rs:178)

- [x] 4.1

### Task 4.2: one failing-input fixture per load rejection

**Files:**
- `crates/camel-dsl/tests/transacted_load_rejections.rs` (new)

**Steps:**
1. One test per rejection, each: build the route as a YAML string inline, call the dark default entry `compile_declarative_route` (all ten fixtures use the YAML path; canonical JSON needs no fixture here — its production lowering is covered by `canonical_runtime_registration_rejects_block` in Task 4.1, since the pure canonical-JSON compile seam returns `Ok` by design), and assert the exact variant and fields via `matches!` + field equality on the unwrapped `CamelError::Transaction(_)`:
   - `parallel_split_inside_block_rejected` — split with `parallel: true`, fragment `to: sql:select 1?datasource=pg` → `ParallelInBlock { step, datasource: "pg" }`.
   - `multicast_inside_block_rejected` — multicast recipient sql step on `pg` → `MulticastInBlock { .. }` naming the multicast step and `pg`.
   - `wiretap_inside_block_rejected` — wiretap whose tap step is sql on `pg` → `WireTapInBlock { .. }`.
   - `async_handoff_inside_block_rejected` — `to: kafka:topic` (a scheme in `PRODUCER_SCHEMES`) plus a joined sql step on `pg` → `AsyncInBlock { .. }`.
   - `seda_escape_inside_block_rejected` — `to: seda:q` plus joined sql step on `pg` → `SedaInBlock { .. }`.
   - `stream_list_inside_block_rejected` — `to: sql:select 1?datasource=pg&outputType=StreamList` as the ONLY sql step → `StreamListInBlock { .. }` (unconditional: no consumer step present).
   - `foreign_datasource_inside_block_rejected` — block `pg`, step `sql:...?datasource=audit` → `ForeignDatasource { block_datasource: "pg", step_datasource: "audit", .. }`.
   - `unnamed_datasource_inside_block_rejected` — step `sql:select 1?db_url=postgres://localhost/x` → `UnnamedDatasource { step }`.
   - `nested_transacted_block_rejected` — `transacted { datasource: pg, steps: [ transacted { datasource: pg, steps: [] } ] }` → `NestedBlock { inner: "pg" }`.
   - `unregistered_datasource_fails_at_load` — clean YAML block on `pg` (no structural violations) → `ManagerMissing { datasource: "pg" }` (proves the manager lookup is reachable and typed after the structural pass).
2. One fixture per PRODUCTION entry point, each asserting the typed `ManagerMissing` names the datasource (repair item 4 — replaces the seam-only canonical fixture of the rejected draft). The YAML, JSON, and canonical-registration fixtures live in Task 4.1's test block (`yaml_production_loader_rejects_block`, `json_production_loader_rejects_block`, `canonical_runtime_registration_rejects_block` — the canonical one covers journal replay, which funnels through the same `canonical_step_to_builder_step` lowering); this task adds the fourth:
   - `bundle_boot_rejects_block` (in `crates/camel-bundles/src/security_boot.rs`'s test module or `crates/camel-bundles/tests/`, matching the existing boot-test layout): route a `transacted`-block YAML through the same boot compilation path `security_boot.rs:732` exercises (`camel_dsl::parse_yaml_with_threshold_and_security`) → `CamelError::Transaction(TransactionError::ManagerMissing { datasource: "pg" })`, proving bundle boot fails at load, not at pipeline build.
3. Each structural fixture's route must be minimal: only the offending construct plus the joined sql step the case needs; no other violations that could mask the asserted variant.

**Tests:** the ten structural fixtures plus `bundle_boot_rejects_block` are the deliverable (the other three production-entry fixtures live in Task 4.1); command: `cargo test -p camel-dsl --test transacted_load_rejections` and `cargo test -p camel-bundles --lib --features security security_boot` (the fixture is gated behind camel-bundles' `security` feature — without the flag the module cfg's out and zero copies run)

**Coverage notes (for the record, merge-gate round):**
- Template materializer: no dedicated fixture — `template/materializer.rs:341` calls `compile_declarative_route_with_stream_cache_threshold`, the validating choke point Task 4.1 hooked, so materialized routes inherit the transacted rejections transitively; decision recorded instead of a duplicate fixture.
- Canonical-JSON unknown keys: naming is proven by `canonical_transacted_unknown_key_rejected` (Task 3.1) via the strict canonical payload struct — no further fixture needed.
- Dynamic-URI steps (RecipientList/RoutingSlip) and structural coverage on the canonical/programmatic lowering path are deferred to slice 2 under bd rc-c3mjp (ADR 0101 references it).

**Acceptance:**
- the ten structural fixtures pass, each asserting the exact variant (not just `Err(_)`)
- `bundle_boot_rejects_block` passes and names the datasource
- `cargo clippy -p camel-dsl -p camel-bundles --tests -- -D warnings` exits 0

- [x] 4.2

## Dark landing proof + ADR + docs

### Task 5.1: dark proof, authoring equivalence under a fake manager, txnreject green

**Files:**
- `crates/camel-dsl/tests/transacted_dark_test.rs` (new)

**Steps:**
1. `transacted_block_fails_at_load_dark`: full YAML route (from + transacted block, two joined sql steps on `pg`) → `compile_declarative_route` → `CamelError::Transaction(TransactionError::ManagerMissing { datasource: "pg" })`. This is the dark-landing contract test.
2. `authoring_equivalence_under_fake_manager`: test-local `FakeManager` (impl `TransactionManager`, registered only in the test's resolver closure) → compile the YAML route and the canonical round-tripped route via `compile_declarative_route_with`, and the builder route via `RouteBuilder::build()` (no resolver — camel-builder is dsl-free) → all three succeed and `format!("{:?}", def.steps())` is identical across the three authorings (`BuilderStep` has no `PartialEq`; debug-format equality of the `steps()` accessor is the pinned mechanism). Comment states: production never registers; this fake is test-only evidence for family req 1's THEN clause; end-to-end runtime loading completes in slice 2.
3. Verify untouched: `git diff --stat 448e91421 -- crates/components/camel-sql` prints nothing (the sql component is unmodified in this change; package name `camel-component-sql`, directory `crates/components/camel-sql`) and `cargo test -p camel-component-sql --lib` passes — the existing `txnreject` tests (`create_endpoint_rejects_managed_transaction_mode`, `create_endpoint_accepts_auto_transaction_mode`) stay green unmodified.

**Tests:** the two new tests above; command: `cargo test -p camel-dsl --test transacted_dark_test` and `cargo test -p camel-component-sql --lib`

**Acceptance:**
- dark test asserts the exact `ManagerMissing { datasource: "pg" }` error
- `cargo test -p camel-component-sql --lib` green with zero camel-component-sql diff vs `448e91421`
- `cargo test -p camel-api -p camel-core -p camel-dsl -p camel-builder -p camel-bundles` green

- [x] 5.1

### Task 5.2: ADR 0101, docs skeleton, component lanes

**Files:**
- `docs/adr/0101-transacted-block-surface.md` (new)
- `docs/src/sql-transactions.md` (new — skeleton; confirm the docs tree layout: `ls docs/src` and place the page + `SUMMARY.md` entry to match mdbook conventions)
- `docs/src/SUMMARY.md` (modified — one entry, matching existing chapter style)
- `AGENTS.md` (modified — five lane rows)

**Steps:**
1. ADR 0101, Status: Accepted, covering exactly the family req 15 content list: chosen `transacted { datasource }` surface; rejected surface (a) per-exchange transactions and rejected surface (c) per-endpoint Managed-everywhere, each with the e_opus 2026-11-15 reasons; concurrency/pool cost (one pooled connection + dedicated session per active block, acquire bounded by `acquire_timeout`); slice order S1 (port + model + load rejections, dark) → S2 (`sqltrans-runtime`: execution, finalization, join semantics) → S3 (`sqltrans-modes`: Managed join-required rework); the at-least-once window between DB commit and idempotent-key confirm (unique outbox message id + downstream dedup); restart-on-upgrade statement-cache clearing; PgBouncer session-mode `server_reset_query=DISCARD ALL` guidance. Amendments for S2/S3 are amendments to this ADR, not new ADRs. One additional recorded decision: the load validator reuses the camel-core producer-scheme telemetry table (`is_async_producer_scheme`) as the async-handoff semantic for req 6, and handles `seda:` separately — the coupling is deliberate and noted here so slice 2 inherits it consciously.
2. Docs skeleton page: what exists after S1 (port, registry, model, load rejections — all dark), what lands in S2 and S3, pointer to ADR 0101; the operational note (hygiene, detach semantics, PgBouncer) is added by slice 2 — leave an explicit "Lands in slice 2" stub section rather than writing it early.
3. `AGENTS.md` `## COMPONENT LANES` table, after the existing local rows:
   - `camel-api | local | cargo test -p camel-api`
   - `camel-dsl | local | cargo test -p camel-dsl`
   - `camel-builder | local | cargo test -p camel-builder`
   - `camel-lint | local | cargo test -p camel-lint`
   - `camel-bundles | local | cargo test -p camel-bundles --features security`
   (camel-core already has its hexagonal row; camel-lint needs the row because
   Task 3.1 modifies `crates/camel-lint/schema/route-schema.json`; camel-bundles
   needs it — WITH the security feature, else the boot fixture cfg's out —
   because Task 4.2 adds the bundle-boot rejection fixture there; both crates
   have integration surfaces.)

**Tests:**
- `cargo xtask lint-ignore` exits 0 (docs/adr is tracked, nothing added to ignored paths)
- `cd docs && mdbook test` — run it; if the `mdbook` binary is unavailable locally, record "doc-guide-test deferred to CI" in the mission report (CI owns it)

**Acceptance:**
- ADR 0101 contains all eight req-15 content items verbatim-mapped (surface, two rejected surfaces with reasons, pool cost, at-least-once window, statement-cache clearing, PgBouncer guidance, slice order) plus the classifier-coupling note
- lane rows present; `bash scripts/gates-local.sh` recognizes every touched crate (no `NO LANE` failure)

- [x] 5.2
