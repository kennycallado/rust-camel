# Tasks: sql-pool-error-rollback

All work is in the worktree. Rust gates run under
`systemd-run --user --scope --collect --unit=fleet-sqltxpool` with
`TMPDIR=/home/shared/tmp`, `CARGO_BUILD_JOBS=6`, `MemoryMax=12G`, from the
worktree root — never in the main checkout, never backgrounded.
Log-policy: every new `warn!`/`debug!` in producer code gets the
`// log-policy: handler-owned` + `// (category a: ...)` comment pair, mirroring
existing producer sites. No `unwrap()` outside tests (`expect("reason")`).

## camel-sql (producer cleanup machinery + single-mode wiring)

### Task 1.1: rollback machinery, release helper, wire execute_modify + fetch_single_rows (red-first)

**Files:**
- `crates/components/camel-sql/src/producer.rs` (modified)

Machinery and its first production caller land in the SAME task: private
items used only from the test module still warn dead-code in the library
target, so a machinery-only task cannot pass `clippy -D warnings`.

**Steps:**
1. RED FIRST (before any machinery or wiring): add these three `#[tokio::test]` fns to `mod tests`, run the commands below, and record both outputs verbatim in the task report:
   - `failing_manual_begin_does_not_poison_the_pool`: setup `sqlite_pool().await` (max_connections=1) + `seed_items_table(&pool)` → build `PreparedQuery { sql: "BEGIN IMMEDIATE; INSERT INTO items (id,name) VALUES (9,'x'); INSERT INTO items (id,name) VALUES (9,'y'); COMMIT;".to_string(), bindings: vec![] }` and an empty `Exchange`; call `execute_modify(&pool, &prepared, &config, &mut exchange)` where `config` is `SqlEndpointConfig::from_uri("sql:x?db_url=sqlite::memory:").unwrap()` + `resolve_defaults()` → assert the call returns `Err` (assert `err.to_string()` is non-empty). Then a second call: `tokio::time::timeout(Duration::from_secs(2), execute_select(&pool, &select_prepared, &select_config, &mut ex2))` where `select_prepared.sql = "select id from items order by id"` and `select_config` uses `&outputType=SelectList` → assert `Ok(Ok(()))`, and `ex2.input.header(headers::ROW_COUNT)` is `json!(2)` (row 9 absent, rolled back — assert on `ex2`, the SELECT's own exchange, not on the failed modify's `exchange`).
   - `failed_statement_without_transaction_keeps_error_and_pool_usable`: setup same helpers → `execute_modify` with `PreparedQuery { sql: "INSERT INTO items (id,name) VALUES (1,'dup')".to_string(), bindings: vec![] }` (id 1 exists → UNIQUE violation, no BEGIN) → assert `Err` whose message contains `"Query execution failed"`; IMMEDIATELY after the failing call assert `pool.size() == 1` (connection kept, benign rollback) BEFORE any follow-up query; then `execute_select` with `"select id from items order by id"` → assert `Ok(())` and ROW_COUNT `json!(2)`.
   - `failing_select_keeps_error_and_pool_usable`: setup same helpers → `execute_select` with `PreparedQuery { sql: "SELECT * FROM items_missing_table".to_string(), bindings: vec![] }` → assert `Err` with message containing `"Query execution failed"` (that is the string `fetch_error` actually emits; the context argument only feeds the warn log); assert `pool.size() == 1` immediately after; follow-up `execute_select("select id from items order by id")` → `Ok(())`.
   - Run: `systemd-run ... cargo test -p camel-component-sql --lib producer::tests::failing_manual_begin_does_not_poison_the_pool` (and the other two). Expected: RED — the poison test fails on unwired code (ROW_COUNT is 3 via the leaked open transaction, or the follow-up times out); the preservation/retention tests may already pass on unwired code (they guard preservation + keep-not-discard) — record which, honestly.
2. Add imports near the existing sqlx imports: `use sqlx::pool::PoolConnection;` and `use sqlx::Any;` (one module per `use`, matching lines 11–18 style).
3. Add file-private items after `SingleBindPlan` (before `plan_single_binds`):
   - `enum RollbackDisposition { Keep, Discard }` — keep = return connection to pool, discard = detach + close.
   - `const SQLITE_NO_ACTIVE_TRANSACTION: &str = "cannot rollback - no transaction is active";`
   - `fn classify_db_message(msg: &str) -> RollbackDisposition` — returns `Keep` iff `msg == SQLITE_NO_ACTIVE_TRANSACTION`, else `Discard`.
   - `fn classify_rollback_error(err: &sqlx::Error) -> RollbackDisposition` — `sqlx::Error::Database(db)` → `classify_db_message(db.message())`; `sqlx::Error::AnyDriverError(inner)` → downcast `inner` (it is `Box<dyn Error + Send + Sync>`) to `&sqlx::Error` and recurse; anything else → `Discard` (fail closed).
   - `struct DiscardOnDrop(Option<PoolConnection<Any>>)` with `impl Drop`: if `Some`, `conn.detach()` then drop the detached connection (closed, never re-pooled); `fn disarm(mut self) -> PoolConnection<Any>` returning `self.0.take().expect("guard disarmed twice")` so the caller drops it back into the pool; `fn conn_mut(&mut self) -> &mut PoolConnection<Any>` — first caller is step 4's helper.
