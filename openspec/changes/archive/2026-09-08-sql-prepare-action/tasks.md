# Tasks: sql-prepare-action

## Phase 1 — Foundations behind the gate (no `document.rs` edits)

## Task 1 — Expose the booted datasource catalog through BootHandle

- **ID**: spa-1
- **Description**: Give the scenario runner the single catalog the
  composition root built, without changing `camel_bundles::boot`'s
  signature.

- **Files**:
  - `crates/camel-bundles/src/lib.rs` (modified)

- **Steps**:
  1. In `crates/camel-bundles/src/lib.rs`, add a private field
     `datasource_catalog: Arc<dyn DatasourceCatalog>` to `pub struct
     BootHandle` (currently `{ jms_pool, cxf_pool }` around line 71).
     Update the struct-literal construction site inside `camel_bundles`
     (around line 356) to carry the `datasource_catalog` value the `boot`
     function already builds around line 241.
  2. Add `pub fn datasource_catalog(&self) -> Arc<dyn DatasourceCatalog>`
     on `impl BootHandle` returning a clone of the field. Doc comment:
     single-catalog invariant — callers resolve pools through the same
     catalog the component cascade registered factories on; one datasource
     name, one pool.
  3. Add a unit test in the `mod tests` of `lib.rs` (follow the existing
     `booted_context` test helper pattern in that module): build a
     `CamelConfig` whose `datasources` map has one entry `appdb` with
     `db_url = "sqlite::memory:?cache=shared"`, run `camel_bundles::boot`
     against a prepared context, and assert the returned handle's
     `datasource_catalog().get_config("appdb").is_some()`.

- **Tests** (executable specs):
  - name: `boot_handle_exposes_datasource_catalog`
    - setup: camel-bundles test module; sqlite driver available through
      camel-sql (already a camel-bundles dependency).
    - action: boot with one configured `appdb` datasource, call
      `handle.datasource_catalog().get_config("appdb")`.
    - assert: returns `Some` with the configured `db_url`.
    - command: `cargo test -p camel-bundles --lib boot_handle_exposes_datasource_catalog`
    - expected: fails before the change (no accessor), passes after.

- **Acceptance**:
  - `cargo test -p camel-bundles --lib` passes.
  - `cargo clippy -p camel-bundles -- -D warnings` exits 0.

- [x] spa-1

## Task 2 — `sql_action.rs`: model, validation, executor, sanitizer

- **ID**: spa-2
- **Description**: The whole SQL action substance in a new module — raw
  serde shape, load-time validation, redaction sanitizer, and the
  feature-gated executor over the catalog. No `document.rs` edits.

- **Files**:
  - `crates/camel-integration-test/src/sql_action.rs` (new)
  - `crates/camel-integration-test/src/lib.rs` (modified: `mod sql_action;`
    and `pub use` of the public items listed below)

