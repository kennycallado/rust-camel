# Tasks: waist-extraction

## Steering seam

### Task 1.1: steering.rs — sanitize move, dual public paths, generic resolver

**Files:**
- `crates/camel-integration-test/src/steering.rs` (new)
- `crates/camel-integration-test/src/sql_action.rs` (modified)
- `crates/camel-integration-test/src/surreal_action.rs` (modified)
- `crates/camel-integration-test/src/lib.rs` (modified)

**Steps:**
1. Create `src/steering.rs` with module docs naming it the datasource
   steering axis: name -> env-steered URL -> pool handle, with the
   identifier law (errors name the datasource, never the URL) and the
   ADR-0051 redaction law.
2. Move `sanitize_db_error` verbatim from `src/sql_action.rs` to
   `src/steering.rs`, including its doc comment and its two unit tests
   (`sanitize` module tests at current sql_action.rs ~lines 288 and
   298 — move them into a `#[cfg(test)] mod tests` inside
   `steering.rs`).
3. In `src/sql_action.rs`, replace the moved definition with
   `pub use crate::steering::sanitize_db_error;` so BOTH public paths
   survive: `camel_integration_test::sanitize_db_error` (the crate-root
   re-export in `lib.rs`) and
   `camel_integration_test::sql_action::sanitize_db_error`.
4. In `src/surreal_action.rs`, change the sanitize import to
   `use crate::steering::sanitize_db_error;` (ends the reach-through
   into `sql_action`).
5. Add the resolver to `src/steering.rs`, gated with
   `#[cfg(any(feature = "sql", feature = "surreal"))]`, its `use`
   statements gated identically:

   ```rust
   pub(crate) async fn resolve_datasource<T: 'static + Send + Sync>(
       catalog: &Arc<dyn camel_api::datasource::DatasourceCatalog>,
       name: &str,
       label: &str,
   ) -> Result<(Arc<T>, String), String>
   ```

   Body: `catalog.get_config(name)` — `None` returns
   `Err(format!("{label}: unknown datasource '{name}'"))`; else clone
   the `db_url`; `catalog.get_pool(name).await` maps errors to
   `Err(format!("{label}: datasource '{name}': {}", sanitize_db_error(&e.to_string(), &db_url)))`;
   `handle.downcast::<T>()` maps errors the same way; return
   `Ok((handle, db_url))`. No family enum, no family-specific text.
6. Confirm `lib.rs` still declares `mod steering;` (ungated) and the
   crate-root `pub use` re-export of `sanitize_db_error` now sources
   from the steering module (either directly or via the unchanged
   path through `sql_action` — one canonical chain, no duplicate
   re-export lines).

**Tests:** (in `steering.rs` `#[cfg(test)] mod tests` for the two moved
sanitize tests; in a `#[cfg(all(test, feature = "sql"))] mod
resolver_tests` for the resolver tests — the resolver is gated
`any(sql, surreal)` and its stubs (`sqlite_catalog`, `sqlx::AnyPool`)
are sql-gated, so the resolver tests must carry the sql gate to keep
featureless and surreal-only test builds compiling)
- `sanitize_replaces_exact_db_url` (moved): err text containing the
  exact db_url → sanitize → `[REDACTED]` in place of every occurrence;
  assert full-string equality.
- `sanitize_empty_db_url_is_noop` (moved): empty db_url → returned
  text unchanged; assert full-string equality.
- `resolver_unknown_datasource_names_label_and_name`: build
  `RuntimeDatasourceCatalog` (as `runner/surreal_validate_test.rs:109`
  does) with one config for name `appdb`; call
  `resolve_datasource::<sqlx::AnyPool>(catalog, "missing", "sql
  action")` → assert `Err` with EXACT string
  `sql action: unknown datasource 'missing'`. Repeat the call with
  label `sql validation`, labels `surreal action` and `surreal
  validation` (4 exact-string asserts total, one per label).