4. Add the owning helper below the classifier/guard items:
   `async fn release_after_error(conn: PoolConnection<Any>, original: CamelError) -> CamelError` — wrap `conn` in `DiscardOnDrop(Some(conn))`; execute the rollback with `sqlx::query(sqlx::AssertSqlSafe("ROLLBACK")).execute(&mut **guard.conn_mut()).await` — double deref through the `&mut PoolConnection<Any>` accessor to `&mut AnyConnection`, the only `Executor` shape (`&mut **` is the same idiom as the existing `query.execute(&mut *conn)` sites); on `Ok(_)` or `Err(e)` with `classify_rollback_error(&e) == Keep` (log `debug!` with the error for the Keep case, log-policy comments): `let conn = guard.disarm(); drop(conn);` → return `original`. On `Err(e)` with `Discard` (log `warn!`, log-policy comments): drop the guard armed → return `original`. Never substitute or wrap the original error.
5. Wire `execute_modify`: replace the direct `plan_single_binds(...).await?` / `bind_single_item(...)?` / `query.execute(&mut *conn).await.map_err(...)?` chain with one fallible async block evaluated into `executed`; on `Ok(result)` continue exactly as today; on `Err(original)` return `Err(release_after_error(conn, original).await)`. Route the `expected_update_count` mismatch return (currently an early `return Err(...)`) through `release_after_error(conn, err).await` as well. Keep every existing error string byte-identical.
6. Wire `fetch_single_rows`: same pattern around plan + bind + `fetch_all` → `Err(original) => Err(release_after_error(conn, original).await)`.
7. Success paths stay untouched: no rollback on success, connection dropped normally.

**Tests:** (executable spec — name, arrange, act, assert)
- `classify_db_message_is_fail_closed`: no setup (pure fn) → call with the exact `SQLITE_NO_ACTIVE_TRANSACTION` string, with `"SQLITE_ERROR: cannot rollback - no transaction is active"`, with `"cannot rollback - no transaction is active!!"`, and with `""` → assert `Keep` for the exact string and `Discard` for all three others. Command: `cargo test -p camel-component-sql --lib classify_db_message_is_fail_closed`. Expected: green.
- `rollback_error_classifier_rejects_non_database_errors`: no setup → `classify_rollback_error(&sqlx::Error::Protocol("cannot rollback - no transaction is active (context)".to_string()))` → assert `Discard`. Command: `cargo test -p camel-component-sql --lib rollback_error_classifier_rejects_non_database_errors`. Expected: green.
- `cleanup_guard_discards_when_armed_keeps_when_disarmed`: setup `sqlite_pool().await` (max_connections=1) + `seed_items_table(&pool)` → (a) acquire one connection via `pool.acquire().await`, wrap in `DiscardOnDrop(Some(conn))`, drop the guard armed → assert `pool.size() == 0`; (b) acquire again, wrap, call `.disarm()`, drop the returned connection → assert `pool.size() == 1`. This test owns the spec's "Cancellation during cleanup discards the connection" scenario: cancellation drops the still-armed guard, which is exactly the armed branch asserted here. Command: `cargo test -p camel-component-sql --lib cleanup_guard_discards_when_armed_keeps_when_disarmed`. Expected: green.
- `failing_manual_begin_does_not_poison_the_pool`: as specified in step 1 → assert `Err` on the BEGIN batch, second call `Ok` within 2 s, `ex2` ROW_COUNT `json!(2)` (row 9 rolled back). Command: `systemd-run ... cargo test -p camel-component-sql --lib producer::tests::failing_manual_begin`. Expected: GREEN after steps 2–6.
- `failed_statement_without_transaction_keeps_error_and_pool_usable`: as specified → error preserved, `pool.size() == 1` immediately after failure, follow-up `Ok`. Expected: GREEN after wiring.
- `failing_select_keeps_error_and_pool_usable`: as specified → error preserved, `pool.size() == 1`, follow-up `Ok`. Expected: GREEN after wiring.