- **Steps**:
  1. In `sql_action.rs`, define `pub struct RawSqlAction` (serde,
     `deny_unknown_fields`, `rename_all = "camelCase"`) with fields
     `datasource: String` and `prepare: Vec<String>`, plus the typed twin
     `pub struct SqlAction { pub datasource: String, pub prepare:
     Vec<String> }` and `pub const SQL_ACTION_KEY: &str = "sql"`.
  2. Implement `pub fn is_read_statement(stmt: &str) -> bool`: loop —
     trim whitespace; if the result starts with `(`, drop that one
     character and repeat; otherwise break. Lowercase the remainder and
     return `true` when it starts with `select` or `with`. (Re-trimming
     inside the loop is what makes `"  ( select 1 )"` read-detected.)
  3. Implement `pub fn validate_sql_action(raw: &RawSqlAction, action_index:
     usize) -> Result<SqlAction, String>` returning owned error strings
     (the `document.rs` hook in Task 4 wraps them into `DocError`):
     - empty `prepare` → `"sql action {action_index}: prepare list must not
       be empty"`.
     - read statement at index `i` → `"sql action {action_index}: prepare
       statement {i} is a read (select/with prefix); reads belong to the
       validate sql target"`.
  4. Implement `pub fn sanitize_db_error(err_text: &str, db_url: &str) ->
     String`: replace every occurrence of `db_url` in `err_text` with
     `[REDACTED]`; when `db_url` is empty, return `err_text` unchanged.
  5. Implement the executor behind `#[cfg(feature = "sql")]`:
     `pub async fn execute_sql_prepare(catalog:
     &Arc<dyn DatasourceCatalog>, action: &SqlAction) -> Result<(),
     String>`:
     - resolve `catalog.get_config(&action.datasource)`; `None` →
       `Err(format!("sql action: unknown datasource '{}'",
       action.datasource))` — name only, nothing else.
     - `catalog.get_pool(&action.datasource).await` and downcast the
       handle to `sqlx::AnyPool`; failure → error naming the datasource
       and the downcast/pool error, sanitized with `sanitize_db_error`
       against the resolved config's `db_url`.
     - for each statement `i` in order: `sqlx::query(stmt).execute(&*pool)`
       — on error return
       `Err(format!("datasource '{}' statement [{}]: {}", action.datasource,
       i, sanitize_db_error(&e.to_string(), &config.db_url)))` and stop.
  6. Unit tests in `sql_action.rs` `mod tests` (run under default features
     — validation and sanitizer are ungated):
     - `read_statement_detection`: `is_read_statement` is `true` for
       `"SELECT 1"`, `"  ( select 1 )"`, and
       `"With x AS (SELECT 1) SELECT * FROM x"`; `false` for
       `"INSERT INTO t VALUES (1)"` and `"(CREATE TABLE t (x INTEGER))"`.
     - `validation_rejects_empty_prepare`: `validate_sql_action` with empty
       list errors naming the action index.
     - `validation_rejects_read_prefixes`: item 0 `"(SELECT 1)"` and,
       separately, item 1 `"with cte as (select 1) select * from cte"`
       produce errors naming statement indices 0 and 1 respectively.
     - `validation_accepts_mutations`: DDL + INSERT + DELETE pass and the
       typed action preserves statement order.
     - `sanitizer_redacts_db_url`:
       `sanitize_db_error("connect failed sqlite::memory:?cache=shared&x=1",
       "sqlite::memory:?cache=shared&x=1")` contains `[REDACTED]` and not
       `cache=shared&x=1`.
     - `sanitizer_empty_url_noop`: empty `db_url` returns input unchanged.
  7. Executor tests behind `#[cfg(all(test, feature = "sql"))]` using a
     `RuntimeDatasourceCatalog` built from a one-entry map (pattern:
     camel-bundles lib.rs line 240) plus a local stub `PoolFactory` whose
     `create` first calls `sqlx::any::install_default_drivers()` (without
     it AnyPool connect fails — precedent camel-sql pool_factory.rs:21)
     and then builds the pool with `AnyPoolOptions`; register the stub via
     the public `DatasourceCatalog::register_factory` trait method
     (camel-api datasource.rs:148). The point is catalog-through execution,
     not factory reuse:
     - `executor_seeds_and_stops_on_error`: statements `["CREATE TABLE t (v
       TEXT UNIQUE)", "INSERT INTO t VALUES ('a')", "INSERT INTO t VALUES
       ('a')"]` return `Err` naming `statement [2]`; the message contains
       the datasource name, contains no `db_url` substring.
     - `executor_unknown_datasource_names_it_only`: missing name errors
       with exactly the unknown-name message.
     - `executor_success_is_silent`: two valid statements return `Ok(())`.

- **Tests** (executable specs):
  - command: `cargo test -p camel-integration-test --lib sql_action` (default
    features — validation/sanitizer tests).
  - command: `cargo test -p camel-integration-test --lib --features sql
    sql_action` (executor tests).
  - expected: module missing before the change; all listed tests pass after.

- **Acceptance**:
  - `cargo test -p camel-integration-test --lib` passes (default features).
  - `cargo test -p camel-integration-test --lib --features sql` passes.
  - `cargo fmt --check -p camel-integration-test` clean.
  - `cargo clippy -p camel-integration-test --all-targets -- -D warnings`
    exits 0 both with and without `--features sql`.

- [x] spa-2

## Task 3 — Boot lint `sql-memory-not-shared`

- **ID**: spa-3
- **Description**: Reject bare SQLite in-memory datasource URLs at scenario
  boot, ungated, right after the sealed config load.

- **Files**:
  - `crates/camel-integration-test/src/boot_scenario.rs` (modified)
  - `crates/camel-integration-test/src/sql_action.rs` (modified)
  - `crates/camel-integration-test/src/boot_scenario_test.rs` (modified)