- `resolver_pool_failure_redacts_url_and_keeps_prefix`: hand-rolled
  stub catalog whose `get_config` returns a config with
  `db_url = "sqlite:///tmp/rc-6waist-secret/x.db?mode=rw"` and whose
  `get_pool` returns a fixed error
  `CamelError::ProcessorError("cannot open sqlite:///tmp/rc-6waist-secret/x.db?mode=rw".into())`;
  call `resolve_datasource::<sqlx::AnyPool>(stub, "appdb", label)` for
  EVERY label — `sql action`, `sql validation`, `surreal action`,
  `surreal validation` — and assert one EXACT string per label
  (equality, not substring; the CamelError Display prefix
  `Processor error: ` is part of each string):
  `sql action: datasource 'appdb': Processor error: cannot open [REDACTED]`,
  `sql validation: datasource 'appdb': Processor error: cannot open [REDACTED]`,
  `surreal action: datasource 'appdb': Processor error: cannot open [REDACTED]`,
  `surreal validation: datasource 'appdb': Processor error: cannot open [REDACTED]`.
- `resolver_returns_handle_and_url`: `sqlite_catalog("appdb")` stub
  (as `src/sql_action_test.rs` uses) → resolve
  `::<sqlx::AnyPool>(catalog, "appdb", "sql action")` → `Ok` with a
  usable pool and the configured `db_url` string back.
- `resolver_downcast_failure_keeps_driver_detail`: the same
  `sqlite_catalog("appdb")` stub → resolve with a mismatched type
  parameter (e.g. `::<String>`) and label `sql action` → assert
  `Err` whose message starts with the exact prefix
  `sql action: datasource 'appdb': ` and continues with the
  camel-api downcast driver detail (`failed to downcast handle`),
  containing no datasource URL.

**Acceptance:**
- `cargo test -p camel-integration-test --features sql --lib steering` exits 0.
- `sql_action_test.rs` passes unchanged (public path intact):
  `cargo test -p camel-integration-test --features sql --lib sql_action` exits 0 —
  including `executor_unknown_datasource_names_it_only`, which asserts
  the exact string `sql action: unknown datasource 'missing'`.
- Featureless build warning-clean:
  `cargo clippy -p camel-integration-test --no-default-features -- -D warnings` exits 0
  (resolver gated away, sanitize still compiled as public surface).
- `cargo fmt --check` clean; `grep -rn 'sanitize_db_error'
  crates/camel-integration-test/src/surreal_action.rs` shows the
  import from `crate::steering`, not `crate::sql_action`.

- [x] 1.1

## Poll driver

### Task 1.2: runner/poll.rs — generic deadline poll driver with paused-time tests