**Acceptance:**
- `cargo test -p camel-component-sql --lib` exits 0 (existing suite + 6 new tests).
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` exits 0 (valid here: machinery ships with its production caller in the same task).
- `cargo fmt --check --all` exits 0.
- `execute_batch` untouched: `git diff` shows no changes inside `execute_batch`.
- The task report quotes the RED output from step 1 and the GREEN output after wiring.

- [x] 1.1

### Task 1.2: wire the StreamList error paths + consumption-error coverage

**Files:**
- `crates/components/camel-sql/src/producer.rs` (modified)

**Steps:**
1. RED FIRST: add the regression test to `mod tests` (the discriminating open-transaction variant — no transaction is open without the BEGIN, so the plain variant passes identically before and after wiring), run it against the UNWIRED code, and record the output verbatim in the task report:
   - `stream_list_error_releases_connection_and_pool_stays_usable`: setup `sqlite_pool().await` + `seed_items_table(&pool)` → `config` with `&outputType=StreamList`; `PreparedQuery { sql: "BEGIN IMMEDIATE; INSERT INTO items (id,name) VALUES (9,'x'); select id, name from items order by id; INSERT INTO items (id,name) VALUES (1,'dup')".to_string(), bindings: vec![] }`; call `execute_select(...)` (returns Ok — the stream is lazy), then drain via `exchange.input.body.clone().into_bytes(1024).await` → assert that drain returns `Err` whose message contains `"UNIQUE"` (deterministic: sqlx-sqlite 0.9 runs all `;`-separated statements mid-stream); IMMEDIATELY after the drain error assert `pool.size() == 1`; then run `execute_select` with `"select id from items order by id"` (SelectList) → assert `Ok(())` and ROW_COUNT `json!(2)` (row 9 rolled back — on unwired code the leaked transaction makes the uncommitted row visible and this is `json!(3)`, which is the RED signal).
   Run: `systemd-run ... cargo test -p camel-component-sql --lib producer::tests::stream_list_error_releases`. Expected: RED on unwired code (ROW_COUNT `json!(3)`).
2. Wire the StreamList branch inside `execute_select` (the `async_stream::try_stream!` body): the acquire error keeps today's mapping (no connection exists yet). For the plan, bind, row-iteration, and row-serialization errors: on error, first `drop` whatever borrows the connection (the `rows` stream value for iteration errors — it is alive across loop iterations; `row_to_json`/serialization errors after a successful `try_next()` also drop `rows` first), then propagate macro-natively: `Err(release_after_error(conn, original).await)?;` — `try_stream!` only transforms `yield` and `?` (a plain `return Err(...)` does NOT compile in the macro body: E0308, the generated body returns `()`), and the `?` transform sends the error as the stream's final item and terminates. Preserve today's error strings (`fetch_error("Query execution failed", e)`, `"JSON serialization failed: ..."`) byte-identically.
3. Re-run the test and record the GREEN output.

**Tests:**
- `stream_list_error_releases_connection_and_pool_stays_usable`: as above → drain `Err` containing `"UNIQUE"`, `pool.size() == 1`, follow-up select `Ok` with ROW_COUNT `json!(2)`. Command: `systemd-run ... cargo test -p camel-component-sql --lib producer::tests::stream_list_error_releases`. Expected: GREEN after step 2 wiring; the task report must quote BOTH outputs (RED from step 1 on unwired code, GREEN from step 3).

**Acceptance:**
- `cargo test -p camel-component-sql --lib` exits 0.
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` exits 0; `cargo fmt --check --all` exits 0.
- `execute_select`'s SelectOne/SelectList paths unchanged by this task.
- The task report quotes the RED output from step 1 (unwired) and the GREEN output from step 3 (wired).

- [x] 1.2

## docs (database component)

### Task 2.1: docs paragraph, limitation record, local gate sweep

**Files:**
- `docs/src/components/database.md` (modified)
- `openspec/changes/sql-pool-error-rollback/parked-report.md` (new)

**Steps:**
1. In the `### Producer` section of the SQL component, append ONE short paragraph after the existing producer text: manual multi-statement transactions inside a single `sql:` call (e.g. `BEGIN; ...; COMMIT`) are safe against mid-statement failures — when a statement fails, the component rolls the connection back on its own pooled connection before returning it to the pool (and discards the connection if that rollback itself fails), so no open transaction leaks into the pool; note that `transactionMode=Managed` is still pending (bd rc-qv2iz) and must not be documented as working. Keep it to 3–5 sentences, ASD-STE100 plain style, English.
2. Write `openspec/changes/sql-pool-error-rollback/parked-report.md` recording: a live non-benign rollback failure is not deterministically forceable against a healthy SQLite connection with the existing harness (no fault-injection seam); the discard path is covered by the classifier unit tests (`classify_db_message_is_fail_closed`, `rollback_error_classifier_rejects_non_database_errors`) and the armed-guard unit test (`cleanup_guard_discards_when_armed_keeps_when_disarmed`); paste the RED/GREEN evidence for the poison and StreamList tests.
3. Run the local gate sweep from the worktree under the fleet systemd scope: `systemd-run --user --scope --collect --unit=fleet-sqltxpool bash scripts/gates-local.sh` → require exit 0; record `passed/failed/skipped` counts and `ran: X of Y` in the task report (network and docs-toolchain gates are CI-owned per the AGENTS.md sweep defaults).

**Tests:**
- Prose + report; verification is step 3's gate sweep exit code.

**Acceptance:**
- The paragraph exists in `docs/src/components/database.md` under `### Producer`, mentions rollback-on-own-connection, discard-on-rollback-failure, and that Managed is pending (rc-qv2iz), without describing Managed as available.
- `parked-report.md` exists with the limitation record + RED/GREEN evidence.
- `bash scripts/gates-local.sh` exit 0 from the worktree (recorded counts in the task report).
- No docs files other than `docs/src/components/database.md` touched.

- [x] 2.1
