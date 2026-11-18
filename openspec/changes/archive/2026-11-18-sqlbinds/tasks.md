# Tasks: sqlbinds

Order note: this plan is test-first per the mission ("write these tests
first, show them failing, then implement"). Task 1.1 writes the docker
integration tests and runs them against the PRE-FIX code to collect RED
evidence; Tasks 1.2–1.5 implement; Task 1.6 turns the suite green; Task 1.7
parks with the gate table. The integration tests compile against the current
public surface (endpoint URIs + raw sqlx seeding), so they build and run
before any production change.

## camel-test

### Task 1.1: Docker RED baseline — write the bind-attack tests and record pre-fix outcomes

**Files:**
- `crates/camel-test/tests/sql_test.rs` (modified)

**Steps:**
1. Reuse the existing harness (`shared_postgres`/`shared_postgres_17`,
   `create_pool`, `setup_table`, `run_batch_route`, `read_f64_cols`,
   `read_i64_cols`, `read_text_cols`, `kinds_harness`,
   `add_sqlkinds_route`). Every test creates its own tables via
   `setup_table` with unique names (`sqlbinds_t1`…`sqlbinds_t10`), following
   the patterns of `batch_mixed_int_float_kinds_never_corrupts`. Named
   parameters in consumer URIs use the query.rs grammar `:#name` (list
   expansion `:#in:name`) — never bare `:name`.
2. Write the ten tests below EXACTLY as specified (names, setup, action,
   assert). They use only public surface (endpoint URIs, raw sqlx for
   seeding/verification), so they compile against the pre-fix code.
3. Run each test against the PRE-FIX code and record the observed outcome in
   its doc-comment: `RED (pre-fix: <observation>)` or
   `defense-in-depth (pre-fix: <observation>; corruption site is decode-side
   per the design §1 experiment)`. The bd-recorded reproductions
   (rc-7e681, rc-0eiqm) are the historical evidence; the baseline run is the
   live evidence for this tree.
4. Do not touch any production file in this task.

**Tests:**
- a. `batch_bare_select_text_described_own_oid_green`: table
  `sqlbinds_t1 (id BIGSERIAL PRIMARY KEY, n BIGINT)`; batch body of two
  items of the template `SELECT #` (the resolved `$1` is bare — described
  TEXT by PG17) with bindings
  `[3544952156018063160]` and `[123]`; route through `run_batch_route` →
  assert the batch succeeds (UPDATE_COUNT present, no error). Prevents the
  bd rc-7e681 (a) `"12345678"` ASCII read-back; the value itself is
  observable only via the pinned single-path test
  `single_select_bare_param_number_stays_numeric` (the batch shape discards
  rows).
- b. `batch_text_column_int_stores_correctly_cast_value`: table
  `sqlbinds_t2 (id BIGSERIAL PRIMARY KEY, txt TEXT)`; batch items of the
  template `INSERT INTO sqlbinds_t2 (txt) VALUES (#)` with bindings
  `[3544952156018063160]` and `[123]` → assert the batch succeeds and
  the stored `txt` values (read via an inline raw-sqlx SELECT —
  `read_text_cols` needs two TEXT columns) yield exactly `"3544952156018063160"` and `"123"` (bd
  rc-7e681 acceptance (b) verbatim; the int8 wire bytes of
  `3544952156018063160` ARE the ASCII digits `"12345678"`, so a bytes-as
  -text store would show exactly that string; it must not appear).
- c. `batch_text_described_number_cast_or_loud_never_silent`: table
  `sqlbinds_t3 (id BIGSERIAL PRIMARY KEY, n BIGINT)`; batch item of the
  template `INSERT INTO sqlbinds_t3 (n) VALUES (#::text)` with binding
  `[3544952156018063160]` (the cast pins TEXT; text→bigint has no
  assignment cast, and PG 42804 fires at PREPARE — verified live) → assert
  the batch fails with the prepare-time wrapper
  (`Batch query prepare failed: …` containing the PG cast error) and zero
  rows committed.
- d. `batch_bool_payload_casts_not_reinterprets`: tables `sqlbinds_t4 (id
  BIGSERIAL PRIMARY KEY, txt TEXT)` and `sqlbinds_t5 (id BIGSERIAL PRIMARY
  KEY, b BOOLEAN)`. `execute_batch` parses ONE template for all rows
  (producer.rs:805-828), so this is two routes: route A (template `INSERT
  INTO sqlbinds_t5 (b) VALUES (#)`, single item, binding `[true]`) →
  eligible, named path, `sqlbinds_t5.b` is `true`. Route B (template
  `INSERT INTO sqlbinds_t4 (txt) VALUES (#::text)`, single item, binding
  `[true]`) → Text×bool ⇒ ineligible ⇒ whole batch to the alias →
  the stored `txt` (inline raw-sqlx SELECT) yields `"true"` — the bool wire byte (`0x01`) never
  surfaces as `"\u0001"` (verified live).
- e. `batch_hybrid_keeps_described_float_guard_on_alias`: table
  `sqlbinds_t6 (id BIGSERIAL PRIMARY KEY, txt TEXT, a DOUBLE PRECISION, k
  INTEGER)`; batch item of the template `INSERT INTO sqlbinds_t6 (txt, a, k)
  VALUES (#::text, #, #)` with bindings `[7, 9007199254740993, 1]` (prepare is
  valid: the cast position lands in the TEXT column; the 2^53 integer sits
  at the described DOUBLE position) → assert the typed 2^53 error at
  parameter 1 (`Batch item 0, parameter 1: integer value 9007199254740993
  exceeds 2^53 and would lose precision bound as f64`) and zero rows
  committed.
- f. `batch_in_shapes_track_per_shape_targets`: table
  `sqlbinds_t7 (id BIGSERIAL PRIMARY KEY, k INTEGER, txt TEXT, a DOUBLE
  PRECISION)`; seed rows k=1,2,3,4 with `txt='seed'`, `a=2.5` (the DOUBLE
  is a WHERE predicate that matches the seeds). ONE endpoint template with
  real IN expansion, body-indexed bindings, and the DOUBLE AFTER the
  expanded list: `UPDATE sqlbinds_t7 SET txt = :#body.0 WHERE k IN
  (:#in:body.1) AND a = :#body.2` (per-item SQL text differs by list
  length: item 0 ks=[1] resolves `$1=txt, $2=k, $3=a`; item 1 ks=[2,4]
  resolves `$1=txt, $2..$3=k, $4=a` — the DOUBLE position SHIFTS with the
  list length: parameter 3 versus parameter 4). CONTROL batch first — a
  successful HYBRID batch (one ineligible item sends it to the alias,
  everything still lands): item 0 body `["x", [1], 2.5]`, item 1 body
  `["x", [2,4], 2.5]`, item 2 body `[true, [3], 2.5]` (TEXT×bool
  ineligible) → succeeds on the aliases; assert the txt writes round-trip
  (k=1,2,4 `txt="x"`; k=3 `txt="true"`) and `a` still `2.5` everywhere
  (mission acceptance item 4: described targets survive the fallback with
  exact values). Then the FAILING batch — item 0 body `["y", [1], 2.5]`
  (an observable eligible write); item 1 body `["z", [2,4],
  9007199254740993]` (2^53 at its shape's described DOUBLE — SQL `$4`, the
  zero-based binder index 3, whose index carries INTEGER-only values `2`/`4`/`2`
  in the other
  shapes' rows — a scan-only planner would select I64 there and silently
  round through the int8→float8 assignment cast; the kept DOUBLE target
  is what fires the guard); item 2 body `[true, [3], 2.5]` (TEXT×bool
  ineligible, sending the whole batch to the alias) → assert the typed
  2^53 error at item 1's kept DOUBLE position (`Batch item 1, parameter
  3`) and the control values still stand (rollback: k=1 keeps `txt="x"`,
  `a=2.5`; the `y` write never committed). This is the
  variable-length-IN regression: per-shape probes (positions shift with
  list length), per-shape described targets discriminated against the
  scan-only alternative, message-wide scan fallback, eligible-first-write
  rollback.
- g. `consumer_post_query_seeded_entry_batch_2pow53_typed`: table
  `sqlbinds_t8 (id BIGSERIAL PRIMARY KEY, f DOUBLE PRECISION)`. Both steps
  MUST share one pool: mirror the shared named-datasource fixture of
  `batch_guard_still_holds_after_a_single_mode_entry` (sql_test.rs:2363-2426)
  — a `RuntimeDatasourceCatalog` with one `pgkinds` datasource at
  `max_connections=1` registered via `SqlComponent::with_config_and_catalog`,
  and `?datasource=pgkinds` on BOTH endpoints. Separate `db_url` endpoints
  do NOT share a pool. Step 1:
  consumer route polling a source row whose column `v` is the INTEGER
  `20`, with `onConsume=INSERT INTO sqlbinds_t8 (f) VALUES (:#v)` — the
  rc-0eiqm raw persistent writer (an integer payload, whose i64 bind seeds
  a bind-derived entry). Await CONFIRMED post-query completion
  (`wait_until` on `sqlbinds_t8` containing `20.0`). Step 2: WITHOUT
  recycling, run a batch route on the SAME statement text
  (endpoint template `INSERT INTO sqlbinds_t8 (f) VALUES (#)`; resolved SQL `INSERT INTO sqlbinds_t8 (f) VALUES ($1)`) with binding
  `[9007199254740993]` → assert the typed 2^53 error and no row with a
  rounded value. If the RED (pre-fix silent rounding) cannot be reproduced
  after a genuine attempt (separate pools defeat shared-cache seeding),
  document the attempts in the doc-comment, mark `defense-in-depth`, and
  list it in the park.
- h. `consumer_post_query_number_at_float_is_typed`: source table with a
  `v` column carrying the JSON number `9007199254740993`; target table
  `sqlbinds_t9 (id BIGSERIAL PRIMARY KEY, f DOUBLE PRECISION)` seeded with
  `id=1, f=0.0`; consumer route with error handler
  `ErrorHandlerConfig::dead_letter_channel("mock:error")`,
  `breakBatchOnConsumeFail=true` and `bridgeErrorHandler=true` (without
  the bridge the post-processing failure never reaches `mock:error` —
  consumer.rs:156-170, 364-403), and
  `onConsume=UPDATE sqlbinds_t9 SET f = :#v WHERE id = 1` → assert the
  poisoned exchange arrives on `mock:error` carrying the exact typed 2^53
  error text, `sqlbinds_t9.f` still holds `0.0`, and no rounded value was
  written.
  GENUINE RED candidate: pre-fix the raw persistent int8 bind
  assignment-casts int8→float8 server-side (implicit cast, `pg_cast.dat`
  castcontext `i`) and silently stores `9007199254740992.0` while
  consumption continues; post-fix the planner's kept DOUBLE target fires
  the typed 2^53 error at bind time.
- i. `batch_bool_column_int_payload_casts_or_fails_loudly`: table
  `sqlbinds_t10 (id BIGSERIAL PRIMARY KEY, b BOOLEAN)`; batch item
  `INSERT INTO sqlbinds_t10 (b) VALUES (#)` (resolved `$1`) with binding `[7]` (numeric
  payload at a BOOL-described position — the matrix cell BOOL×Numeric) →
  assert the batch fails with the pinned PG rejection (SQLSTATE 42804 —
  verified live on postgres:17: the alias's BIGINT argument is rejected
  for the BOOLEAN column) and zero rows are committed (the committed
  state never contains a reinterpreted byte). This is the dedicated
  integration demonstration for the spec scenario "incompatible payload
  at a described BOOL position never reinterprets bytes".
  `defense-in-depth` label if PG already rejects the payload pre-fix
  (record the observed behaviour either way).
- j. `consumer_poll_and_stream_stay_green_through_planner`: passthrough —
  run a `consumer_polling`-style poll and a STREAM-LIST poll. For the
  stream half, configure the consumer endpoint for StreamList output
  (the `SqlOutputType::StreamList` config — follow
  `streamlist_undecodable_column_is_typed`'s URI configuration) and DRAIN
  the yielded NDJSON stream body to completion (the stream is lazy;
  `consumer_empty_result` neither configures nor drains — do not copy it).
  Assert rows flow and bodies are unchanged (the planner's empty-bindings
  fast path).

**Acceptance:**
- The ten tests compile and run against the PRE-FIX code; each carries a
  doc-comment recording its live baseline outcome.
- `git status` shows changes only under `crates/camel-test/tests/`.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

## camel-component-sql

### Task 1.2: Bind planner core in bind_kinds.rs

**Files:**
- `crates/components/camel-sql/src/bind_kinds.rs` (modified)

**Steps:**
1. Add `pub(crate) enum BindMode { Single, Consumer, Batch }` with an
   `alias_suffix(self) -> &'static str` helper returning `"single"`,
   `"consumer"`, or `"batch"`.
2. Add `pub(crate) struct BindPlan { pub sql: String, pub persistent: bool,
   pub targets: Vec<Option<BindTarget>> }` (same fields as today's
   `SingleBindPlan` in producer.rs).
3. Add `pub(crate) enum ProbeOutcome { Described(Vec<AnyTypeInfoKind>),
   Unusable }` and a private `probe_kinds(conn, sql, mode) ->
   Result<ProbeOutcome, CamelError>` reproducing today's probe: prepare with
   empty parameter types on `<sql> /* {mode}-bind-kinds-probe */`, map
   `Some(Either::Left(types)) if !types.is_empty()` to `Described`, other
   metadata shapes to `Unusable`, `Err(sqlx::Error::AnyDriverError(_))` to
   `Unusable`. On other errors: keep the `warn!` log prefixes unchanged
   (`Single describe probe failed: {e}`, `Batch describe probe failed: {e}`,
   Consumer reuses the Single log prefix) AND the propagated CamelError
   wrappers unchanged (`SQL query prepare failed: {e}` for Single/Consumer,
   `Batch query prepare failed: {e}` for Batch — producer.rs:480 and
   producer.rs:880 are the two wrappers; do not swap them). Prepare-time PG
   errors (e.g. the 42804 of a `::text`-into-BIGINT statement) propagate
   through exactly this wrapper.
4. Add `pub(crate) fn hybrid_fallback_targets_rows(described:
   &[Option<BindTarget>], message_scan: &[Option<BindTarget>]) ->
   Vec<Option<BindTarget>>` = `described[i].or(message_scan[i])`. The scan
   argument is always the MESSAGE-WIDE `scan_bind_targets(all rows)` — the
   no-loosening rule (design.md §2): the scan never narrows per shape.
   (The old `hybrid_fallback_targets(described, bindings)` wrapper stays in
   place in this task — its production caller `plan_single_binds` is still
   alive; Task 1.3 deletes it and migrates its unit tests.)
5. Add the pure agreement predicate:
   `pub(crate) fn named_kinds_agree(probe: &[AnyTypeInfoKind],
   readback: Option<&[AnyTypeInfoKind]>) -> bool` — `true` only when
   `readback` is `Some` and equal to `probe` position-wise by RAW
   `AnyTypeInfoKind` (not mapped targets: mapped comparison hides
   TEXT-vs-BOOL disagreements).
6. Add the pure path decider: `pub(crate) enum BindPath { Named, Alias }`
   and `pub(crate) fn decide_path(probe: &ProbeOutcome, rows_eligible: bool,
   readback_agrees: bool) -> BindPath` — `Named` only when the probe is
   `Described` AND `rows_eligible` AND `readback_agrees`; otherwise `Alias`.
   On `Alias`, targets = `hybrid_fallback_targets_rows(probe_targets,
   message_scan)` when the probe is `Described`, or the message-wide scan
   when `Unusable`.
7. Add `pub(crate) async fn plan_binds(conn: &mut AnyConnection, sql: &str,
   rows: &[&[serde_json::Value]], mode: BindMode) -> Result<BindPlan,
   CamelError>`: rows with all-empty bindings plan to the default persistent
   path (`BindPlan { sql: sql.into(), persistent: true, targets: vec![] }`,
   no probe); otherwise probe, compute eligibility
   (`described_mode_eligible` against every row), named prepare with
   read-back, `decide_path`, and return: Named → `BindPlan { sql: real,
   persistent: true, targets: readback_targets }`; Alias →
   `BindPlan { sql: alias /* {mode}-bind-kinds-fallback */, persistent:
   false, targets: hybrid-or-scan }`. This subsumes `plan_single_binds`;
   single-path behaviour must not change in this task (Task 1.3 migrates
   the call sites).
8. Add the pure batch shape planner: `pub(crate) struct ShapeGroup { pub
   sql: String, pub rows: Vec<usize> }`, `pub(crate) fn
   group_rows_by_sql(sqls: &[String]) -> Vec<ShapeGroup>`
   (first-appearance order), `pub(crate) async fn probe_batch_shapes(conn:
   &mut AnyConnection, groups: &[ShapeGroup]) -> Result<Vec<ProbeOutcome>,
   CamelError>` (one probe per distinct shape on its own
   `/* batch-bind-kinds-probe */` text), `pub(crate) enum ItemBindSource
   { NamedReadBack, AliasPlan }`, `pub(crate) struct BatchItemPlan { pub
   source: ItemBindSource, pub alias_targets: Vec<Option<BindTarget>> }`,
   and `pub(crate) fn plan_batch_items(probes: &[(String, ProbeOutcome)],
   items: &[(String, Vec<serde_json::Value>)], message_scan:
   &[Option<BindTarget>]) -> Vec<BatchItemPlan>` — the planner PRECOMPUTES
   each item's alias targets so the unit tests exercise production output:
   the FIRST probe in the list selects the batch mode (today's semantics,
   producer.rs probes `prepared_items.first()`): first probe `Unusable` →
   every item is `BatchItemPlan { source: AliasPlan, alias_targets:
   message_scan.to_vec() }` (legacy behaviour, no loosening — scan only,
   no described targets). Otherwise per shape: usable probe + every row of
   the shape passing `described_mode_eligible` → `BatchItemPlan { source:
   NamedReadBack, alias_targets: hybrid_fallback_targets_rows(shape probe
   targets, message_scan) }` (precomputed for the whole-batch collapse); an
   ineligible row on a usable probe → `BatchItemPlan { source:
   AliasPlan, alias_targets: hybrid_fallback_targets_rows(shape probe
   targets, message_scan) }` (mirrors the single-path precedent,
   producer.rs:505-514: the row's scan keeps every described numeric
   target); a later-unusable shape → `BatchItemPlan { source: AliasPlan,
   alias_targets: message_scan.to_vec() }` (scan alone — a
   later unusable shape never erases earlier described guards, and a kept
   BIGINT target from a later shape's usable probe never replaces the
   message-wide scan's F64 guard at a shared position). The whole-batch
   rule (one `AliasPlan` item ⇒ every item executes on its precomputed
   alias targets) is enforced by the batch loop in Task 1.4.
9. Because the new `pub(crate)` API has no production caller until Tasks
   1.3–1.5, annotate the new items with `#[allow(dead_code)]` and REMOVE
   those annotations as call sites land: Task 1.3 removes them from
   `BindPlan`, `plan_binds`, `named_kinds_agree`,
   `hybrid_fallback_targets_rows` (`BindMode` keeps its allow until Task
   1.5 — the `Consumer` variant is first constructed there, and rustc's
   dead-code lint flags unconstructed variants); Task 1.4 from
   `probe_batch_shapes`, `plan_batch_items`, `group_rows_by_sql`,
   `ShapeGroup`, `BatchItemPlan`, `ItemBindSource` (`decide_path` is already production-
   called by `plan_binds` in this task — remove its allow here too if
   present); Task 1.5 from `BindMode`.
10. Run `cargo test -p camel-component-sql --lib`, `cargo fmt --check
    --all`, and `cargo clippy -p camel-component-sql --all-targets -- -D
    warnings`; fix findings.

**Tests:**
- `every_matrix_cell_is_decided`: extend `eligibility_matrix_is_closed` to
  iterate all 9 `AnyTypeInfoKind` values × 4 payload families
  (Bool/Numeric/Text/Null) plus the Text/Bool/Blob × JSON-number-and-bool
  cases from today's table → assert each cell against the matrix table in
  `specs/sql-binds/spec.md`. Command: `cargo test -p camel-component-sql
  --lib eligibility_matrix_is_closed`. Expected: pass.
- `named_kinds_agree_requires_raw_equality`: probe `[Double, Text]` vs
  read-back `Some([Double, Bool])` → `false` (mapped comparison would hide
  the TEXT-vs-BOOL disagreement); probe `[Double, Integer]` vs
  `Some([BigInt, Integer])` → `false`; probe `[Double, Integer]` vs
  `Some([Double, Integer])` → `true`; read-back `None` → `false`. Command:
  `cargo test -p camel-component-sql --lib named_kinds_agree`. Expected:
  pass.
- `decide_path_aliases_on_disagreement`: probe `Described([Double])`,
  rows eligible, read-back disagrees → `BindPath::Alias`; all three
  conditions hold → `BindPath::Named`; probe `Unusable` → `Alias`. Command:
  `cargo test -p camel-component-sql --lib decide_path`. Expected: pass.
- `hybrid_rows_keep_described_numeric_targets`: described
  `[Some(F64), None]` with message scan `[None, Some(F64)]` →
  `hybrid_fallback_targets_rows` yields `[Some(F64), Some(F64)]`. Command:
  `cargo test -p camel-component-sql --lib hybrid_rows_keep_described`.
  Expected: pass.
- `message_wide_scan_regression_two_shapes_share_float_guard`: shapes A and
  B both probe `Unusable`; shape A's row is `[json!(1.5)]`, shape B's row is
  `[json!(9007199254740993i64)]` at the same position index → the
  message-wide scan over both rows yields `[Some(F64)]` for BOTH shapes'
  alias targets, and `normalize_for_target(&json!(9007199254740993i64),
  Some(F64), …)` is the typed 2^53 error (a per-shape scan would have given
  shape B `[Some(I64)]` and rounded silently — the no-loosening
  regression). Command: `cargo test -p camel-component-sql --lib
  message_wide_scan_regression`. Expected: pass.
- `group_rows_by_sql_preserves_first_appearance`: sqls `["A", "B", "A"]` →
  groups `[("A", [0, 2]), ("B", [1])]`. Command: `cargo test -p
  camel-component-sql --lib group_rows_by_sql`. Expected: pass.
- `plan_batch_items_mode_only`: probes
  `[("A", Described(vec![Double, Integer])), ("B", Described(vec![Text]))]`,
  items `[("A", vec![json!(2.5), json!(1)]), ("B", vec![json!(123)])]` →
  item 0 `source: NamedReadBack` (its shape is eligible), item 1
  `source: AliasPlan` (Text× number). Command: `cargo test -p
  camel-component-sql --lib plan_batch_items_mode_only`. Expected: pass.
- `plan_batch_items_unusable_probe_is_alias_plan`: probes
  `[("N", Unusable)]`, items `[("N", vec![json!(1), json!("x")])]` →
  `AliasPlan`. Command: `cargo test -p camel-component-sql --lib
  plan_batch_items_unusable_probe`. Expected: pass.
- `first_unusable_probe_scans_whole_batch`: probes `[("A", Unusable),
  ("B", Described(vec![Text, BigInt, Double]))]`, items `[("A",
  vec![json!("x"), json!(1.5), json!(1)]), ("B", vec![json!("y"),
  json!(9007199254740993i64), json!(2.5)])]`, message scan
  `[None, Some(F64), ...]` → `plan_batch_items` returns EVERY item as
  `BatchItemPlan { source: AliasPlan, alias_targets: [None, Some(F64),
  ...] }` (the production output — scan only, no described targets), and
  `normalize_for_target(&json!(9007199254740993i64), Some(F64), …)` is
  the typed 2^53 error: the legacy rejection survives (a kept BIGINT
  target would have silently accepted it). Command: `cargo test -p
  camel-component-sql --lib first_unusable_probe`. Expected: pass.
- `later_unusable_shape_never_erases_described_guards`: probes in the
  reverse order `[("A", Described(vec![Text, Double, Integer])),
  ("B", Unusable)]`, items `[("A", vec![json!("x"),
  json!(9007199254740993i64), json!(1)]), ("B", vec![json!(true),
  json!(9007199254740993i64), json!(1)])]`, message scan
  `[None, Some(I64), Some(I64)]` (INTEGER-only at the guarded index —
  scan-only selection would pick I64 and silently accept) →
  `plan_batch_items` returns item 0 as `BatchItemPlan { source:
  NamedReadBack, alias_targets: [None, Some(F64), Some(I32)] }` (its
  shape's described DOUBLE KEPT at the guarded index — the production
  output that the whole-batch rule then executes on the alias) and item 1
  as `BatchItemPlan { source: AliasPlan, alias_targets: [None, Some(I64),
  Some(I64)] }` (scan alone); `normalize_for_target(
  &json!(9007199254740993i64), Some(F64), …)` is the typed 2^53 error
  while `normalize_for_target(&json!(9007199254740993i64), Some(I64), …)`
  would silently pass — proving the described guard survives the
  later-unusable shape. Command: `cargo test -p camel-component-sql --lib
  later_unusable_shape`. Expected: pass.

**Acceptance:**
- `cargo test -p camel-component-sql --lib` exits 0.
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` exits
  0 with the scoped `#[allow(dead_code)]` annotations present (step 9).
- The existing unit tests `bind_ctx_wording::batch_wording_is_byte_exact`
  and `single_wording_names_only_the_position` still pass byte-identically.
- No behaviour change outside bind_kinds.rs (producer.rs/consumer.rs
  untouched in this task).

- [x] 1.2

### Task 1.3: Producer single path routes through plan_binds

**Files:**
- `crates/components/camel-sql/src/producer.rs` (modified)
- `crates/components/camel-sql/src/bind_kinds.rs` (modified)

**Steps:**
1. Delete `plan_single_binds` and the `SingleBindPlan` struct; replace their
   uses in `fetch_single_rows`, the `StreamList` block, and `execute_modify`
   with `plan_binds(&mut conn, &prepared.sql,
   &[prepared.bindings.as_slice()], BindMode::Single)` returning `BindPlan`.
   Keep the release-after-error error propagation and the
   plan-execute-on-the-same-connection structure byte-identical.
2. Delete the old `hybrid_fallback_targets(described, bindings)` wrapper in
   bind_kinds.rs (its sole production caller died with `plan_single_binds`)
   and migrate its unit tests to `hybrid_fallback_targets_rows`.
3. Remove the `#[allow(dead_code)]` annotations from `BindPlan`,
   `plan_binds`, `named_kinds_agree`, and `hybrid_fallback_targets_rows`
   (all now production-called). KEEP the annotation on `BindMode`: the
   `Consumer` variant still has no constructor until Task 1.5, and rustc's
   dead-code lint flags unconstructed variants.
4. The alias suffix strings the planner generates stay identical to today's
   (`/* single-bind-kinds-probe */`, `/* single-bind-kinds-fallback */`).
5. Run the crate lib tests and fmt/clippy; fix findings.

**Tests:**
- All existing single-mode lib tests stay green: command
  `cargo test -p camel-component-sql --lib`. Expected: pass — the ONLY
  permitted test edit in this task is migrating the `hybrid_targets` unit
  tests to `hybrid_fallback_targets_rows` (step 2); every pinned
  integration assertion stays unchanged.
- Existing integration shapes stay pinned (run in Task 1.6's suite):
  `single_insert_mixed_kinds_never_corrupts`,
  `single_select_mixed_kinds_never_corrupts`,
  `single_bind_precision_error_is_typed`,
  `batch_guard_still_holds_after_a_single_mode_entry`,
  `single_select_bare_param_number_stays_numeric`,
  `single_select_bool_then_string_stays_exact`,
  `single_select_hybrid_keeps_described_float_guard` — no assertion edits
  allowed.

**Acceptance:**
- `grep -n "plan_single_binds\|SingleBindPlan\|hybrid_fallback_targets\b"
  crates/components/camel-sql/src/producer.rs
  crates/components/camel-sql/src/bind_kinds.rs` shows no hits for the
  deleted symbols (the rows variant remains).
- `cargo test -p camel-component-sql --lib` exits 0.
- `cargo fmt --check --all` and `cargo clippy -p camel-component-sql
  --all-targets -- -D warnings` exit 0.

- [x] 1.3

### Task 1.4: Batch path routes through the planner

**Files:**
- `crates/components/camel-sql/src/producer.rs` (modified)
- `crates/components/camel-sql/src/bind_kinds.rs` (modified)

**Steps:**
1. In `execute_batch`, after `prepared_items` are resolved, build
   `sqls: Vec<String>` from the items, call `group_rows_by_sql`, and obtain
   per-shape `ProbeOutcome`s via `probe_batch_shapes` on the transaction
   connection. The FIRST shape's probe selects the batch mode (today's
   semantics): first probe `Unusable` → whole batch on the legacy
   message-wide scan; first probe usable → described mode where later
   unusable shapes only make their own items ineligible (whole batch to
   the alias, usable shapes keeping their described targets there). Keep
   producer.rs:879's `warn!` prefix (`Batch describe probe
   failed: {e}`) and producer.rs:880's error wrapper (`Batch query prepare
   failed: {e}`) unchanged, with `AnyDriverError` → `Unusable`. A
   prepare-time PG error on ANY shape (e.g. 42804 from a `::text` cast)
   propagates through this same wrapper before any item binds — Task 1.1
   test (c) asserts exactly this.
2. Call `plan_batch_items(probes, items,
   &scan_bind_targets(all rows))` (message-wide — the no-loosening rule)
   and enforce the whole-batch rule: decide EVERY item's plan (probes and,
   for `NamedReadBack` items, the named prepare read-backs) BEFORE
   executing item 0 — a mismatch discovered on a later shape must still
   send the whole batch to the aliases. If any item's `source` is
   `AliasPlan`, every item executes on its PRECOMPUTED `alias_targets` on
   the alias text (`<item sql> /* batch-bind-kinds-fallback */`,
   `persistent(false)`); `NamedReadBack` items keep today's named prepare
   + read-back path (their precomputed `alias_targets` are only used when
   the whole-batch collapse or a disagreeing read-back moves them to the
   alias). Binds go through the unchanged `bind_batch_item`. The
   message-wide `scanned_targets` variable and the `described_mode`
   boolean disappear.
3. When every item is `NamedReadBack`, keep today's named path per item:
   `Executor::prepare_with` on the item's real text, and the item runs
   named only when `named_kinds_agree(shape_probe_kinds, readback)` holds;
   a disagreeing or unusable read-back moves that item — and by the
   whole-batch rule already the whole batch — to the alias with the
   probe-kept hybrid targets (the rc-7e681 defect-2 fix and the
   poisoned-entry defence; design.md §1/§2).
4. Keep the transaction wrapping, `expected_update_count` handling, the
   `warn!` prefix `Batch describe failed at index {batch_idx}: {e}` with its
   error wrapper `Batch query prepare failed: {e}`, `Batch query execution
   failed: {e}`, and the commit path byte-identical.
5. Remove the remaining `#[allow(dead_code)]` from `probe_batch_shapes`,
   `plan_batch_items`, `group_rows_by_sql`, `ShapeGroup`, `BatchItemPlan`,
   and `ItemBindSource` as they become production-called (`ProbeOutcome` and
   `decide_path` have been live since Task 1.2 — any allow on them was
   already removed there).
6. Run the crate lib tests and fmt/clippy; fix findings.

**Tests:**
- `batch_plan_keeps_described_double_on_alias`: unit test — probe kinds
  `[Double, Text]`, item `vec![json!(9007199254740993i64), json!(123)]`
  (position 1: Text×number ⇒ shape ineligible) → `plan_batch_items`
  yields `BatchItemPlan { source: AliasPlan, alias_targets: [Some(F64),
  Some(I64)] }` (described numeric target survives the fallback, position
  1 scanned from the message-wide scan), and `bind_batch_item` on those
  targets produces the pinned error string
  `Batch item 0, parameter 0: integer value 9007199254740993 exceeds 2^53
  and would lose precision bound as f64`. Command: `cargo test -p
  camel-component-sql --lib batch_plan_keeps_described_double`. Expected:
  pass.
- All existing batch lib tests stay green unchanged: command
  `cargo test -p camel-component-sql --lib`. Expected: pass with zero test
  edits in this task.

**Acceptance:**
- `grep -n "scanned_targets\|described_mode"
  crates/components/camel-sql/src/producer.rs` returns no hits inside
  `execute_batch` (the scan now lives behind the planner).
- No `#[allow(dead_code)]` remains on the batch-planner symbols.
- `cargo test -p camel-component-sql --lib` exits 0.
- `cargo fmt --check --all` and `cargo clippy -p camel-component-sql
  --all-targets -- -D warnings` exit 0.

- [x] 1.4

### Task 1.5: Consumer routes through the planner; raw binder becomes test-only

**Files:**
- `crates/components/camel-sql/src/consumer.rs` (modified)
- `crates/components/camel-sql/src/bind_kinds.rs` (modified)

**Steps:**
1. `poll_database` (site at consumer.rs:112): replace
   `bind_json_values(sqlx::query(...))` + `fetch_all(pool)` with: acquire
   one pooled connection, `plan_binds(&mut conn, &prepared.sql,
   &[prepared.bindings.as_slice()], BindMode::Consumer)`, build the query
   with `sqlx::query(AssertSqlSafe(plan.sql))` +
   `.persistent(plan.persistent)`, bind via `bind_single_item`, fetch on
   that same connection. Keep the `fetch_error("SQL consumer poll query
   failed", e)` mapping and the empty-bindings fast path.
2. `poll_database_stream` (site at :245): inside the `try_stream!` block,
   acquire one pooled connection, plan with `BindMode::Consumer`, bind via
   `bind_single_item`, and `fetch(&mut *conn)` on that connection for the
   stream's lifetime (mirror the producer `StreamList` structure). The
   producer's connection-release helper (`release_after_error`,
   producer.rs:408) is private to producer.rs: make it `pub(crate)` and
   reuse it — do not duplicate rollback logic.
3. `execute_post_query` (site at :336): acquire one pooled connection, plan
   with `BindMode::Consumer`, bind via `bind_single_item`, execute on that
   connection. Keep the `Post-query execution failed: {e}` mapping and the
   zero-rows warning.
4. In `bind_kinds.rs`, mark `bind_json_values` `#[cfg(test)]` (keep
   `bind_single_json_value` production — the planner's `Default` arm uses
   it). The compiler must now reject any new production caller. Remove the
   last `#[allow(dead_code)]` from `BindMode` (the `Consumer` variant now
   has a constructor).
5. Run the crate lib tests and fmt/clippy; fix findings.

**Tests:**
- Existing consumer lib tests stay green unchanged: command
  `cargo test -p camel-component-sql --lib`. Expected: pass with zero test
  edits.
- Compile-time privacy check: `grep -rn "bind_json_values"
  crates/components/camel-sql/src --include="*.rs" | grep -v
  "bind_kinds.rs"` shows only `#[cfg(test)]` test-module uses.

**Acceptance:**
- `cargo test -p camel-component-sql --lib` exits 0.
- `cargo check -p camel-component-sql` exits 0 (no non-test
  `bind_json_values` references remain).
- No `#[allow(dead_code)]` remains anywhere in bind_kinds.rs.
- `cargo fmt --check --all` and `cargo clippy -p camel-component-sql
  --all-targets -- -D warnings` exit 0.

- [x] 1.5

## camel-test

### Task 1.6: Turn the attack suite green; pin the untouched suites

**Files:**
- `crates/camel-test/tests/sql_test.rs` (modified, doc-comments only)

**Steps:**
1. Run the ten Task 1.1 tests; fix failures loop-wise (production code
   belongs to Tasks 1.2–1.5 — if a failure reveals a production defect, fix
   it in the component and note it in the park).
2. Finalize each test's doc-comment: `RED (pre-fix: …)` or
   `defense-in-depth (…)` per the recorded baseline (Task 1.1) versus the
   post-fix behaviour.
3. Run the pinned suites and confirm `git diff` shows no edits to
   `batch_homogeneous_kinds_unchanged`,
   `batch_mixed_int_float_kinds_never_corrupts`,
   `batch_numeric_column_fallback_works_twice`,
   `batch_guard_still_holds_after_a_single_mode_entry`,
   `single_insert_mixed_kinds_never_corrupts`,
   `single_select_mixed_kinds_never_corrupts`,
   `single_bind_precision_error_is_typed`,
   `single_select_bare_param_number_stays_numeric`,
   `single_select_bool_then_string_stays_exact`,
   `single_select_hybrid_keeps_described_float_guard`.
4. Run the whole docker suite (foreground): `cargo test -p camel-test
   --features integration-tests --test sql_test`.

**Tests:** (commands)
- `cargo test -p camel-test --features integration-tests --test sql_test
  batch_bare_select_text_described_own_oid_green` → pass.
- `cargo test -p camel-test --features integration-tests --test sql_test
  batch_text_column_int_stores_correctly_cast_value` → pass.
- `cargo test -p camel-test --features integration-tests --test sql_test
  batch_text_described_number_cast_or_loud_never_silent` → pass.
- `cargo test -p camel-test --features integration-tests --test sql_test
  batch_bool_payload_casts_not_reinterprets` → pass.
- `cargo test -p camel-test --features integration-tests --test sql_test
  batch_hybrid_keeps_described_float_guard_on_alias` → pass.
- `cargo test -p camel-test --features integration-tests --test sql_test
  batch_in_shapes_track_per_shape_targets` → pass.
- `cargo test -p camel-test --features integration-tests --test sql_test
  consumer_post_query_seeded_entry_batch_2pow53_typed` → pass.
- `cargo test -p camel-test --features integration-tests --test sql_test
  consumer_post_query_number_at_float_is_typed` → pass.
- `cargo test -p camel-test --features integration-tests --test sql_test
  batch_bool_column_int_payload_casts_or_fails_loudly` → pass.
- `cargo test -p camel-test --features integration-tests --test sql_test
  consumer_poll_and_stream_stay_green_through_planner` → pass.
- `cargo test -p camel-test --features integration-tests --test sql_test` →
  the full suite (including the pinned batch/single tests) passes.

**Acceptance:**
- All ten new tests pass; the full `sql_test` suite passes in the
  foreground with Docker available.
- Zero modifications to the pinned test functions listed in step 3.
- `cargo fmt --check --all` and `cargo clippy -p camel-component-sql
  --all-targets -- -D warnings` exit 0.

- [x] 1.6

## Closeout

### Task 1.7: Gate sweep, park notes, park file

**Files:**
- `openspec/changes/sqlbinds/park-notes.md` (new)

**Steps:**
1. Run the mission gate table (foreground, from the worktree) and record
   every exit code: `cargo fmt --check --all`;
   `cargo clippy -p camel-component-sql --all-targets -- -D warnings`;
   `cargo test -p camel-component-sql`;
   `cargo test -p camel-test --features integration-tests --test sql_test`
   (docker, REQUIRED before parking);
   `cargo xtask lint-unwrap`; `cargo xtask lint-secrets`;
   `cargo xtask lint-unbounded-wait`; `cargo xtask lint-ignore`;
   `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-sql --no-deps`;
   `bash scripts/gates-local.sh`. Do NOT run `cargo test --workspace`.
   If docs/** changed (it should not), run `cd docs && mdbook test`.
2. Write `park-notes.md` containing:
   - the release-note caveat verbatim: "The SQL bind planner fix prevents
     new statement-cache poisoning. sqlx statement caches are per connection
     and do not survive process exit: restart on upgrade to clear entries
     seeded by a pre-upgrade binary. PgBouncer session mode must keep
     `server_reset_query=DISCARD ALL` (the default).";
   - the experiment table (design.md §1): describe kinds for bare/`::text`
     positions on named and alias prepares, and the measured server
     outcomes (int8→TEXT-column stores `"3544952156018063160"`; bool→
     `"true"`; bare SELECT reads back exact; `::text`→BIGINT fails 42804 at
     prepare);
   - the mapping of the ten tests to the mission's six acceptance items
     and to the `sql-binds` spec scenarios, with each test's RED vs
     `defense-in-depth` label and the live baseline observation;
   - the consumer choice (machinery via `plan_binds`, not alias-only, with
     the justification from design.md §3) and the batch alias/namespace
     (`<item sql> /* batch-bind-kinds-fallback */`, `persistent(false)`);
   - the eligibility matrix as implemented (pointer to the spec table plus
     the shape-layer rule: per-shape described targets, message-wide scan);
   - the residual documented by the ruling: a NUMERIC-probe batch keeps the
     legacy message-wide scan, which can still round an integer above 2^53
     at a float column server-side (postgres-native describe, rc-cdger,
     closes it);
   - the deferrals array (empty `[]` unless something was deferred — the
     mission forbids deferring anything from the acceptance list);
   - the diffstat and the full gate table with exit codes;
   - the squash-commit body draft: subject `fix(sql): one bind planner for
     producer, batch and consumer`, body naming rc-7e681 (batch
     described-OID corruption + scan-drop) and rc-0eiqm (consumer raw
     persistent binds), plus the release-note caveat paragraph, ending with
     `Bd: rc-7e681, rc-0eiqm`.
3. Add the caveat as a comment on bd `rc-0eiqm` (from the repo root:
   `bd comment rc-0eiqm "<caveat text>"`) so the release-notes cut picks it
   up from the closed issue. Record in the park that the spec's
   upgrade-caveat scenario is satisfied only when the GENERATED release
   notes for the next version carry the caveat verbatim — the commit body,
   bd comment, and park are the three sources the release cut consumes, and
   the master conductor verifies the generated notes before tagging.
4. Create the park file `.opencode/fleet/inbox/sqlbinds-parked.json`
   (mission landing contract) from the repo root with: the gate table with
   exit codes, the diffstat, the matrix-as-implemented pointer, the spec
   scenario list, the ten test names with results (including the docker
   suite GREEN — no failure waiver), the consumer choice and justification,
   the batch alias/namespace, and the full deferrals array.

**Tests:**
- No code tests. Verification: `grep -c "DISCARD ALL"
  openspec/changes/sqlbinds/park-notes.md` returns `1` or more; the bd
  comment appears in `bd comments rc-0eiqm`; the gate table in
  `park-notes.md` lists every gate from step 1 with its exit code.

**Acceptance:**
- `park-notes.md` exists with the caveat, experiment table, test mapping,
  deferrals array, diffstat, gate table, and commit-body draft.
- bd `rc-0eiqm` carries the caveat comment.
- No `docs/**` changes exist in the diff (`git diff main...HEAD -- docs/`
  is empty), so `mdbook test` is N/A.
- Every gate in step 1 exited 0 (no waivers: the docker suite must be
  GREEN before parking).
- `.opencode/fleet/inbox/sqlbinds-parked.json` exists with the mission
  landing contract fields.

- [x] 1.7