**Files:**
- `crates/camel-integration-test/src/runner/poll.rs` (new)
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/Cargo.toml` (modified)

**Steps:**
1. Add to `[dev-dependencies]` in Cargo.toml:
   `tokio = { workspace = true, features = ["test-util"] }` (the
   workspace `full` feature set does not enable `test-util`; paused
   tests need it).
2. Create `src/runner/poll.rs` with the driver, gated
   `#[cfg(any(test, feature = "http", feature = "sql", feature = "surreal"))]`,
   `pub(super)` visibility (sibling modules under `runner` call it as
   `super::poll::poll_until`; the design's `pub(super)` stands):

   ```rust
   pub(super) async fn poll_until<S, F, Fut>(
       deadline: Option<Duration>,
       interval: Duration,
       mut snapshot: impl FnMut() -> Fut,
       mut early: impl FnMut(&S) -> Option<Result<(), F>>,
       mut decide: impl FnMut(&S) -> Result<(), F>,
   ) -> Result<(), F>
   where
       Fut: std::future::Future<Output = Result<S, F>>,
   ```
   Note for callers (tasks 1.3/1.4): a plain borrowing closure cannot
   satisfy `FnMut() -> Fut` with one fixed `Fut` when the async block
   borrows owned locals — callers pre-bind references
   (`let pool = &pool; let db_url = &db_url;`) and pass
   `move || async move { ... }` so the future type is uniform.

3. Body, in this exact order: `None` deadline → one `snapshot().await?`
   then `decide(&s)`. `Some(deadline)` → `let until =
   tokio::time::Instant::now() + deadline;` (BEFORE the first
   snapshot), then loop: `let s = snapshot().await?;` (snapshot error
   stops at once); `if let Some(result) = early(&s) { return result; }`;
   `let now = tokio::time::Instant::now(); if now >= until { return
   decide(&s); }`;
   `tokio::time::sleep((until - now).min(interval)).await;` — the
   sleep never exceeds the remaining window.
4. Module docs: the shared poll discipline contract (delta spec
   "Deadline poll driver contract"): no-deadline single snapshot;
   expiry instant fixed before the first snapshot; snapshot error
   stops at once; early judgment precedes the expiry decision;
   in-flight snapshots are never cancelled by the deadline.
5. In `src/runner.rs`, declare `mod poll;` with the same `#[cfg(any(test,
   feature = "http", feature = "sql", feature = "surreal"))]` gate.
6. Write the unit tests inside `poll.rs` under
   `#[cfg(test)] mod tests`, ALL using
   `#[tokio::test(start_paused = true)]` (no real sleeps;
   lint-test-sleep compliant). Snapshot closures return
   `std::future::ready(Ok(state))` where state is a small
   counter/struct the tests control.

**Tests:**
- `no_deadline_takes_one_snapshot_and_decides`: snapshot closure
  counts invocations; no deadline; `decide` returns Ok → assert
  result Ok AND snapshot was called exactly once.
- `early_judgment_stops_before_deadline`: deadline 5s, interval 100ms;
  state becomes satisfying on call 2; `early` returns Some(Ok(()))
  for satisfying state → assert Ok AND snapshot called exactly twice
  (not 50 times) — auto-advance of paused time makes a busy loop fail
  this count.
- `expiry_snapshot_decides_absence_claim`: deadline 2s, interval
  100ms; `early` always None (absence claim);
  `decide` inspects the final state and returns Ok only for it →
  assert Ok AND the state passed to `decide` is the last-snapshot
  state (record states; assert decide saw snapshot N's state where N
  is the total).
- `snapshot_error_stops_at_once`: snapshot returns `Err(f)` on first
  call → assert the SAME `Err(f)` propagates AND snapshot called
  exactly once (no sleep, no second snapshot).
- `sleep_never_exceeds_remaining_window`: deadline 250ms, interval
  100ms → the sleeps taken are 100ms, 100ms, 50ms (record
  `tokio::time::Instant` deltas across a full-window run; assert the
  third sleep advances exactly 50ms, never 100ms).
- `overrunning_snapshot_still_gets_early_judgment`: deadline 100ms,
  interval 100ms; the first snapshot's future does not resolve until
  paused time is manually advanced past the deadline
  (`tokio::time::advance`), then yields a satisfying state → `early`
  returns `Some(Ok(()))` while `decide` is written to return `Err`
  for that same satisfying state → assert `Ok` (proves `early`
  decided, not `decide`; a vacuous pass is impossible because `decide`
  disagrees).

**Acceptance:**
- `cargo test -p camel-integration-test --lib runner::poll` exits 0
  (6 tests).
- Featureless build warning-clean:
  `cargo clippy -p camel-integration-test --no-default-features -- -D warnings` exits 0.
- `cargo fmt --check` clean.

- [x] 1.2

## SQL family migration

### Task 1.3: sql_action + sql_validate call the seams

**Files:**
- `crates/camel-integration-test/src/sql_action.rs` (modified)
- `crates/camel-integration-test/src/runner/sql_validate.rs` (modified)

**Steps:**
1. In `execute_sql_prepare`: replace the inline `get_config` /
   `get_pool` / `downcast` block with one call
   `crate::steering::resolve_datasource::<sqlx::AnyPool>(catalog, name,
   "sql action").await?` destructured to `(pool, db_url)`. The
   statement loop keeps its exact error text
   (`"datasource '{name}' statement [{i}]: {sanitized}"`) using the
   returned `db_url` and `sanitize_db_error`.
2. In `sql_validate_action` (feature `sql`): keep the no-catalog check
   verbatim. Replace the inline resolution block with
   `resolve_datasource::<sqlx::AnyPool>(catalog, name, "sql
   validation")` mapping the `Err(String)` DIRECTLY into
   `ScenarioFailure::ActionTransport { action: index, source:
   TransportError::Other { message } }` — the message is already
   complete; do NOT pass it through `apparatus` (that would double
   the prefix).
3. The `apparatus` helper keeps its exact body — it still formats
   snapshot/query errors (raw sanitized text) with the
   `"sql validation: datasource '{name}': {text}"` prefix.
4. Replace the `match deadline` poll block with one call to
   `super::poll::poll_until`: pre-bind `let pool = &pool; let db_url =
   &db_url;` (plus the other snapshot captures) and pass the snapshot
   as `move || async move { ... }` returning
   `Result<Snapshot, ScenarioFailure>` (apparatus errors mapped with
   `?` inside the async block — a snapshot error stops the poll at
   once); `early` = ceiling-breach only: `expected.bound.as_ref().is_some_and(|bound|
   camel_matchers::above_ceiling(bound, snapshot.tuples.len()))`
   → `Some(Err(mismatch(...)))`, else `None`; `decide` = the existing
   `decide(index, name, expected, snapshot)`. Interval constant
   `SQL_VALIDATE_POLL_INTERVAL` stays in this file.
5. Delete the now-dead inline poll loop and resolution code.

**Tests:** (all pre-existing, must pass UNCHANGED — this task adds no
new tests; it is a behavior-preserving migration)
- `sql_action_test::executor_unknown_datasource_names_it_only`:
  exact string `sql action: unknown datasource 'missing'` (site:
  action unknown-name).
- `sql_validate_test` unknown-datasource test (current ~line 542):
  message contains `unknown datasource 'nope'` under the
  `sql validation` label (site: validate unknown-name).
- Full suites: `cargo test -p camel-integration-test --features sql
  --lib sql_validate` and `cargo test -p camel-integration-test
  --features sql --lib sql_action` exit 0.
- New exact-string coverage rides task 1.1's resolver tests (all four
  labels); this task proves the wiring preserves them at the call
  sites.

