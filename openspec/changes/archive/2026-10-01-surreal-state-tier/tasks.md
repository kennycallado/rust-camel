# Tasks: surreal-state-tier

## Phase 1: Grammar and load-time contract

### camel-integration-test (parse and load validation)

#### Task 1.1: `surreal:` prepare action — parse, read gate, demand gate

**Files:**
- `crates/camel-integration-test/src/surreal_action.rs` (new)
- `crates/camel-integration-test/src/surreal_action_test.rs` (new)
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/Cargo.toml` (modified)
- `crates/camel-integration-test/src/lib.rs` (modified)

**Steps:**
1. Add the `surreal` Cargo feature to `crates/camel-integration-test/Cargo.toml`:
   `surreal = ["dep:surrealdb", "camel-bundles/surrealdb"]`, and add
   `surrealdb = { workspace = true, optional = true }` to
   `[dependencies]`.
2. Create `surreal_action.rs` mirroring `sql_action.rs`: `pub const
   SURREAL_ACTION_KEY: &str = "surreal";`, `pub struct
   RawSurrealAction { datasource: String, prepare: Vec<String> }`
   (serde `camelCase` attribute retained for future fields), `pub
   struct SurrealAction { pub datasource: String, pub prepare:
   Vec<String> }`, `pub fn is_surreal_read_statement(stmt: &str) ->
   bool` (trimmed prefix `select`, case-insensitive, skipping leading
   `(` groups with re-trim — the `is_read_statement` algorithm with
   `select` as the only prefix), and `pub fn
   validate_surreal_action(raw: &RawSurrealAction, action_index:
   usize) -> Result<SurrealAction, String>` rejecting an empty
   `prepare` list and any `select`-prefixed statement, error strings
   naming the action index and statement index.
3. In `document.rs`, add the `Surreal` variant to the
   `ScenarioAction` enum with its raw twin parsed from the `surreal`
   key, gated exactly as the `Sql` arm: parse and load validation run
   in both feature configurations; when the `surreal` feature is off,
   a document declaring `surreal:` fails at load with the named
   demand-gate error mirroring the `sql:` arm's behavior. Register the
   module in `lib.rs`.

**Tests:** (in `surreal_action_test.rs`, load-level, must pass in
both feature configurations)
- `surreal_read_gate_select_prefix`: a `RawSurrealAction` whose item 0
  is `"(SELECT * FROM user)"` → `validate_surreal_action` returns Err
  naming action index and statement index 0.
- `surreal_read_gate_case_insensitive`: item 0 `"  select 1"` → Err.
- `surreal_write_prefixes_pass`: items `["DEFINE TABLE user SCHEMALESS",
  "CREATE user SET name = 'alice'"]` → Ok.
- `empty_prepare_list_rejected`: empty `prepare` → Err naming the
  action index.
- `surreal_action_feature_off_is_named_load_error`: (run in a build
  without the `surreal` feature) a document declaring `surreal:` →
  load fails with the demand-gate error naming `surreal`, exit 2.

**Acceptance:**
- `cargo test -p camel-integration-test --lib` passes (feature off:
  load errors fire).
- `cargo test -p camel-integration-test --lib --features surreal`
  passes.
- `cargo clippy -p camel-integration-test -- -D warnings` exits 0 in
  both configurations.

- [x] 1.1

#### Task 1.2: surreal validate target — expectation grammar and deadline validity

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/document/validate.rs`
  (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)

**Steps:**
1. Add the surreal target to `document/validate.rs`: a
   `ScenarioTarget::Surreal(SurrealTarget)` variant where `pub
   struct SurrealTarget { pub datasource: String, pub query: String
   }`, reusing the existing `ValidateExpectation` grammar (rows
   tuples through the shared dual matcher, count bounds
   `count`/`atLeast`/`atMost`/range, `columns` projection list,
   `unordered` flag) — no new expectation type. Parse the raw twin in
   `document.rs` next to `RawSqlTarget` (line 438).
2. Load validation mirrors the sql target: query read gate
   (`is_surreal_read_statement` — anything else is a
   `doc-validation` error naming the action index, both feature
   configurations) plus the single-statement law (a `;` separator
   followed by further non-whitespace text is a `doc-validation`
   error — appended statements rejected); unknown
   target/expectation fields, `rows` mixed with a count bound,
   empty `rows`, empty or non-string `columns`, row-tuple length
   differing from the projection width, inverted range — all
   `doc-validation` errors naming the action index and offending
   field (and the row index for length mismatch). Surreal-specific
   deviation: `rows` without `columns` is a `doc-validation` error
   naming the action index (the driver returns key-sorted objects;
   projection order is not recoverable) — count bounds do not
   require `columns`.
3. Extend the deadline-validity check from "partner and sql" to
   "partner, sql, and surreal": the field doc comment
   (`document.rs:191` today) and the enforcement match
   (`document.rs:965-978`, error string at 978).
4. Ordered-rows advisory: an ordered `rows` expectation whose query
   lacks `ORDER BY` (case-insensitive substring) emits exactly one
   WARN naming the action index; `unordered: true` suppresses it —
   the sql loader rule, applied to the surreal target.

**Tests:** (load-level, both feature configurations)
- `surreal_mutation_query_is_load_error`: query `"DELETE user"` →
  `doc-validation` naming the action index.
- `surreal_appended_statement_is_load_error`: query `"SELECT * FROM
  user; CREATE x SET y = 1"` → `doc-validation` naming the action
  index (single-statement law).
- `surreal_rows_without_columns_is_load_error`: `rows` expectation
  with no `columns` → `doc-validation` naming the action index.
- `surreal_count_bound_without_columns_loads`: `atLeast: 1` with no
  `columns` → load succeeds (bounds do not need `columns`).
- `surreal_create_query_is_load_error`: query `"CREATE user SET name
  = 'x'"` → `doc-validation` naming the action index.
- `surreal_rows_mixed_with_count_is_load_error`: expectation with
  `rows` and `count: 1` → `doc-validation` naming the expectation
  node and action index.
- `surreal_row_length_mismatch_is_load_error`: `columns: [id, name]`
  with a 3-cell row tuple → `doc-validation` naming the row index.
- `surreal_ordered_rows_without_order_by_warns_once`: ordered rows
  with `columns` declared, query without `ORDER BY` → load succeeds
  with exactly one WARN naming the action index.
- `surreal_unordered_rows_without_order_by_no_warn`: same document
  with `unordered: true` → no WARN.
- `surreal_deadline_accepted`: validate surreal target with
  `deadline: 2s` → load succeeds.
- `surreal_target_parses`: (in `doc_parse_test.rs`) a `validate`
  action with `target: {surreal: {datasource: statedb, query:
  "SELECT * FROM user"}}` parses to the typed surreal target.
- `deadline_on_last_received_still_load_error`: (regression)
  `lastReceived` target with `deadline` → `doc-validation` (the
  MODIFIED Partner request verification contract holds).

**Acceptance:**
- `cargo test -p camel-integration-test --lib` and `--lib --features
  surreal` both pass.
- All sql-family parse tests remain green (no expectation-grammar
  drift).

- [x] 1.2

#### Task 1.3: sql prepare-action spec-restore regression pins

**Files:**
- `crates/camel-integration-test/src/sql_action_test.rs` (modified)
- `crates/camel-integration-test/src/runner_test.rs` (modified)

**Steps:**
1. Audit the restored "State prepare actions" sql scenarios against
   existing tests: the seeds/proceeds e2e, the select-prefixed and
   empty-list load errors, the statement-failure redaction, the
   demand-gate load error (in `sql_action_test.rs`,
   `doc_parse_test.rs`, `runner_test.rs`), the unknown-datasource
   fail-closed, and the single-catalog invariant.
2. Add any missing pin by name:
   - `sql_with_prefix_load_error` (if the `with`-prefixed statement
     load error has no direct test): item 0 `"with cte as (select 1)
     select * from cte"` → `doc-validation` naming action and
     statement index 0.
   - `sql_unknown_datasource_fails_closed` (if unpinned): action
     naming a datasource absent from the booted config → failure
     names the missing name only.
   - `sql_empty_prepare_list_load_error` (if unpinned): empty
     `prepare` → `doc-validation` naming the action index.
3. Pin the single-catalog invariant unconditionally: assert the
   executor resolves the pool through the passed catalog only —
   extend `sql_prepare_seeds_and_proceeds` (or add
   `sql_single_catalog_invariant`) to assert one pool handle per
   datasource name across two actions in one document.

**Tests:**
- `sql_with_prefix_load_error`: as above → Err naming indices.
- existing `sql_prepare_seeds_and_proceeds` re-run green.

**Acceptance:**
- `cargo test -p camel-integration-test --lib` passes with every
  restored sql scenario pinned by a named test.

- [x] 1.3

## Phase 2: Executor, projection, and hermetic mem tier

### Workspace and camel-component-surrealdb

#### Task 2.1: embedded `mem` engine — workspace feature and factory scheme

**Files:**
- `Cargo.toml` (modified — workspace `surrealdb` dependency)
- `crates/components/camel-component-surrealdb/src/pool_factory.rs`
  (modified)

**Steps:**
1. In the workspace `Cargo.toml` `surrealdb` dependency (line 165),
   add `"kv-mem"` to the feature list (alongside `protocol-ws`,
   `protocol-http`, `rustls`).
2. In `SurrealDbPoolFactory::supported_schemes` (line 176), add
   `"mem"` to the scheme slice.
3. Split the factory's auth contract by scheme
   (`pool_factory.rs:89-134`): today `namespace`, `database`,
   `username`, and `password` are mandatory extras (`extra_str`
   errors when missing) and `create()` runs an unconditional
   `signin(Root)` — a fresh `mem://` instance has no root user, so
   that path fails. For the `mem` scheme: skip the `signin` step
   entirely, make `username`/`password` unused, and make
   `namespace`/`database` optional extras defaulting to `test` /
   `test` (the `use_ns`/`use_db` calls still run, with the
   defaults). Remote schemes keep the mandatory extras and the
   signin step unchanged.

**Tests:** (in `pool_factory.rs` `mod tests`)
- `factory_supports_mem_scheme`: `supported_schemes()` contains
  `"mem"`.
- `mem_connect_without_credentials`: a `DatasourceConfig` with
  `db_url = "mem://"` and no `username`/`password`/`namespace`/
  `database` extras → `create` succeeds (no signin attempted) and
  the client answers a query.
- `mem_connect_creates_isolated_client`: two sequential
  `SurrealDbPoolFactory::create` calls over a `DatasourceConfig`
  with `db_url = "mem://"` produce two handles whose downcasts are
  distinct `Surreal<SurrealAny>` clients, and a record created
  through the first is invisible to the second.
- `remote_scheme_still_requires_credentials`: (regression) a `ws://`
  config missing `username` → the same missing-extra error as
  before this change.

**Acceptance:**
- `cargo test -p camel-component-surrealdb --lib` passes.
- `cargo clippy -p camel-component-surrealdb --all-targets --
  -D warnings` exits 0.

- [x] 2.1

### camel-integration-test (executor)

#### Task 2.2: surreal prepare executor over the catalog

**Files:**
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/src/surreal_action.rs` (modified)
- `crates/camel-integration-test/src/runner_test.rs` (modified)

**Steps:**
1. Add `pub async fn execute_surreal_prepare(catalog:
   &Arc<dyn DatasourceCatalog>, action: &SurrealAction) ->
   Result<(), String>` in `surreal_action.rs` (behind
   `#[cfg(feature = "surreal")]`) — the sql prepare executor's
   signature (`sql_action.rs:156-164`), not CamelError: resolve the
   datasource by name through the catalog, downcast the handle to
   `Surreal<SurrealAny>` (the `check` precedent in
   `pool_factory.rs:161-171`), execute each `prepare` statement in
   order through the SDK query path, check each response for
   statement errors before discarding results (the driver reports
   statement errors inside the response object — await alone does
   not establish success), stop at the first failure. The runner
   arm wraps the Err String into the action failure exactly as the
   sql arm does (`runner.rs:662-667`).
2. Sanitize failure text with the existing
   `sql_action::sanitize_db_error` pattern (replace the resolved
   `db_url` occurrences with `[REDACTED]`); diagnostics carry the
   datasource name and failing statement index, never the `db_url`
   or record values.
3. Wire the `Surreal` arm in `runner.rs` (behind
   `#[cfg(feature = "surreal")]`) next to the sql arm at line 662:
   `None` catalog fails closed with the same message shape as the
   sql arm (runner.rs:648-653); a missing datasource name fails
   naming the name only.

**Tests:** (in `runner_test.rs`, feature `surreal`)
- `surreal_prepare_seeds_and_proceeds`: a Camel.toml with
  `[datasources.statedb] provider = "surrealdb" db_url = "mem://"`,
  a `surreal:` action with `["DEFINE TABLE user SCHEMALESS",
  "CREATE user SET name = 'alice'"]`, then a `validate` surreal
  target `SELECT name FROM user` with `columns: [name]` and
  `rows: [["alice"]]` → the document passes.
- `surreal_statement_failure_stops_and_redacts`: second statement
  fails (malformed SurrealQL), datasource `db_url` carries a sentinel
  query parameter, first statement seeded a record with a sentinel
  field value → failure names `statedb` and statement index 1,
  sanitized text contains neither sentinel.
- `surreal_unknown_datasource_fails_closed`: action names `nodb` →
  failure names `nodb` only.
- `surreal_no_catalog_fails_closed`: surreal action through the
  single-action loop (no catalog) → fails closed naming the missing
  catalog.
- `surreal_single_catalog_invariant`: a document with two `surreal:`
  actions over the same datasource name → both resolve through one
  client handle per name (the catalog's), mirroring the sql pin in
  Task 1.3.

**Acceptance:**
- `cargo test -p camel-integration-test --lib --features surreal`
  passes.
- `cargo clippy -p camel-integration-test --features surreal --
  -D warnings` exits 0.

- [x] 2.2

#### Task 2.3: surreal validate executor — record projection into matcher tuples

**Files:**
- `crates/camel-integration-test/src/runner/surreal_validate.rs`
  (new)
- `crates/camel-integration-test/src/runner/surreal_validate_test.rs`
  (new)
- `crates/camel-integration-test/src/runner.rs` (modified — module
  registration and the validate dispatch arm; the runner dir has no
  mod.rs, submodules declare in `runner.rs`)

**Steps:**
1. Implement `pub(crate) fn surreal_value_to_cell(v:
   &surrealdb::Value, field: &str) -> Result<camel_api::Value,
   String>`: None and Null → `Value::Null`; Bool → boolean; Number →
   number when Int or finite Float (decimal or non-finite → Err
   naming the field and kind); String → string; Uuid → its string
   form; Datetime → its string form; RecordId → the `table:key`
   string; Object and Array → the structured `camel_api::Value`
   (matcher verbs see them whole), built recursively through
   `surreal_value_to_cell` so a nested unsupported kind fails
   closed naming the field path; every other kind (Bytes, Duration,
   Geometry, Table, File, Range, Regex, Set) → Err naming the field
   and the SurrealQL kind (fail closed, never a silent null).
2. Implement `pub(crate) fn surreal_rows_to_tuples(rows:
   Vec<surrealdb::Value>, fields: &[String]) ->
   Result<Vec<Vec<camel_api::Value>>, String>`: for each result
   object, project the listed fields in declared order through
   `surreal_value_to_cell`; an absent field → Err naming the field
   (unknown projection field fails closed). The loader guarantees
   `columns` is present whenever `rows` is declared (Task 1.2 load
   error), so this function only runs with a declared projection —
   count-bound-only expectations skip projection entirely.
3. Implement `pub(crate) async fn surreal_validate_action(index:
   usize, target: &SurrealTarget, expected: &RowsExpectation,
   deadline: Option<Duration>, datasource_catalog:
   Option<&Arc<dyn DatasourceCatalog>>) -> Result<(), ScenarioFailure>`
   following `sql_validate.rs` — the sql validate seam's shapes
   (`RowsExpectation` in, `ScenarioFailure` out,
   `runner/sql_validate.rs:48-54`), with the bound-only branch
   sharing the sql decision code and error classes: resolve the
   datasource through the catalog, execute the read, check the
   response for statement errors before projection, project, and
   hand the tuples and bounds to the shared expectation matcher the
   sql target uses.
4. Wire the surreal target arm in `runner.rs` validate dispatch
   (line 981) mirroring the sql arm's gating shape: the dispatch arm
   itself stays ungated (the sql arm at `runner.rs:980-981` is
   ungated); the `#[cfg(feature = "surreal")]` split lives inside
   `surreal_validate.rs`, whose feature-off arm returns the named
   demand-gate error naming the `surreal` feature at action time —
   the same split `sql_validate.rs` uses.

**Tests:** (in `surreal_validate_test.rs`, feature `surreal`,
`mem://` datasource)
- `ordered_rows_pass_immediately`: seed `CREATE user SET num = 1,
  name = 'alice'` and `num = 2, name = 'bob'` (user-defined numeric
  field — record ids project as `table:key` strings), select with
  `ORDER BY num`, `columns: [num, name]`, `rows: [[1, "alice"], [2,
  "bob"]]` → pass.
- `unordered_rows_match_reorder`: same two records, no `ORDER BY`,
  `unordered: true`, `columns: [num, name]`, `rows: [[2, "bob"],
  [1, "alice"]]` → pass.
- `wildcard_ignore_matches_any_cell`: one record `CREATE user SET
  num = 7, name = NULL` (a stored null — if the driver drops the
  key instead of persisting explicit NULL, seed via `CREATE user
  content { num: 7, name: null }`; do NOT use `NONE`, which omits
  the field), `columns: [num, name]`, `rows: [[{ignore: null},
  {equals: null}]]` → pass (stored-null mapping; the missing-field
  fail-closed is pinned separately by
  `unknown_projection_field_fails_closed`).
- `count_bound_passes_on_record_count`: three records, `atLeast: 2`
  → pass.
- `field_projection_reorders_by_name`: query selects `id, name`,
  `columns: [name, id]`, `rows: [["alice", "user:1"]]` → pass.
- `record_id_projects_as_string`: one record, query `SELECT id FROM
  user`, `columns: [id]`, `rows: [["user:1"]]` → pass (the
  `table:key` string form).
- `unknown_projection_field_fails_closed`: `columns: [id, missing]`
  → action fails naming `missing`.
- `unknown_value_kind_fails_closed`: a field holding a value kind
  outside the mapping (insert a geometry or bytes payload) → action
  fails naming the field and its kind.
- `nested_unsupported_kind_fails_closed`: a field holding an array
  that contains a geometry value → action fails naming the field
  path and the nested kind (recursion never launders unsupported
  kinds).
- `surreal_validate_feature_off_names_gate`: (run in a build without
  the `surreal` feature; a document whose surreal target passes
  load, e.g. a well-formed read query) the action runs and fails
  naming the `surreal` feature.
- `surreal_validate_no_catalog_fails_closed`: a surreal validate
  action through the single-action loop (no catalog) → fails closed
  naming the missing catalog (the validate-side mirror of the
  2.2 prepare pin).
- `row_length_mismatch_load_error_sibling`: expectation row tuple of
  3 cells against 2 columns → load error (pins Task 1.2 at e2e
  level).

**Acceptance:**
- `cargo test -p camel-integration-test --lib --features surreal`
  passes.
- `cargo fmt --check --all` exits 0.

- [x] 2.3

#### Task 2.4: poll semantics, deadline, and redaction for surreal targets

**Files:**
- `crates/camel-integration-test/src/runner/surreal_validate.rs`
  (modified)
- `crates/camel-integration-test/src/runner/surreal_validate_test.rs`
  (modified)

**Steps:**
1. Reuse the sql poll loop exactly: no early settle (record sets
   are not monotone); without a deadline one immediate snapshot
   decides; with a deadline poll at the fixed interval, fail
   immediately on a snapshot above the bound ceiling (`atMost`
   breach or above a range maximum), otherwise decide on the final
   snapshot.
2. Mismatch and driver-failure diagnostics follow the sql redaction
   shape: datasource name, rendered bound or expected row count,
   actual row count, projection field names; never the resolved
   `db_url` (sanitize via the `sanitize_db_error` pattern) and never
   an actual cell value.

**Tests:** (feature `surreal`, `mem://`)
- `deadline_poll_passes_when_record_appears`: empty table at start;
  spawn the validate action (deadline comfortably longer than the
  write delay) and a writer task (`tokio::spawn`, the sql suite's
  sanctioned sleep idiom: write at ~150ms via
  `execute_surreal_prepare(["CREATE user SET ..."])` over the same
  catalog); AWAIT the writer's completion (join) before awaiting
  the validate result — the join proves the write landed inside the
  deadline window; the validate (deadline, `columns`, the rows the
  spawn writes) passes on the final snapshot. Deterministic: first
  poll is immediate (record absent), write at 150ms, decision at
  expiry. No route, no boot — route-driven coverage is Task 2.6.
- `no_early_settle_final_snapshot_decides`: seed the record, PROVE
  presence with a preliminary immediate call (a direct
  `surreal_validate_action` without `deadline`, asserting the
  record is observed); then spawn the deadline validate and a
  deleter (`execute_surreal_prepare(["DELETE user"])` at ~150ms);
  AWAIT the deleter's join before awaiting the validate result
  (proves deletion landed inside the window); the `rows` +
  `columns` + `deadline` validate fails as validation mismatch on
  the final empty snapshot — the test outcome is independent of
  spawn scheduling.
- `ceiling_breach_fails_immediately`: two records present,
  `atMost: 1` with `deadline` → first snapshot fails without waiting.
- `mismatch_detail_elides_cells_and_db_url`: failing rows assertion,
  `db_url` with an embedded credential sentinel → detail names
  datasource, expected/actual counts, field names; contains neither
  the credential nor any cell value.
- `driver_error_is_sanitized`: query with a guaranteed statement
  error on `mem://` — `SELECT FROM WHERE` (SurrealQL parse error;
  passes the load-time select-prefix gate, errors in the response)
  → failure carries the datasource name and sanitized driver text,
  never the `db_url`.

**Acceptance:**
- `cargo test -p camel-integration-test --lib --features surreal`
  passes.
- No new `unwrap()` (cargo xtask lint-unwrap exits 0).

- [x] 2.4

#### Task 2.5: teardown and per-boot freshness for mem surreal datasources

**Files:**
- `crates/camel-integration-test/src/boot_scenario_test.rs`
  (modified)
- `crates/components/camel-component-surrealdb/src/pool_factory.rs`
  (modified — the close hook gains a mandatory `invalidate()` call)

**Steps:**
1. Wire the factory close hook: the surreal `PoolFactory` close path
   SHALL call `Surreal::<SurrealAny>::invalidate()` on the client
   (auth revocation — the SDK's only teardown lever; the catalog's
   `close_all` does NOT drop its cached entry,
   `datasource.rs:140-189`). NOTE: embedded `mem://` runs with
   authentication disabled (surrealdb 3.2.4 local engine) —
   `invalidate()` on an auth-free mem client revokes nothing
   observable, so the close-hook invocation is verified in the
   component crate, not through post-close query failure.
2. Add the freshness scenario tests (sequential boots in one
   process).

**Tests:**
- (in `pool_factory.rs` `mod tests`, camel-component-surrealdb)
  `close_hook_invalidates_and_completes`: create a mem client via
  the factory, invoke the close path, assert it completes Ok (the
  `invalidate()` call ran — on the auth-free mem tier its effect is
  hygiene; observability is the remote tier's concern, exercised by
  the existing remote component tests).
- (in `boot_scenario_test.rs`, feature `surreal`)
  `mem_surreal_second_boot_starts_empty`: first boot seeds records
  over `[datasources.statedb] db_url = "mem://"` with a `surreal:`
  prepare and completes teardown (close_all Ok); second boot over
  the same alias reads zero records (fresh catalog, new client —
  freshness by construction).

**Acceptance:**
- `cargo test -p camel-integration-test --lib --features surreal`
  passes; existing teardown and boot-freshness tests stay green
  (sql scenarios unaffected).

- [x] 2.5

#### Task 2.6: hermetic e2e — prepare, route write, validate, in one document

**Files:**
- `crates/camel-integration-test/tests/surreal_state_test.rs` (new)
- `crates/camel-integration-test/tests/fixtures/` (modified — new
  fixture document)

**Steps:**
1. Add an e2e scenario fixture: Camel.toml with the mem surreal
   datasource; a route file with a `direct:` consumer feeding a
   `surrealdb:create?datasource=statedb&table=order` producer; a
   scenario document that (a) prepares with `surreal:`
   (`REMOVE TABLE order` first — the clean-first idiom, tolerant of
   absence — then `DEFINE TABLE order SCHEMALESS`), (b) sends one
   `direct:` message the route persists, (c) validates the record
   with a surreal target, `columns`, and a `deadline`.
2. The test boots the document through the scenario entry point
   `boot_scenario` in-crate: `tests/surreal_state_test.rs` carries
   `#![cfg(feature = "surreal")]` and mirrors the
   `wasm_boot_test.rs` pattern (the crate has no separate e2e
   feature; the sql full-boot e2e lives in camel-cli and is owned
   by the integration-sql workflow).

**Tests:**
- `surreal_state_e2e_prepare_route_validate`: the fixture document
  runs to a passing verdict.

**Acceptance:**
- `cargo test -p camel-integration-test --features surreal --test
  surreal_state_test` passes; default `cargo test -p
  camel-integration-test --lib` stays hermetic and green.

- [x] 2.6

## Phase 3: Gate forwarding, CLI, CI, docs

#### Task 3.1: feature independence and gate-forwarding lint

**Files:**
- `crates/camel-integration-test/Cargo.toml` (modified — only if
  fix-up is needed after Task 1.1)

**Steps:**
1. Run `cargo xtask lint-gate-forwarding` and
   `cargo xtask lint-component-deps`; fix any violation (the
   `surreal` feature must forward `camel-bundles/surrealdb`).
2. Prove standalone compile:
   `cargo check -p camel-integration-test --no-default-features
   --features surreal` exits 0 without `http` or `sql`.

**Tests:**
- `surreal_feature_compiles_alone`: the check command above —
  expected exit 0 (a command-level criterion, recorded in CI by Task
  3.2's job).

**Acceptance:**
- `cargo xtask lint-gate-forwarding` exits 0.
- `cargo xtask lint-component-deps` exits 0.
- The standalone check exits 0.

- [x] 3.1

#### Task 3.2: CLI gate and CI independence job

**Files:**
- `crates/camel-cli/Cargo.toml` (modified)
- `crates/camel-cli/src/commands/test/scenario.rs` (modified)
- `.github/workflows/integration-surreal.yml` (new)

**Steps:**
1. Add the `integration-surreal` feature to `camel-cli/Cargo.toml`
   mirroring `integration-sql`: it enables the camel-test /
   integration-test surreal wiring and the `surrealdb` component
   feature. Add it EVERYWHERE `integration-sql` rides, adjusted for
   the BUSL rule: `full` (line 152) and `flavor-full` (line 175 —
   `flavor-full` does NOT include `full`, so it needs its own
   entry; it already carries `surrealdb`). Keep it OUT of
   `flavor-regular` (BUSL-1.1: `integration-surreal` forwards the
   BUSL-licensed surrealdb engine, which rides `flavor-full` only —
   `Cargo.toml:174-175`).
2. Wire the surreal full-boot selection in
   `camel-cli/src/commands/test/scenario.rs`: `run_scenario_doc`
   (~276-318) today selects a full boot (booted catalog) only for
   HTTP wiring or a declared `sql:` action — extend the condition
   so a document declaring a `surreal:` action or a surreal
   validate target also takes the full-boot path. Every gate that
   carries the http/sql boot condition gets the surreal arm:
   `BOOT_SCHEMES` (~27-49), `run_scenario_full_boot` and the
   dispatch (~281-318), and the config selection at ~435, each
   behind `#[cfg(feature = "integration-surreal")]` mirroring the
   sql arm's gate shape. Without this, a surreal-only document
   takes the no-boot path and has no catalog (the exact failure
   `surreal_no_catalog_fails_closed` pins at unit level), or the
   standalone CLI build fails to compile the boot path.
3. Create `.github/workflows/integration-surreal.yml` mirroring
   `integration-sql.yml`'s path-filters list exactly
   (`integration-sql.yml:6-15`): `Cargo.toml`, `Cargo.lock`,
   `crates/components/camel-component-surrealdb/**`,
   `crates/camel-integration-test/**`, `crates/camel-bundles/**`,
   `crates/camel-cli/src/**`,
   `crates/camel-cli/tests/test_scenario_cli_e2e.rs`,
   `crates/camel-cli/Cargo.toml`, and the workflow file itself;
   the job builds `camel-cli --no-default-features --features
   integration-surreal,itest-e2e` (independence check), then runs
   `cargo test -p camel-integration-test --features surreal --test
   surreal_state_test` (Task 2.6's suite — the crate has no e2e
   feature of its own); default suite untouched.

**Tests:**
- CI-level criterion: the job's build line compiles `camel-cli`
  without `integration-http` and `integration-sql` and the suite
  passes (no Docker required — the mem tier is embedded).

**Acceptance:**
- `cargo check -p camel-cli --no-default-features --features
  integration-surreal` exits 0 locally.
- `cargo check -p camel-cli --no-default-features --features
  flavor-full` exits 0 (the flavor closure carries
  `integration-surreal`).
- `cargo xtask lint-publish-cycles` and `lint-publish-registration`
  exit 0.

- [x] 3.2

#### Task 3.3: docs page, ADR ladder, context citations

**Files:**
- `docs/src/testing/scenario-surreal.md` (new)
- `docs/src/testing/index.md` (modified)
- `docs/src/SUMMARY.md` (modified)
- `docs/adr/0069-integration-tier-testing-contract.md` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)
- `crates/components/camel-component-surrealdb/CONTEXT.md`
  (modified)

**Steps:**
1. Write `scenario-surreal.md` mirroring `scenario-sql.md`: the
   `surreal:` prepare action, the validate surreal target, the
   record-id projection rule, the mem convention
   (`db_url = "mem://"`, `provider = "surrealdb"`), the remote
   steering note (ws/http through `db_url`, clean-first idiom), and
   the isolation section (mem dies with the boot; remote state is
   durable). Link from `index.md` (beside the SQL page) and add the
   SUMMARY entry under Testing.
2. In ADR-0069 §8, add the surreal entry to the activation ladder:
   the demand signal (bd rc-25lup.5), the `surreal` feature, the
   `integration-surreal` CI job, the embedded mem tier (no Docker).
3. Update both CONTEXT.md files: the harness crate's scenario
   grammar section gains the surreal family; the component's
   scenario-tier note gains the `mem` scheme and the
   scenario-validate read path. Cite the spec
   (`openspec/specs/integration-tier`) per lint-context-citations.

**Tests:**
- `cargo xtask lint-context-citations` exits 0.
- `cargo xtask schema --check` exits 0.

**Acceptance:**
- Both lints exit 0; `mdbook build` (or the repo's doc build
  command) renders the new page without dead links.

- [x] 3.3
