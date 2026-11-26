# Tasks: transacted-canonical-walk

Single-phase change. The structural walk is already wired on `main` (both
lowering fan-ins call `camel_core::transacted_walk`); these tasks pin the
mission-named tests, replace the ADR-0101 paragraph, and record the mutation
evidence for the impossible-on-main pre-fix runs. No production behaviour
changes.

## camel-core tests

### Task 1.1: canonical-entry structural rejections (tests 1-3)

**Files:**
- `crates/camel-core/src/lifecycle/application/commands_tests.rs` (modified)

**Steps:**
1. In the existing test module (wired via `#[path]` at
   `crates/camel-core/src/lifecycle/application/commands.rs:1390`), reuse the
   existing fixtures `registry_with_pg()` (registers a manager for `pg`) and
   the `canonical_to_route_definition` / `canonical_step_to_builder_step_at`
   helpers. Add the needed imports at the test-fn scope:
   `camel_api::MulticastConfig`,
   `camel_api::runtime::{CanonicalSplitAggregationSpec, CanonicalSplitExpressionSpec}`,
   `camel_api::transaction::TransactionError` (already imported in some fns;
   do not duplicate file-level imports).
2. Add `canonical_block_rejects_parallel_split`: lower
   `CanonicalStepSpec::Transacted(CanonicalTransactedSpec { datasource: "pg",
   steps: vec![CanonicalStepSpec::Split { expression:
   CanonicalSplitExpressionSpec::BodyLines, aggregation:
   CanonicalSplitAggregationSpec::LastWins, parallel: true, parallel_limit:
   None, trace_item_threshold: None, stop_on_exception: false, steps:
   vec![CanonicalStepSpec::To { uri: "sql:select 1?datasource=pg" }] }] })`
   through `canonical_step_to_builder_step_at(step, &registry, None, 0)`.
3. Add `canonical_block_rejects_wiretap_and_multicast`, three halves in one
   test fn:
   - (a) wiretap on a joined sql URI: `Transacted { datasource: "pg",
     steps: [To("sql:select 1?datasource=pg"), WireTap { uri:
     "sql:select 1?datasource=pg" }] }` → `WireTapInBlock`;
   - (b) wiretap callee on a foreign datasource: `Transacted { datasource:
     "pg", steps: [WireTap { uri: "sql:select 1?datasource=audit" }] }` →
     `ForeignDatasource` (slice-1 callee-side minor);
   - (c) multicast: no canonical variant exists, so build
     `BuilderStep::Transacted { datasource: "pg", steps:
     vec![BuilderStep::Multicast { steps: vec![BuilderStep::To(
     "sql:select 1?datasource=pg".into())], config:
     MulticastConfig::new() }] }` and call
     `crate::transacted_walk::walk_transacted_block(&built,
     &|ds| registry.get(ds), &|_ds: &str| None)` → `MulticastInBlock`.
   Use match/assert_eq on the exact `TransactionError` variants (as the
   existing `transaction_error` helper does), not on rendered strings.
4. Add `canonical_block_rejects_nested_and_unnamed`, two halves:
   - nested: `Transacted { datasource: "pg", steps: [Transacted {
     datasource: "inner", steps: [] }] }` → `NestedBlock { inner: "inner" }`;
   - unnamed: `Transacted { datasource: "pg", steps: [To("sql:select 1")] }`
     → `UnnamedDatasource { step: "sql:select 1" }`.
5. In each test fn, add a one-line comment stating the manager for `pg` IS
   registered, so the asserted variant proves the structural check wins over
   `ManagerMissing` (the slice-2 incident).

**Tests:** (executable spec — name, arrange, act, assert)
- `canonical_block_rejects_parallel_split`: arrange canonical Transacted
  block with a parallel split over `sql:select 1?datasource=pg` and a registered
  `pg` manager; act: `canonical_step_to_builder_step_at(step, &registry,
  None, 0)`; assert `Err(CamelError::Transaction(TransactionError::ParallelInBlock
  { step, datasource }))` with `step == "split"`, `datasource == "pg"`.
  Command: `cargo test -p camel-core canonical_block_rejects_parallel_split`.
  Expected: passes on the current wiring (the fix already landed on main);
  fails under the Task 1.5 mutation.