**Acceptance:**
- `cargo test -p camel-integration-test --features sql --lib` exits 0.
- `cargo clippy -p camel-integration-test --features sql -- -D warnings` exits 0.
- `grep -n 'get_config\|get_pool' crates/camel-integration-test/src/sql_action.rs
  crates/camel-integration-test/src/runner/sql_validate.rs` returns
  ZERO hits outside `ensure_sqlite_memory_shared` (which reads
  `config.datasources` directly and does not resolve pools — it may
  keep its `get_config`-equivalent; assert no `catalog.get_pool` calls
  remain outside `steering.rs`).
- `cargo fmt --check` clean.

- [x] 1.3

## Surreal family migration

### Task 1.4: surreal_action + surreal_validate call the seams

**Files:**
- `crates/camel-integration-test/src/surreal_action.rs` (modified)
- `crates/camel-integration-test/src/runner/surreal_validate.rs` (modified)

**Steps:**
1. In `execute_surreal_prepare`: replace the inline resolution block
   with
   `crate::steering::resolve_datasource::<surrealdb::Surreal<surrealdb::engine::any::Any>>(catalog, name, "surreal action").await?`
   destructured to `(client, db_url)`. The statement loop (`.query` /
   `.check()` with `format!("datasource '{name}' statement [{i}]:
   {sanitized}")`) keeps its exact text using the returned `db_url`.
2. In `surreal_validate_action` (feature `surreal`): keep the
   no-catalog check verbatim; replace the inline resolution with
   `resolve_datasource::<Surreal<SurrealAny>>(catalog, name, "surreal
   validation")` mapping `Err(String)` DIRECTLY into
   `ActionTransport { Other { message } }` (never through `apparatus`
   — no doubled prefix). `apparatus` keeps its exact body for
   snapshot/query errors.