- **Steps**:
  1. In `sql_action.rs` (from Task 2 — re-export from lib.rs), add
     `pub const SQL_MEMORY_NOT_SHARED: &str = "sql-memory-not-shared"` and
     `pub fn ensure_sqlite_memory_shared(config: &CamelConfig) -> Result<(),
     CamelError>` iterating `config.datasources` in BTreeMap order; for
     each entry whose `db_url` (lowercased) starts with `sqlite::memory:`
     or `sqlite://:memory:` and whose remainder lacks the substring
     `cache=shared`, return `Err(CamelError::Config(format!("{}:
     datasource '{}' uses a per-connection sqlite :memory: URL without
     cache=shared; INSERT and SELECT can hit different databases. Use
     sqlite::memory:?cache=shared", SQL_MEMORY_NOT_SHARED, name)))` —
     datasource NAME only, never the URL.
  2. In `boot_scenario.rs`, call `ensure_sqlite_memory_shared(&config)`
     immediately after `CamelConfig::from_file_sealed(...)` and its
     error mapping, before context preparation. Ungated: no `cfg`.
  3. Tests in `boot_scenario_test.rs` (the file's existing pattern):
     - `bare_memory_sqlite_rejected`: a temp root with a `Camel.toml`
       declaring `[datasources.appdb] db_url = "sqlite::memory:"` and a
       minimal `.test.yaml` scenario doc with NO `sql:` action;
       `boot_scenario` fails with the `sql-memory-not-shared` message
       naming `appdb`.
     - `shared_cache_memory_sqlite_passes_lint`: same doc with
       `db_url = "sqlite::memory:?cache=shared"` — boot proceeds past the
       lint (full boot may then succeed or fail later for unrelated
       reasons; assert the error, if any, is not `sql-memory-not-shared`).
     - `file_backed_sqlite_unaffected`: `db_url = "sqlite:file:/tmp/x.db"`
       — assert no `sql-memory-not-shared` error.
     - `lint_is_ungated`: marked `#[cfg(not(feature = "sql"))]`; asserts
       the bare-memory rejection still occurs in a build without the
       feature (runs in the default feature-less suite).

- **Tests** (executable specs):
  - command: `cargo test -p camel-integration-test --lib sqlite` (default
    features — matches the three lint tests; `lint_is_ungated` runs in the
    full default `--lib` suite, which CI always runs).
  - command: `cargo test -p camel-integration-test --lib --features sql
    sqlite` (first three tests under the feature).
  - expected: before the change, bare memory boots (or fails elsewhere);
    after, the named error appears in both feature configurations.

- **Acceptance**:
  - Both test commands pass.
  - `cargo clippy -p camel-integration-test --all-targets -- -D warnings`
    exits 0 with and without `--features sql`.

- [x] spa-3

## Phase 2 — Grammar wiring (HARD GATE: conductor has confirmed the
## document.rs sweep landed and this worktree rebased onto fresh main)

## Task 4 — Wire `sql:` into the grammar, runner, and features

- **ID**: spa-4
- **Description**: Minimal `document.rs` touch (enum variant + raw arm +
validation hook), runner dispatch, `sql` feature, `integration-sql`
CLI feature, and the end-to-end scenario tests.

- **Files**:
  - `crates/camel-integration-test/src/document.rs` (modified — MINIMAL)
  - `crates/camel-integration-test/src/runner.rs` (modified)
  - `crates/camel-integration-test/src/runner_test.rs` (modified)
  - `crates/camel-integration-test/src/doc_parse_test.rs` (modified)
  - `crates/camel-integration-test/tests/direct_reply_test.rs` (modified)
  - `crates/camel-integration-test/tests/http_inbound_test.rs` (modified)
  - `crates/camel-integration-test/tests/http_outbound_test.rs` (modified)
  - `crates/camel-integration-test/tests/http_partner_scripting_test.rs` (modified)
  - `crates/camel-integration-test/tests/partner_verification_test.rs` (modified)
  - `crates/camel-integration-test/Cargo.toml` (modified)
  - `crates/camel-cli/Cargo.toml` (modified)
  - `crates/camel-cli/src/commands/test/scenario.rs` (modified)
  - `crates/camel-cli/tests/test_scenario_cli_e2e.rs` (modified)

- **Steps**:
  1. `document.rs`: add `Sql { datasource: String, prepare: Vec<String> }`
     to `ScenarioAction` with a doc comment citing bd rc-25lup.1. In the
     raw serde stage, add the matching raw arm keyed by
     `sql_action::SQL_ACTION_KEY` using `RawSqlAction` from
     `sql_action.rs`. ORDERING MANDATE (inbound precedent,
     document.rs:899-937): the raw arm parses and
     `sql_action::validate_sql_action(&raw, index)` runs BEFORE any
     feature demand-gate check — a read-prefixed or empty `prepare` doc
     fails `doc-validation` naming the action index and statement index in
     BOTH feature configurations (spec scenarios 2, 3, 4). Map the error
     string into the existing `DocError::Validation` shape. Only after
     validation passes does `#[cfg(not(feature = "sql"))]` code reject the
     action with a named demand-gate error mirroring the
     `inbound:`-without-http behavior (exact mechanism at document.rs
     ~899-937 — copy the pattern).
  2. `document.rs` doc-comment header: extend the action enumeration
     sentence with `sql:` (one clause). The structural error strings that
     enumerate the action vocabulary (document.rs:961, :966, :1098 —
     "`send`, `receive`, `sleep`, `validate`") gain `sql` in the same
     edit.
  3. `runner.rs`: the dispatch site has no precedent carrying the boot
     handle — `run.ctx` never reaches `run_action` (context stimulus rides
     the router via `DirectStimulus`, runner.rs:367-371). Thread the
     catalog explicitly: add a `datasource_catalog: Option<&Arc<dyn
     DatasourceCatalog>>` parameter to `run_scenario_document` and
     `run_action` (public API — update every call site: the ~10 in
     `runner_test.rs` plus the integration-test suites
     `tests/direct_reply_test.rs` (1 call),
     `tests/http_inbound_test.rs` (4),
     `tests/http_outbound_test.rs` (6),
     `tests/http_partner_scripting_test.rs` (2),
     `tests/partner_verification_test.rs` (12), passing `None` at each
     except the new sql tests), sourced from
     `run.boot.datasource_catalog()` at the CLI call site
     (camel-cli/src/commands/test/scenario.rs:438 and :493). In the
     action dispatch (the `match` around line 420), add
     `ScenarioAction::Sql { datasource, prepare } =>` behind
     `#[cfg(feature = "sql")]`; construct
     `sql_action::SqlAction { datasource: datasource.clone(), prepare:
     prepare.clone() }` and call
     `sql_action::execute_sql_prepare(catalog, &action)`;
     under `#[cfg(not(feature = "sql"))]` the arm is unreachable (load
     already rejected the action) — emit the demand-gate error if the
     compiler requires the arm.
  4. `runner.rs` and `document.rs` exhaustiveness: `ScenarioAction::
     bindings()` (document.rs ~189) is an exhaustive match — add
     `Self::Sql { .. } => Vec::new()` (a `sql:` action declares no
     endpoint bindings).
   5. `crates/camel-integration-test/Cargo.toml`: add optional dependency
     `sqlx = { workspace = true, optional = true, features =
     ["runtime-tokio", "any", "sqlite", "chrono"] }` with a comment
     mirroring the `http` block, and extend `[features]` with
     `sql = ["dep:sqlx"]`.
   6. `crates/camel-cli/Cargo.toml`: add feature
     `integration-sql = ["camel-integration-test/sql"]` and add
     `"integration-sql"` to the default feature list beside
     `"integration-http"`. `integration-sql` MUST NOT require
     `integration-http` (a sql-only scenario boots without any http
     endpoint).
   7. CLI dispatch (`camel-cli/src/commands/test/scenario.rs:242-255`):
     today a `fake`-only or endpoint-less document stays on the no-boot
     smoke path (`BOOT_SCHEMES` at :26, dispatch at :255) and would never
     build the datasource catalog. Route a document that carries any
     `sql:` action to `run_scenario_full_boot` — extend the full-boot
     predicate beside the scheme check. `run_scenario_full_boot` itself
     (:370) is today gated only by `integration-http` and unconditionally
     calls HTTP-only `bind_partners`: re-gate the function
     `any(all(integration-http), all(integration-sql))` style — i.e. it
     compiles when EITHER feature is on — and isolate the HTTP partner
     setup (`bind_partners` and friends) behind `integration-http` alone
     so an `integration-sql`-only build links. Without any sql feature
     the document-level demand gate already rejects the document at
     load, so the smoke path never sees a `sql:` action. Update the
     `PROVIDED_ADAPTERS` capability strings (:32-34) to mention the sql
     action when `integration-sql` is compiled.
   8. End-to-end tests in `crates/camel-integration-test/src/runner_test.rs`
     (follow the existing scenario-doc test helpers; all behind
     `#[cfg(feature = "sql")]`):
     - `sql_prepare_seeds_and_proceeds`: temp root, Camel.toml with
       `[datasources.appdb] db_url = "sqlite::memory:?cache=shared"`, doc
       with one `sql:` action (`CREATE TABLE t (v TEXT)` + `INSERT INTO t
       VALUES ('seed')`); run the scenario; assert success; then through
       `handle.datasource_catalog()` get the pool and
       `sqlx::query("SELECT COUNT(*) AS n FROM t")` (test-side read is
       fine — it is not a scenario statement) assert count 1. This also
       pins the single-catalog invariant: the rows the action wrote are
       visible through the booted catalog.
     - `sql_read_statement_is_load_error`: doc whose `sql:` `prepare`
       item 0 is `"(SELECT 1)"`; load fails `doc-validation` naming the
       action index and statement index 0.
     - `sql_with_statement_is_load_error`: item 0 is the CTE read from
       Task 2's test; same assertion.
     - `sql_empty_prepare_is_load_error`: empty list; load error naming
       the action index.
     - `sql_failure_redacts_and_stops`: statements
       `["CREATE TABLE t (v TEXT UNIQUE)", "INSERT INTO t VALUES
       ('LEAKROW7')", "INSERT INTO t VALUES ('LEAKROW7')"]`; the run
       fails naming `statement [2]`; the diagnostic contains neither the
       configured `db_url` string nor `LEAKROW7`.
     - `sql_unknown_datasource_fails_closed`: action naming `nosuch`;
       failure names `nosuch` and nothing about URLs.
  9. Feature-off tests (default suite, `#[cfg(not(feature = "sql"))]` in
     `doc_parse_test.rs`): `sql_action_without_feature_is_named_error` —
     a doc with a valid `sql:` action fails load with the demand-gate
     error naming `sql`; `sql_read_rejected_without_feature` — a doc
     whose `sql:` `prepare` item 0 is `"(SELECT 1)"` fails
     `doc-validation` naming the action index and statement index 0;
     `sql_with_rejected_without_feature` — item 0 is the CTE read,
     same assertion shape; `sql_empty_prepare_rejected_without_feature`
     — empty list, load error naming the action index. Together these
     pin the feature-off halves of spec scenarios 2, 3, and 4
     (validation precedes the demand gate).
  10. CLI-path test (`crates/camel-cli/tests/test_scenario_cli_e2e.rs`,
      behind the existing `itest-e2e` feature gate, following that
      suite's spawn-the-binary pattern): `sql_only_doc_boots_full` — a
      temp project with a `sqlite::memory:?cache=shared` datasource, a
      route file with one `log:` route (endpoint-less), and a scenario
      doc whose only action is `sql:` seeding one table; `camel test`
      exits 0. This pins step 7's dispatch: without the routing fix the
      document takes the no-boot smoke path and fails.

- **Tests** (executable specs):
  - command: `cargo test -p camel-integration-test --lib --features sql
    runner` (e2e set).
  - command: `cargo test -p camel-integration-test --lib` (default —
    feature-off load error).
  - expected: before Task 4, `sql:` documents fail as unknown fields;
    after, the named behaviors hold.

- **Acceptance**:
  - Both test commands pass.
  - `cargo test -p camel-cli --no-default-features --features
    integration-sql,itest-e2e --test test_scenario_cli_e2e
    sql_only_doc_boots_full` passes — proving `integration-sql` links
    and runs WITHOUT `integration-http`.
  - `cargo clippy -p camel-integration-test -p camel-cli --all-targets --
    -D warnings` exits 0 with and without `--features sql` (itest).
  - `cargo fmt --check --all` clean.
  - `document.rs` diff versus the rebased base touches only: the enum
    variant, the `bindings()` arm (`Self::Sql { .. } => Vec::new()`), the
    raw serde arm, the validation hook, the doc-comment clause, the three
    action-vocabulary error strings (document.rs:961, :966, :1098), and
    (if required by the inbound pattern) the feature-off named-error
    branch. Anything beyond that list is a deviation.
  - `rg -n 'interpolate' crates/camel-integration-test/src/sql_action.rs`
    returns no matches (identifier law: no env interpolation of the
    datasource name or statements).

- [x] spa-4