- `canonical_block_rejects_wiretap_and_multicast`: (a) assert
  `WireTapInBlock { step: "sql:select 1?datasource=pg", datasource: "pg" }`;
  (b) assert `ForeignDatasource { step: "sql:select 1?datasource=audit",
  block_datasource: "pg", step_datasource: "audit" }`; (c) assert
  `MulticastInBlock { step: "multicast", datasource: "pg" }`. Command:
  `cargo test -p camel-core canonical_block_rejects_wiretap_and_multicast`.
  Expected: passes.
- `canonical_block_rejects_nested_and_unnamed`: assert
  `NestedBlock { inner: "inner" }` and
  `UnnamedDatasource { step: "sql:select 1" }`. Command:
  `cargo test -p camel-core canonical_block_rejects_nested_and_unnamed`.
  Expected: passes.

**Acceptance:**
- The three test fns exist with exactly those names and pass.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- No production code in `commands.rs` or `transacted_walk.rs` is modified by
  this task (`git diff --stat` touches only `commands_tests.rs`).

- [x] 1.1

### Task 1.2: dynamic-target rejection pinned at the walk entry (test 4)

**Files:**
- `crates/camel-core/src/transacted_walk.rs` (modified — test module only,
  next to `walk_rejects_dynamic_targets_in_block` at ~line 1299)

**Steps:**
1. Add `canonical_block_dynamic_uri_is_rejected_at_load` to the existing
   `#[cfg(test)] mod tests` in `transacted_walk.rs`. Reuse the module's
   `block(...)` / `sql(...)` / `predicate()` helpers and the
   registered-manager resolver shape (`FakeManager` + `resolve_registered`).
2. Structure the test with a doc comment recording the decision: reject at
   load, never membership proof — the routing expression is evaluated at
   exchange time, so block membership is not statically provable; the
   canonical `CanonicalStepSpec` cannot author a dynamic target at all, so
   this test exercises the SAME `walk_transacted_block` entry and
   registry-backed resolver shape the canonical fan-in uses.
3. Arrange `BuilderStep::Transacted { datasource: "registered_ds", steps:
   [sql("sql:select 1?datasource=registered_ds"),
   BuilderStep::DeclarativeRecipientList { expression: predicate(),
   delimiter: ",".into(), parallel: false, parallel_limit: None,
   stop_on_exception: false, aggregation: "last_wins".into() }] }`; act:
   `transaction_error(&step)` (the existing helper); assert
   `TransactionError::DynamicTargetUnprovable { step: "recipient-list" }`.
4. Second half: `BuilderStep::DeclarativeRoutingSlip { expression:
   predicate(), uri_delimiter: ",".into(), cache_size: 0,
   ignore_invalid_endpoints: false }` inside the same block shape → assert
   `DynamicTargetUnprovable { step: "routing-slip" }`.

**Tests:**
- `canonical_block_dynamic_uri_is_rejected_at_load`: arrange/act/assert as
  above. Command:
  `cargo test -p camel-core canonical_block_dynamic_uri_is_rejected_at_load`.
  Expected: passes on the current wiring; fails under the Task 1.5
  dynamic-arm mutation.

**Acceptance:**
- The test fn exists with that exact name and passes.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `git diff --stat` touches only `transacted_walk.rs`.

- [x] 1.2

## camel-dsl test

### Task 1.3: declarative and canonical share one walk (test 5)

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified — inline `mod tests` at ~line
  2682)

**Steps:**
1. Add `declarative_and_canonical_share_one_walk` to the inline test module.
2. Structural half — production-only `include_str!` assertions. The test
   module itself contains the searched strings, so search only the production
   text before the test module:
   - `let src = include_str!("compile.rs");`
     `let (prod, tests) = src.split_once("\n#[cfg(test)]\nmod tests {")`
     `.expect("test module marker present");` assert `!tests.is_empty()`
     (the split really happened) and assert `prod` contains
     `"walk_transacted_block_deferred(&built, resolve_manager)"`;
   - `include_str!("../../camel-core/src/lifecycle/application/commands.rs")`
     contains `"crate::transacted_walk::walk_transacted_block("` — the
     opening parenthesis excludes the `_deferred*` wrappers. This is a
     textual read only — no new crate dependency.