3. Replace the `match deadline` poll block with one call to
   `super::poll::poll_until`: pre-bind `let client = &client; let
   db_url = &db_url;` (plus the other snapshot captures) and pass the
   snapshot as `move || async move { ... }` returning
   `Result<Snapshot, ScenarioFailure>` (apparatus errors mapped with
   `?` inside the async block); `early` = ceiling-breach only:
   `expected.bound.as_ref().is_some_and(|bound|
   camel_matchers::above_ceiling(bound, snapshot.tuples.len()))`
   → `Some(Err(mismatch(...)))`, else `None`; `decide` = the existing
   surreal `decide(index, name, expected, snapshot)`. Interval
   constant `SURREAL_VALIDATE_POLL_INTERVAL` stays in this file and is
   passed as the interval argument.
4. Delete the now-dead inline poll loop and resolution code.

**Tests:** (pre-existing, must pass UNCHANGED)
- `runner_test::surreal_unknown_datasource_fails_closed` (feature
  `surreal`, current ~line 2015): the e2e prepare-action
  unknown-datasource case — failure names `nodb`, carries nothing
  URL-shaped.
- `runner/surreal_validate_test` full suite exits 0 — including
  `deadline_poll_passes_when_record_appears`,
  `no_early_settle_final_snapshot_decides`,
  `ceiling_breach_fails_immediately`, `driver_error_is_sanitized`
  (the four poll-discipline witnesses for the surreal family).
- `cargo test -p camel-integration-test --features surreal --lib` exits 0.

**Acceptance:**
- `cargo test -p camel-integration-test --features surreal --lib` exits 0.
- `cargo clippy -p camel-integration-test --features surreal -- -D warnings` exits 0.
- `grep -n 'catalog.get_pool\|catalog.get_config'
  crates/camel-integration-test/src/surreal_action.rs
  crates/camel-integration-test/src/runner/surreal_validate.rs`
  returns zero hits.
- `grep -rn 'sql_action' crates/camel-integration-test/src/surreal_action.rs
  crates/camel-integration-test/src/runner/surreal_validate.rs`
  returns zero hits (reach-through eliminated).
- `cargo fmt --check` clean.

- [x] 1.4

## Partner migration

### Task 1.5: partner_validate runs on the poll driver

**Files:**
- `crates/camel-integration-test/src/runner/partner_validate.rs` (modified)

**Steps:**
1. Keep the closures `snapshot` (one recorder read), `mismatch`,
   `shape_failure`, `judged_failure`, and `settles` exactly as they
   are (their bodies do not move).
2. Replace the `match deadline` block with one call to
   `super::poll::poll_until`:
   - snapshot closure: `let (requests, actual) = snapshot();`
     `std::future::ready(Ok((requests, actual)))` — the state type is
     `(Vec<HttpWireRequest>, usize)`, error type `ScenarioFailure`
     (partner snapshots are infallible, but the driver signature is
     uniform).
   - `early(&state)`: `judged_failure(&requests, actual).map(Err)`
     first; else `settles(actual, false).then_some(Ok(()))`.
   - `decide(&state)`: `judged_failure(&requests, actual).map(Err)`
     else `if settles(actual, true) { Ok(()) } else {
     Err(mismatch(actual, &requests)) }`.
3. `PARTNER_POLL_INTERVAL` stays in this file, passed as the interval
   argument.
4. Delete the now-dead inline poll loop. The no-deadline path is the
   driver's single-snapshot branch — verify the final `settles(actual,
   true)` semantics are reached through `decide` (they are identical:
   `settles_early` implies `bound_holds`, so early-settling states
   also pass the final check; behavior preserved).

**Tests:** (pre-existing, must pass UNCHANGED)
- `runner/partner_validate_test` full suite exits 0 (feature `http`) —
  including `poll_passes_once_count_settles` (current ~line 426, the
  poll-until-settle witness) and `deadline_expiry_reports_final_actual`
  (current ~line 502, the absence-claim full-window witness).
- `http_partner_test` full suite exits 0.
- `runner_test` partner cases exit 0.

**Acceptance:**
- `cargo test -p camel-integration-test --features http --lib` exits 0.
- `cargo clippy -p camel-integration-test --features http -- -D warnings` exits 0.
- `grep -c 'tokio::time::sleep'
  crates/camel-integration-test/src/runner/partner_validate.rs`
  returns 0 (the only sleep lives in the driver).
- `cargo fmt --check` clean.

- [x] 1.5

## Documentation

### Task 1.6: ADR-0069 section 14 + glossary anchors

**Files:**
- `docs/adr/0069-integration-tier-testing-contract.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)

