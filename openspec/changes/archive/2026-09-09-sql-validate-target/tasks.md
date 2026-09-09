# Tasks: sql-validate-target

All commands run from the feature worktree root. Tests marked
`--features sql` run only in the sql-enabled configuration; grammar
tests run in both. The dispatch order is fixed at SEVEN tasks:
1.1 → 1.2 → 2.1 → 2.2 → 3.1 → 3.2 → 3.3.

## Phase 1: matcher algebra

### Task 1.1: `Expectation::Any` wildcard verb in camel-matchers

**Files:**
- `crates/camel-matchers/src/lib.rs` (modified)

**Steps:**
1. Add variant `Any` to the `#[non_exhaustive]` `Expectation` enum with
   doc-comment: matches any value including null; the wildcard verb
   (`ignore` at the grammar layer, the Citrus `@ignore@` equivalent).
2. Add the `Expectation::Any => true` arm to `expectation_matches`.
3. Extend the `settles_early` doc-comment with the non-monotone
   caveat: early-settle soundness assumes a monotone subject (arrivals
   only add); SQL row sets are NOT monotone (a DELETE shrinks them),
   so SQL validation must not settle early — the final snapshot at
   deadline decides (papal e_opus, bd rc-25lup.2, 2026-09-09).

**Tests:**
- `any_matches_all_values_including_null`:
    `expectation_matches(&Expectation::Any, v)` is true for
    `json!(null)`, `json!(0)`, `json!("x")`, `json!([1,2])`,
    `json!({"k":"v"})` → `cargo test -p camel-matchers --lib any_matches`
    (fails before: variant does not exist).
- `any_distinct_from_exists`:
    `expectation_matches(&Expectation::Exists, &json!(null))` is false
    while `Any` is true → same command.

**Acceptance:**
- `cargo test -p camel-matchers --lib` passes.
- `cargo clippy -p camel-matchers -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: `RowsExpectation` type + pure `rows_match`

**Files:**
- `crates/camel-matchers/src/lib.rs` (modified)

**Steps:**
1. Add `pub struct RowsExpectation { pub columns:
   Option<Vec<String>>, pub unordered: bool, pub rows:
   Option<Vec<Vec<Expectation>>>, pub bound: Option<CountBound> }`
   with doc-comment: exactly one row shape is populated at parse time
   (`rows` XOR `bound`); `columns` projects by name at the call site
   (ADR-0072 §3 — the algebra is parameterized, observation is
   per-tier).
2. Add `pub fn rows_match(expected: &[Vec<Expectation>], actual:
   &[Vec<serde_json::Value>], unordered: bool) -> bool`. Ordered:
   lengths equal AND every cell satisfies `expectation_matches`
   positionally. Unordered: lengths equal AND a perfect matching
   exists between expected patterns and actual rows — implement with
   Kuhn's augmenting-path bipartite matching over the boolean
   cell-compatibility matrix (`row_pattern_matches` helper: same
   length AND all cells `expectation_matches`); iterate expected rows
   in declaration order so the result is deterministic; never
   factorial backtracking.
3. Extend the `CountBound` doc-comment: the bound semantics document
   monotone subjects; SQL row counts are non-monotone — SQL count
   assertions never settle early.

**Tests:**
- `rows_match_ordered_positional`:
    `expected [[Equals(1), Contains("li")], [Equals(2), Any]]` vs
    actual `[[1,"alice"],[2,"bob"]]` (unordered=false) is true;
    swapping the actual rows makes it false →
    `cargo test -p camel-matchers --lib rows_match`.
- `rows_match_length_mismatch_fails`:
    2 expected vs 1 actual (both modes) is false.
- `rows_match_unordered_reorder`:
    same expected, actual reversed, unordered=true is true.
- `rows_match_unordered_duplicates`:
    expected `[[Equals(1)],[Equals(1)]]` vs actual `[[1],[1]]` true;
    vs `[[1],[2]]` false.
- `rows_match_kuhn_needs_augmenting`:
    expected `[[Any, Any], [Equals(json!(1)), Any]]` vs actual
    `[[1,"x"],[2,"a"]]` unordered — pattern 1 matches both rows,
    pattern 2 only `[1,"x"]`: greedy first-fit assigns pattern 1 to
    `[1,"x"]` and strands pattern 2; the augmenting path reassigns
    pattern 1 to `[2,"a"]`; assert true.
- `rows_match_wildcard_including_null_cells`:
    expected `[[Any]]` vs actual `[[null]]` true in both modes.

**Acceptance:**
- `cargo test -p camel-matchers --lib` passes.
- `cargo clippy -p camel-matchers -- -D warnings` exits 0.

- [x] 1.2

## Phase 2: document grammar

### Task 2.1: `ignore` verb + `ValidateExpectation::Rows` variant (compilable standalone)

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)

**Steps:**
1. Add `"ignore"` to `is_matcher_key`; in `expectation_from_value`,
   the `"ignore"` arm accepts ONLY a null payload (mirror the
   `exists` law: non-null → `{field}: \`ignore\` takes no argument`)
   and returns `Expectation::Any`.