3. Behavioural half — build a `DeclarativeRoute` with the fields
   `from: "direct:start"`, `route_id: "share-walk"`, `auto_startup: true`,
   `startup_order: 0`, all optional fields `None` (including
   `http_error_mapping`), `steps: vec![DeclarativeStep::Transacted(
   TransactedStepDef { datasource: "pg".into(), steps:
   vec![DeclarativeStep::To(ToStepDef::new("sql:select 1?datasource=pg")),
   DeclarativeStep::RecipientList(RecipientListStepDef { expression:
   make_predicate("${body}"), delimiter: ",".into(), parallel: false,
   parallel_limit: None, stop_on_exception: false, aggregation:
   MulticastAggregationDef::LastWins })] })]`; act:
   `compile_declarative_route(route)`; assert
   `Err(CamelError::Transaction(TransactionError::DynamicTargetUnprovable
   { step: "recipient-list" }))`. NOTE: the S1 AST validator defers
   dynamic targets (`transacted_validation.rs:316-318`), so this rejection
   proves the LOWERED walk ran on the declarative entry.
4. Direct-walk half — build the equivalent
   `camel_core::BuilderStep::Transacted { datasource: "pg", steps:
   [To("sql:select 1?datasource=pg"), DeclarativeRecipientList {
   expression: make_predicate("${body}"), delimiter: ",".into(),
   parallel: false, parallel_limit: None, stop_on_exception: false,
   aggregation: "last_wins".into() }] }` and call
   `camel_core::transacted_walk::walk_transacted_block(&built, &|_ds| None,
   &|_ds: &str| None)`; assert the SAME
   `TransactionError::DynamicTargetUnprovable { step: "recipient-list" }`.

**Tests:**
- `declarative_and_canonical_share_one_walk`: arrange/act/assert as above.
  Command: `cargo test -p camel-dsl declarative_and_canonical_share_one_walk`.
  Expected: passes (the source contains both call strings today; the
  declarative compile rejection comes from the lowered walk).

**Acceptance:**
- The test fn exists with that exact name and passes.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.
- `git diff --stat` touches only `crates/camel-dsl/src/compile.rs`.

- [x] 1.3

## ADR

### Task 1.4: replace the softened ADR-0101 paragraph

**Files:**
- `docs/adr/0101-transacted-block-surface.md` (modified — §(h) S1 bullet,
  lines ~191-200)

**Steps:**
1. Replace the softened tail of the S1 bullet. Old text (exact, current
   ADR-0101 lines 191-200):
   `Those rejections hold on the` / `DECLARATIVE load path — the validating
   compile entry. The canonical and` / `programmatic lowering paths resolve
   the manager and fail closed with the` / `typed `ManagerMissing` backstop,
   but they do NOT run the structural walk;` / `covering them at the
   lowering fan-in is a BLOCKING obligation of S2 (`bd` / `rc-c3mjp`).`
   followed by the ManagerMissing rejection sentences.