**Steps:**
1. ADR-0069: insert `### 14. Waist: adapter taxonomy and shared
   seams` as a level-3 numbered section under `## Decision` — the
   same heading style as section 13 (level-3, at current ~line 375) —
   placed AFTER section 13 and BEFORE the `## Consequences` heading
   (current ~line 497). Do not append at file end. Content, in order:
   - Section-5 qualification: "partner-side assertions are the only
     normative proof" is traffic-family law. State families carry
     their own normative proof: the catalog-backed observation of rows
     at rest through the same boot the system under test runs on.
   - Taxonomy: **traffic adapters** (harness-owned far side of the
     wire; the partner family, feature `http`) vs **state adapters**
     (assertions over data at rest through a datasource catalog pool;
     the SQL and Surreal families, features `sql` and `surreal`).
   - Waist map (the rule-of-three seams, all inside
     `camel-integration-test`): the **steering axis**
     (`src/steering.rs` — name -> env-steered URL -> pool handle, the
     identifier law and ADR-0051 redaction in one resolver), the
     **poll driver** (`src/runner/poll.rs` — the shared deadline poll
     discipline), and the **matcher algebra home**
     (`camel-matchers`, ADR-0072 — cited as Proposed; this amendment
     decides nothing about its status and claims no vocabulary
     ownership).
   - Activation-per-need: one cargo feature per family; the section-8
     demand gate applies per family. A fourth family reuses the waist
     and adds only query vocabulary, record projection, and an action
     name.
   - Non-goal: no generic `state:` verb; no shared state-family
     trait; per-family poll intervals, error labels, and mismatch
     detail stay in the family files.
2. Update the ADR-0069 status line with the amendment note, following
   the section-13 precedent wording ("Amended 2026-10-03: section 14
   added (waist: adapter taxonomy and shared seams; bd rc-25lup.6)").
3. CONTEXT-MAP.md: add the amendment line to the ADR-0069 entry in
   the architecture-decisions index (match the existing entry style
   for section 13's amendment if present).
4. `crates/camel-integration-test/CONTEXT.md`: add a short glossary
   section (or extend the existing terms section — match file
   conventions) defining: **traffic adapter**, **state adapter**,
   **steering axis**, **poll driver**, each 1-2 sentences citing
   ADR-0069 §14.
5. English prose throughout (language policy); no emoji.
6. Human-ratification flag: this amendment edits a human-ratified
   Accepted ADR — the implementing mission's park report MUST flag
   ADR-0069 section 14 for human ratification (the task's completion
   includes carrying that flag, per the blessed design's requirement).

**Tests:**
- `cargo xtask lint-context-citations` exits 0 (the new CONTEXT.md
  terms anchor the ADR citations).
- `cargo xtask lint-test-sleep` exits 0 (no test sleeps introduced
  anywhere in the change).
- `cargo xtask lint-unbounded-wait` exits 0 (the poll driver's sleep
  is deadline-bounded).
- Manual doc check: `docs/adr/0069*` contains the string
  `### 14. Waist: adapter taxonomy and shared seams` (level-3, under
  `## Decision`, before `## Consequences`); the status line contains
  `section 14 added`.

**Acceptance:**
- `grep -c '### 14\. Waist' docs/adr/0069-integration-tier-testing-contract.md`
  returns 1.
- `cargo xtask lint-context-citations` exits 0.
- Doc build gate (conductor STAGE 4) stays green.
- Final five-matrix warning-denied sweep (conductor STAGE 4, run
  after this task): for each of featureless, `--features http`,
  `--features sql`, `--features surreal`,
  `--features http,sql,surreal` — both
  `cargo clippy -p camel-integration-test <matrix> --all-targets -- -D warnings`
  and `cargo check -p camel-integration-test <matrix> --all-targets`
  exit 0; plus `cargo xtask lint-test-sleep` and
  `cargo xtask lint-unbounded-wait` exit 0 across the workspace.

- [x] 1.6