2. Add variant `Rows(RowsExpectation)` to `ValidateExpectation`
   (doc-comment: the sql-target row shape; `Message` and `Partner`
   unchanged). The pub variant carrying the pub `RowsExpectation`
   from camel-matchers compiles clean standalone — no dead code.

**Tests (doc_parse_test.rs, ungated — run without features):**
- `ignore_verb_parses_to_any`: `expectation_from_value`-level:
  `{ignore: null}` yields `Expectation::Any` (shared dual grammar,
  same verb set as `exists`) → `cargo test -p
  camel-integration-test --lib ignore_verb`.
- `ignore_takes_no_argument`: cell `{ignore: 5}` → error with
  "`ignore` takes no argument".
- `ignore_matches_any_cell`: `expectation_matches(&Expectation::Any,
  v)` true for `json!(null)`, `json!(3)`, `json!("x")` (pins the
  verb's runtime meaning at the grammar layer).

**Acceptance:**
- `cargo test -p camel-integration-test --lib` (no features) and
  `--features sql` both pass.
- `cargo clippy -p camel-integration-test -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: `SqlTarget` wiring + sql `expectation` grammar + ORDER-BY WARN

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)

**Steps:**
1. Add `pub struct SqlTarget { pub datasource: String, pub query:
   String }` (doc-comments: the identifier law — datasource is a
   configured name, never interpolated; the query is doc-authored
   read text) and variant `ScenarioTarget::Sql(SqlTarget)` after
   `Partner`.
2. In `ScenarioAction::bindings`, the `ScenarioTarget::Sql(_)` arm
   returns `Vec::new()` (a named datasource declares no endpoint
   binding) — mirroring the `Variable` arm.
3. Add `RawSqlTarget { datasource: String, query: String }`
   (`#[serde(deny_unknown_fields, rename_all = "camelCase")]`); in
   `build_target`, key `"sql"` deserializes it and rejects an empty
   `datasource` or `query` with the action-index `DocError`. Reject a
   query that fails `crate::sql_action::is_read_statement` with a
   doc-validation error naming the action index and stating reads
   belong here (the `sql:` prepare action owns mutations). Update
   `build_target`'s unknown-key and must-be-map error strings to
   include `sql` in the accepted-target list.
4. Widen the `deadline` gate in the `validate` parse arm: accepted
   when `matches!(target, ScenarioTarget::Partner(_) |
   ScenarioTarget::Sql(_))`; the error message for other targets now
   reads "only valid on a `partner` or `sql` validate target".
   `elapsed_at_least` stays `LastReceived`-only (its gate needs no
   edit; add a pinning test).
5. Add `fn sql_expectation_from_value(value: &Value, index: usize)
   -> Result<RowsExpectation, DocError>` parsing the sql-target
   `expectation` node, wired in the same task (no dead-code window):
   - recognized keys: `rows`, `columns`, `unordered`,
     `count`/`atLeast`/`atMost` (the partner count-key semantics:
     exactly one bound form, `atLeast <= atMost` for a range — reuse
     the partner error phrasing);
   - `rows`: a non-empty sequence of sequences; every cell parses via
     `expectation_from_value` (field name `rows`); when `columns` is
     declared, a row whose length differs from the columns length →
     error naming the action index AND the row index (when `columns`
     is absent, widths are checked at execution against the query's
     projection);
   - `columns`: a non-empty list of strings; duplicates rejected
     naming the duplicated column;
   - `unordered`: a boolean, default false;
   - `rows` and any count key together → exclusivity error naming
     both keys;
   - unknown key → error listing the recognized keys (the
     `backticked` helper);
   - result populates exactly one of `RowsExpectation.rows` /
   `.bound` (the other `None`).
6. Wire the pairing: the `ScenarioTarget::Sql(_)` branch of the
   expectation match calls `sql_expectation_from_value` and wraps
   into `ValidateExpectation::Rows(..)`.
7. Add `fn sql_query_lacks_order_by(query: &str) -> bool`
   (case-insensitive regex `(?i)\border\s+by\b` — the crate already
   depends on `regex`; `\s+` covers `ORDER\nBY`); in the validate
   parse, when the shape is ordered (`!unordered`), `rows` is
   populated, and the predicate holds, emit exactly one
   `tracing::warn!` naming the action index and the nondeterminism.

**Tests (doc_parse_test.rs, ungated):**
- `sql_target_parses`: a validate action with
  `target: {sql: {datasource: appdb, query: "SELECT id FROM t ORDER
  BY id"}}`, `expectation: {rows: [[1]]}` parses to
  `ScenarioTarget::Sql` with both fields and
  `ValidateExpectation::Rows` → `cargo test -p
  camel-integration-test --lib sql_target_parses`.
- `mutation_query_is_load_error`: query `"DELETE FROM users"` →
  `DocError::Validation` naming action index 0 and the read rule.
- `with_prefix_query_accepts`: query `"with c as (select 1) select *
  from c"` parses.
- `paren_wrapped_select_accepts`: query `"(select 1)"` parses.
- `sql_target_unknown_field_rejected`: `target: {sql: {datasource:
  d, query: "select 1", extra: true}}` → serde deny-unknown error
  naming `extra`.
- `empty_datasource_or_query_rejected`: empty string fields →
  doc-validation error.
- `deadline_on_sql_target_accepts`: `deadline: 2s` on a sql target
  parses to `Some(Duration::from_secs(2))`.
- `deadline_on_last_received_still_rejected`: existing behavior —
  error message now names `partner` or `sql`.
- `elapsed_at_least_on_sql_rejected`: `elapsedAtLeast: 1s` on a sql
  target → doc-validation error.
- `order_by_predicate`: unit-test `sql_query_lacks_order_by`
  directly: `"SELECT * FROM t"` true; `"select * from t order by id"`
  false; `"SELECT * FROM t ORDER\nBY id"` false (`\s+` covers the
  newline); `"SELECT 'order by literal'"` false is NOT promised —
  the documented false-positive case is a string literal containing
  the words matching true; pin `"select 'totally ordered by intent'
  from t"` true (no `order by` token at all).
- `unordered_flag_parses`: `expectation: {unordered: true, rows:
  [[1]]}` yields `RowsExpectation.unordered == true`.
- `sql_expectation_rows_cells`: `expectation: {rows: [[1, "a"], [2,
  {contains: "b"}], [{ignore: null}, {equals: null}]], columns: [id,
  name]}` parses with 3 rows, `columns` Some(2 names), `bound` None.
- `sql_expectation_count_bound_forms`: `{count: 3}`, `{atLeast: 2}`,
  `{atMost: 5}`, `{atLeast: 1, atMost: 3}` parse to
  Exact/AtLeast/AtMost/Range.
- `sql_expectation_rows_and_bound_exclusive`: `{rows: [[1]], count:
  1}` → error naming both keys.
- `sql_expectation_empty_rows_rejected`: `{rows: []}` → error.
- `sql_expectation_row_length_names_row_index`: `columns: [id, name]`
  with a 3-cell row at index 1 → error text contains "row 1".
- `sql_expectation_unknown_field_rejected`: `{rows: [[1]], foo: 1}` →
  error listing recognized keys.
- `sql_expectation_columns_must_be_nonempty_strings`: `{columns: [],
  rows: [[1]]}` and `{columns: [1], rows: [[1]]}` → errors.
- `sql_expectation_columns_duplicate_rejected`: `{columns: [id, id],
  rows: [[1, 2]]}` → error naming `id`.
- `ordered_without_order_by_warns_once` /
  `unordered_without_order_by_does_not_warn`: through the
  `crate::log_capture` machinery (the same subscriber/window the
  `logs:` evaluation uses — `ensure_capture_subscriber()` then
  `open_window()`), parse a document whose ordered `rows` assertion
  lacks ORDER BY, close the window, and assert exactly one Warn
  event naming the action index; repeat with `unordered: true` and
  assert zero Warn events.

**Acceptance:**
- `cargo test -p camel-integration-test --lib` (no features) and
  `--features sql` both pass.
- `cargo clippy -p camel-integration-test -- -D warnings` exits 0.

- [x] 2.2

## Phase 3: executor + e2e

### Task 3.1: `runner/sql_validate.rs` executor + dispatch wiring (feature `sql`)

**Files:**
- `crates/camel-integration-test/src/runner/sql_validate.rs` (new)
- `crates/camel-integration-test/src/runner.rs` (modified: `mod
  sql_validate;` + the `validate_action` signature/dispatch changes
  below — SAME TASK, so no dead-code window in either feature
  configuration)
- `crates/camel-integration-test/src/lib.rs` (modified: `#[cfg(test)]
  mod sql_validate_test;` — the module compiles in BOTH feature
  configurations; the file's test bodies split internally with
  `#[cfg(feature = "sql")]` / `#[cfg(not(feature = "sql"))]` blocks)
- `crates/camel-integration-test/src/sql_validate_test.rs` (new)

**Steps:**
1. Create `runner/sql_validate.rs` mirroring `partner_validate.rs`
   structure. `pub(super) async fn sql_validate_action(index: usize,
   target: &SqlTarget, expected: &RowsExpectation, deadline:
   Option<Duration>, catalog: Option<&Arc<dyn DatasourceCatalog>>) ->
   Result<(), ScenarioFailure>` behind `#[cfg(feature = "sql")]`,
   plus a `#[cfg(not(feature = "sql"))]` twin returning
   `ScenarioFailure::ValidationMismatch` with detail "sql validation
   requires the `sql` feature" (the partner no-http precedent);
   `catalog: None` → fail closed with detail "sql validation: no
   datasource catalog is available" (the sql-action precedent).
2. Pool resolution copied from `execute_sql_prepare`: `get_config`
   (unknown name → error naming it), `get_pool`, downcast to
   `sqlx::AnyPool`; every driver error string passes through
   `crate::sql_action::sanitize_db_error` with the config's `db_url`.
3. Snapshot: `sqlx::query(&target.query).fetch_all(&*pool)`; map rows
   via `fn any_row_to_tuple(row: &sqlx::any::AnyRow, names:
   &[String]) -> Result<Vec<Value>, String>` — per column, in order:
   NULL → `Value::Null`; try `i64` → number; try `f64` → number; try
   `bool` → bool; try `String` → string; try `Vec<u8>` →
   `serde_json::from_slice::<Value>` first, fallback
   `Value::String(lossy UTF-8)` (the `reply_bytes_value` precedent).
   A column whose type decodes through NO arm is an EXPLICIT
   validation failure naming the column and its sqlx type info
   (fail-closed: never a silent null, never a sentinel a wildcard
   could match away — documented limitation before surrealdb parity).
   Column names from `row.columns()`.
4. Projection: with `expected.columns`, build the tuple by NAME in
   declared order; a name absent from the result → fail closed
   (`ScenarioFailure::ValidationMismatch` detail "sql validation:
   unknown projection column `X`"). Without `columns`, use query
   order.
5. Poll lattice (papal deviation — SQL state is non-monotone):
   `SQL_VALIDATE_POLL_INTERVAL = 100ms`. No deadline: one snapshot
   decides. With deadline: loop { snapshot; if bound shape AND
   `above_ceiling(bound, actual_rows.len())` → fail immediately with
   the mismatch detail; else sleep min(remaining, interval) }; at
   expiry the FINAL snapshot decides: rows shape → `rows_match(
   expected_rows, actual, unordered)`; bound shape → `bound_holds(
   bound, actual.len())`. NEVER settles early (no `settles_early`
   call).
6. Mismatch renderer `fn sql_mismatch_detail(datasource: &str,
   expected: &RowsExpectation, actual_count: usize, columns:
   &[String]) -> String`: "sql {datasource}, {render_bound(bound) |
   expected N rows (ordered|unordered)}, actual {actual_count} rows,
   columns: [names]" — no actual cell values, no query text, no
   db_url.
7. Wire the dispatch: `validate_action` (runner.rs ~line 906) does
   NOT carry the catalog today — extend its signature to
   `validate_action(action, index, started_at, router, vars,
   datasource_catalog: Option<&Arc<dyn DatasourceCatalog>>)` and
   update the single `run_action` call site to pass the parameter
   `run_action` already receives (no `run_action` signature change).
   New arm in the `match (target, expectation)`:
   `(ScenarioTarget::Sql(target), ValidateExpectation::Rows(
   expected))` calls `sql_validate_action(index, target, expected,
   deadline, datasource_catalog)`; `(ScenarioTarget::Sql(_), _)` and
   `(_, ValidateExpectation::Rows(_))` fall through to the
   `unpaired_validate` error — extend its message to name the sql/
   rows pairing rule. Driver failures map to the same failure class
   the `sql:` action arm uses for executor errors (mirror the
   existing mapping verbatim).

**Tests (sql_validate_test.rs; sql-catalog tests `#[cfg(all(test,
feature = "sql"))]`, twin test `#[cfg(all(test, not(feature =
"sql")))]`; reusing the `sqlite_catalog`/`RuntimeDatasourceCatalog`
pattern and `DB_URL = "sqlite::memory:?cache=shared"` from
sql_action_test.rs):**
- `ordered_rows_pass_immediately`: create table, insert `(1,"alice")`
  and `(2,"bob")` via `execute_sql_prepare`; validate `columns: [id,
  name]`, rows `[[1,"alice"],[2,"bob"]]`, no deadline → Ok. Command:
  `cargo test -p camel-integration-test --lib --features sql
  ordered_rows_pass`.
- `unordered_rows_match_reorder`: same seed, expected reversed,
  `unordered: true`, query without ORDER BY → Ok.
- `wildcard_and_equals_null`: seed `(7, NULL)`; rows `[[{ignore},
  {equals: null}]]` → Ok.
- `count_bounds_decide_final_snapshot`: seed 3 rows; `atLeast: 2` no
  deadline → Ok; `count: 2` → Err mismatch.
- `ceiling_breach_fails_immediately`: seed 2 rows; `atMost: 1`,
  `deadline: 300ms`; assert the failure returns in well under the
  deadline (elapsed < 250ms) with the rendered bound in the detail.
- `deadline_poll_passes_when_row_appears`: empty table;
  `tokio::spawn` inserts the expected row after ~150ms; validate
  `rows` + `deadline: 2s` → Ok (final snapshot at expiry matches).
- `no_early_settle_matching_snapshot_deleted`: insert expected row,
  spawn a task that DELETEs it after ~100ms; validate `rows` +
  `deadline: 400ms` → Err (final snapshot decides).
- `column_projection_reorders_and_unknown_fails`: `columns: [name,
  id]` with rows `[["alice", 1]]` → Ok; `columns: [id, missing]` →
  Err detail contains "unknown projection column `missing`".
- `mismatch_detail_elides_cells_and_db_url`: failing rows assertion;
  assert the detail contains the datasource name and "actual 1 rows"
  and contains NEITHER "sqlite::memory:" NOR any seeded cell literal.
- `driver_error_names_datasource_sanitized`: query against a missing
  table; assert the error text names the datasource and does not
  contain `sqlite::memory:` bytes.
- `blob_maps_json_first`: store a JSON text blob and a non-JSON blob;
  assert tuple cells equal the parsed JSON value and the lossy string
  respectively (direct `any_row_to_tuple` unit test).
- `unknown_datasource_fails_closed`: `datasource: "nope"` → Err
  naming it.
- `feature_off_twin_names_gate` (`not(feature = "sql")` config):
  calling `sql_validate_action` with any arguments returns
  `ValidationMismatch` whose detail is exactly "sql validation
  requires the `sql` feature" → `cargo test -p
  camel-integration-test --lib feature_off_twin`.

**Acceptance:**
- `cargo test -p camel-integration-test --lib --features sql` passes.
- `cargo test -p camel-integration-test --lib` (feature off,
  including the twin test) passes.
- `cargo clippy -p camel-integration-test --all-targets -- -D
  warnings` exits 0 in BOTH feature configurations.

- [x] 3.1

### Task 3.2: lib re-exports + runner-level integration tests

**Files:**
- `crates/camel-integration-test/src/lib.rs` (modified)
- `crates/camel-integration-test/src/runner_test.rs` (modified)

**Steps:**
1. Re-export in `lib.rs` alongside the existing document exports:
   `SqlTarget` (from the document module) and align the
   `RowsExpectation` re-export with how `PartnerExpectation`
   re-exports from camel-matchers today; leave `execute_sql_prepare`
   exports untouched.
2. runner-level tests exercising the dispatch through `run_action`
   (the unit-level executor behaviors are Task 3.1's).

**Tests (runner_test.rs, sql-catalog cases gated `#[cfg(all(test,
feature = "sql"))]`; the unpaired message test ungated):**
- `validate_sql_routes_through_catalog`: build
   `ScenarioAction::Validate` with `ScenarioTarget::Sql` +
   `ValidateExpectation::Rows`, seed via `execute_sql_prepare`,
   `run_action` with the catalog → Ok. Command:
   `cargo test -p camel-integration-test --lib --features sql
   validate_sql_routes`.
- `validate_sql_without_catalog_fails_closed`: same action,
  `datasource_catalog: None` → Err whose detail names the missing
  catalog.
- `unpaired_validate_sql_message`: `ScenarioTarget::Sql` +
  `ValidateExpectation::Message` → `unpaired_validate`-class error
  naming the pairing rule (construct directly, no catalog needed,
  ungated compile).

**Acceptance:**
- `cargo test -p camel-integration-test --lib` and `--lib --features
  sql` both pass.
- `cargo clippy -p camel-integration-test --all-targets -- -D
  warnings` exits 0.

- [x] 3.2

### Task 3.3: CLI end-to-end scenario document test

**Files:**
- `crates/camel-cli/tests/test_scenario_cli_e2e.rs` (modified — the
  ONLY file; both scenario documents are written to a
  `tempfile::tempdir()` inside each test, the landed sql-action e2e
  pattern already in this file at ~line 93)

**Steps:**
1. Add one passing e2e mirroring the landed sql-action e2e: a
   `.test.yaml` in a tempdir declaring a `[datasources.appdb]` with
   `sqlite::memory:?cache=shared` (the boot_scenario config-injection
   pattern from the landed e2e), a `sql:` action creating and seeding
   a table, and a `validate` sql target asserting the seeded rows
   (ordered, with ORDER BY). Run through the `camel test` CLI entry
   the existing e2e tests use; assert exit 0.
2. Add one failing e2e: same document with an expectation that does
   not match (wrong row content), `deadline: 300ms`; assert the run
   exits nonzero with a `validation-mismatch`-carrying report whose
   text contains neither the db_url nor the seeded cell literals.

**Tests:**
- `sql_validate_e2e_pass`: CLI runs the passing fixture → exit 0 →
  `cargo test -p camel-cli --test test_scenario_cli_e2e
  sql_validate_e2e_pass`.
- `sql_validate_e2e_fail_redacts`: CLI runs the failing fixture →
  nonzero exit; stdout/stderr contain "sql appdb" and no
  "sqlite::memory:" and no seeded literal.

**Acceptance:**
- `cargo test -p camel-cli --test test_scenario_cli_e2e` passes
  (integration-sql is in the CLI default features).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 3.3