2. Replace with the verbatim paragraph recorded in
   `openspec/changes/transacted-canonical-walk/design.md` (§ "ADR-0101
   paragraph", the block quote): it states the walk holds on the declarative
   and canonical lowering paths through ONE shared walk (`sqltrans-exec`
   step 3; pinned by `transacted-canonical-walk`), names the programmatic
   `RouteDefinition` seam that does not run the walk, the typed
   `JoinSessionMissing` route-add backstop that protects it today, and bd
   `rc-ltfyp` (blocks `rc-ggo3d`) as the owner of that seam. The
   replacement paragraph already ends with the `Managed` clause, so keep
   ONLY `Runtime behavior is unchanged.` from the old text — do not repeat
   the `Managed` sentence.

**Tests:** (doc task — verification by grep)
- Verification commands: `grep -c "do NOT run the structural walk"
  docs/adr/0101-transacted-block-surface.md` → 0; `grep -c "rc-ltfyp"
  docs/adr/0101-transacted-block-surface.md` → 1; `grep -c
  "JoinSessionMissing" docs/adr/0101-transacted-block-surface.md` → ≥1.
- `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- The softened sentence is gone; the replacement matches design.md verbatim.
- No other ADR section changes (`git diff docs/adr/0101-transacted-block-surface.md`
  is confined to the S1 bullet).

- [x] 1.4

## Evidence

### Task 1.5: mutation evidence for the impossible-on-main pre-fix runs

**Files:**
- `crates/camel-core/src/lifecycle/application/commands.rs` (scratch edit,
  reverted)
- `crates/camel-core/src/transacted_walk.rs` (scratch edit, reverted)
- `/home/shared/tmp/transacted-canonical-walk/mutation-canonical-bypass.log`
  (new, evidence)
- `/home/shared/tmp/transacted-canonical-walk/mutation-dynamic-arm.log` (new,
  evidence)

**Steps:**
1. Back up both files with `cp` to
   `/home/shared/tmp/transacted-canonical-walk/<file>.orig`.
2. Mutation A: comment out the `walk_transacted_block(...)` call block in
   the canonical `Transacted` arm (`commands.rs:1373-1377`) — replace the
   call with `/* mutation: walk bypassed */`. Run
   `cargo test -p camel-core canonical_block_rejects 2>&1 | tee /home/shared/tmp/transacted-canonical-walk/mutation-canonical-bypass.log`;
   expect FAILED for tests 1-3 (the `canonical_block_rejects` filter does
   not run the existing `canonical_fan_in_*` tests; those fail too but are
   outside this filter's run). Restore from the backup with `cp`; verify
   with `cmp crates/camel-core/src/lifecycle/application/commands.rs
   /home/shared/tmp/transacted-canonical-walk/commands.rs.orig` (silent,
   exit 0). NOTE: tests 1-3 lower through
   `canonical_step_to_builder_step_at` at depth 0, which reaches this walk
   call; the multicast half of test 2 calls the walk directly and is
   unaffected by the mutation, but test 2 still fails through its wiretap
   halves (a)/(b).
3. Mutation B: neutralize the dynamic-target arm in `check_per_arm`
   (`transacted_walk.rs:347-355`) by returning `Ok(())` before the match
   (scratch). Run
   `cargo test -p camel-core canonical_block_dynamic_uri_is_rejected_at_load 2>&1 | tee /home/shared/tmp/transacted-canonical-walk/mutation-dynamic-arm.log`;
   expect FAILED. Restore from the backup with `cp`; verify with
   `cmp crates/camel-core/src/transacted_walk.rs
   /home/shared/tmp/transacted-canonical-walk/transacted_walk.rs.orig`
   (silent, exit 0). Do NOT use `git diff --stat` as the restore check:
   the Task 1.2 test in `transacted_walk.rs` is uncommitted, so `git diff`
   would show it even when the restore is correct.
4. Confirm the suite is green again after both restores:
   `cargo test -p camel-core canonical_block_rejects` and
   `cargo test -p camel-core canonical_block_dynamic_uri_is_rejected_at_load`.
5. Record in both logs a first line naming the mutation and the expected
   wrong behaviour, matching the mission text ("canonical blocks skipping the
   walk" / "dynamic targets accepted inside a block").

**Tests:** (evidence task — the assertions are on the logs)
- `mutation-canonical-bypass.log` contains test failures for exactly tests
  1-3. Expected: non-empty `FAILED`/`panicked` occurrences.
- `mutation-dynamic-arm.log` contains a failure for test 4. Expected:
  same.
- Post-restore: both named test commands exit 0.

**Acceptance:**
- Both logs exist under `/home/shared/tmp/transacted-canonical-walk/`.
- `git status --short` shows no modified production files from this task.
- Post-restore test runs pass.

- [x] 1.5

### Task 1.6: focused verification sweep

**Files:**
- none (verification only)

**Steps:**
1. Run `cargo fmt --check --all` (cwd: worktree) — exit 0.
2. Run `cargo build --workspace` — exit 0.
3. Run `cargo clippy -p camel-core -p camel-dsl --all-targets -- -D warnings`
   — exit 0. (The full AGENTS.md clippy registry and the other mission gates
   — `cargo test --workspace --lib`, the hexagonal boundary lane
   `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`,
   `cargo xtask lint-context-citations`, `bash scripts/gates-local.sh` — are
   owned by the STAGE 4 gate sweep and recorded there; this task is the
   fast pre-check so a worker does not hand back a red tree.)
4. Run `cargo test -p camel-core` — exit 0.
5. Run `cargo test -p camel-dsl` — exit 0.
6. Run `openspec validate transacted-canonical-walk --type change --json`
   (cwd: worktree) — `valid: true`.

**Tests:** (the commands above are the tests; no new test fns)
- Each command's exit code is recorded for the park report.

**Acceptance:**
- All five commands exit 0.
- `git status --short` shows only the intended change files modified.

- [x] 1.6
