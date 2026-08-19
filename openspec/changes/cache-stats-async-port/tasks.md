# Tasks: cache-stats-async-port

## Spec-coverage matrix (MODIFIED scenarios → owning task)

| Blessed scenario | Owning task | Evidence |
|---|---|---|
| get returns None on miss and Some on hit | 2.1 (regression) | existing `cache::memory`/`cache::redb` tests under `cargo test -p camel-core --lib cache::` |
| get returns None after in-band expiry, peek_stale returns the entry | 2.1 (regression) | `peek_stale_returns_post_expiry_entry_on_redb` (redb.rs) + memory equivalents |
| get surfaces backend failure as Err, never as silent miss | 2.1 (regression) | existing redb Contract C1 tests |
| set with None ttl stores entry without expiry | 2.1 (regression) | existing memory/redb set tests |
| invalidate is a no-op on absent key | 2.1 (regression) | existing memory/redb invalidate tests |
| stats returns hits/misses/evictions/entries snapshot for tracking backends | 2.1 | `stats_reflects_hits_misses_evictions_entries` (memory.rs:219), `stats_reports_bytes_sum` + `stats_counters_reported_alongside_bytes` (redb.rs) — all `.await` |
| non-tracking backend returns default zero stats | 1.1 | NEW `default_async_stats_returns_zeroed` (NoIter) |
| invalidate_prefix removes exactly the namespace on ordered backends | 2.1 (regression) | `cache_invalidate_prefix_purges_namespace_redb` (camel-test, run in 4.1) |
| invalidate_prefix default reports unsupported backends honestly | 1.1 (regression) | `default_invalidate_prefix_returns_err_naming_backend` (camel-api) |
| entries survive handle drop and reopen | 2.1 (regression) | existing redb reopen test |
| peek_stale returns post-expiry entry on redb | 2.1 (regression) | `peek_stale_returns_post_expiry_entry_on_redb` |
| sweep removes entries past stale_retention | 2.1 (regression) | existing redb sweep test |
| sweep stops on context shutdown | 2.1 (regression) | existing redb shutdown test |
| redb errors surface as Contract C1 Err | 2.1 (regression) | existing redb C1 test |
| stats computes bytes off the tokio worker | 2.1 | `stats_reports_bytes_sum` (`Some(8)` via `spawn_blocking` path); `bytes: None` degradation = disclosed residual gap |
| cache_clear empties the repository | 4.1 (regression) | `cache_clear_then_lookup_misses` (camel-test/tests/cache_admin_test.rs:34) |
| cache_stats emits a JSON snapshot body | 3.1 + 4.1 | `cache_stats_sets_json_body` (processor, exact key set) + `cache_stats_returns_json_snapshot` (camel-test:136, real backend) |
| cache_clear and cache_stats reach canonical parity | 4.1 (regression) | `cargo test -p camel-dsl --test schema_validation` + `cargo test -p camel-dsl --lib` (parity_tests covers `cache_stats`) |

## camel-api

### Task 1.1: Make `CacheRepository::stats` an async default method

**Files:**
- `crates/camel-api/src/cache.rs` (modified)

**Steps:**
1. In `pub trait CacheRepository` (crates/camel-api/src/cache.rs:69), change the `stats` method from `fn stats(&self) -> CacheStats` (line 117) to `async fn stats(&self) -> CacheStats` — it stays inside the existing `#[async_trait::async_trait]` block, keeps the default body `CacheStats::default()`, and stays infallible (no `Result`).
2. Update the method's doc comment to: "Return current cache statistics. Asynchronous so backends can offload I/O-bound byte accounting off the tokio worker (bd rc-22wj). Default implementation returns zeroed stats."
3. Add a test in the existing `mod tests`: `default_async_stats_returns_zeroed` — `#[tokio::test]`, instantiate `NoIter` (the existing test implementor that inherits the default `stats`), call `NoIter.stats().await`, assert every field is 0 and `bytes == None`.
4. Confirm the `NoIter` impl block itself needs NO change (it inherits the default method).

**Tests:** (executable spec — name, arrange, act, assert)
- `default_async_stats_returns_zeroed`: empty `NoIter` → `NoIter.stats().await` → all of `hits/misses/evictions/entries/peek_stale_served/invalidations == 0` and `bytes == None`. Command: `cargo test -p camel-api --lib cache::tests::default_async_stats_returns_zeroed`. Expected: fails before step 1 (`.await` on non-async), passes after.
- Regression: `default_invalidate_prefix_returns_err_naming_backend` and `cache_stats_serialize_round_trip` still pass untouched. Command: `cargo test -p camel-api --lib`.

**Acceptance:**
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `rg -n 'fn stats\(&self\)' crates/camel-api/src/cache.rs` shows `async fn stats` only.

- [x] 1.1

## camel-core

### Task 2.1: Async `stats` on both adapters — memory snapshot helper + redb `spawn_blocking` scan

**Files:**
- `crates/camel-core/src/cache/memory.rs` (modified)
- `crates/camel-core/src/cache/redb.rs` (modified)

**Steps:**
1. memory.rs: extract the body of `fn stats(&self) -> CacheStats` (line 133) into a new private method `fn stats_snapshot(&self) -> CacheStats` on `impl MemoryCacheRepository` (identical body; `bytes: None` stays).
2. memory.rs: change the trait impl to `async fn stats(&self) -> CacheStats { self.stats_snapshot() }`.
3. memory.rs: `impl std::fmt::Debug for MemoryCacheRepository` (line 33-39): change `.field("stats", &self.stats())` to `.field("stats", &self.stats_snapshot())` (Debug cannot await).
4. redb.rs: convert `fn total_bytes(&self) -> Option<u64>` (line 582) from a method into a private free function `fn total_bytes(db: &redb::Database) -> Option<u64>` with the identical scan body (`begin_read` + `open_table(CACHE_TABLE)` + `iter` + `serde_json::from_slice` + `saturating_add`). Update its doc comment: remove "(`stats()` is `&self`, synchronous, and infallible)" and state instead "Called only inside `spawn_blocking` from `stats()`; `None` when the table cannot be read or any entry fails to deserialize — the `bytes` field is a best-effort report, never an error."
5. redb.rs: change the trait impl `fn stats(&self) -> CacheStats` (line 563) to:
   - `async fn stats(&self) -> CacheStats`;
   - load the six eager counters exactly as today (`hits/misses/evictions/entries/peek_stale_served/invalidations` via `Ordering::Relaxed` loads);
   - clone the `Arc<redb::Database>` handle into `let db = Arc::clone(&self.db);`;
   - compute `bytes` via `tokio::task::spawn_blocking(move || total_bytes(&db)).await` mapping a join failure to `None` — write it as a `match` on the join result returning `Ok(v) => v` and `Err(_) => None`.
6. redb.rs test call sites: add `.await` to every `repo.stats()` call — lines 672, 708, 831 (entries assertions) and 854 (`stats_reports_bytes_sum`). All enclosing tests are already `#[tokio::test]`.
7. memory.rs test call sites: add `.await` to every `repo.stats()` call — lines 229, 245, 255, 276. All enclosing tests are already `#[tokio::test]`.

**Tests:** (executable spec)
- `stats_reports_bytes_sum` (redb.rs:838, updated with `.await`): repo with `set("a", 3-byte entry)` + `set("b", 5-byte entry)` → `repo.stats().await.bytes == Some(8)` — payload-sum semantics frozen. Command: `cargo test -p camel-core --lib cache::redb::tests::stats_reports_bytes_sum`. Expected: passes after the change (locks the regression).
- `stats_reflects_hits_misses_evictions_entries` (memory.rs:219, `.await` added): unchanged assertions still pass. Command: `cargo test -p camel-core --lib cache::memory`.
- New `stats_counters_reported_alongside_bytes` (redb.rs tests): repo after `set("a", 3-byte entry, None)` → `repo.stats().await` returns `entries == 1 && bytes == Some(3)`. Command: `cargo test -p camel-core --lib cache::redb::tests::stats_counters_reported_alongside_bytes`. Expected: passes after.
- Note (reviewer visibility): the `bytes: None` degradation branch (scan/join failure) is defensive plumbing inherited from the pre-change `total_bytes` Option contract; no unit test forces a redb read failure (unlinking the backing file does not fail an open mmap on Linux). Spec scenario's happy path is covered above.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0 (memory + redb modules).
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` exits 0 (proposal acceptance criterion).
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `rg -n 'fn stats\(&self\)' crates/camel-core/src/cache/` shows only `async fn stats` matches, and `rg -n 'fn stats_snapshot' crates/camel-core/src/cache/memory.rs` shows the private helper.
- `rg -n --pcre2 'repo\.stats\(\)(?!\.await)' crates/camel-core/src/cache/` returns zero hits (no un-awaited call remains; plain `repo.stats()` matches are all followed by `.await`).

- [x] 2.1

## camel-processor

### Task 3.1: `CacheStatsService` awaits `stats`; mock goes async; exact key-set assertion

**Files:**
- `crates/camel-processor/src/cache_eip.rs` (modified)

**Steps:**
1. In `impl OutcomePipeline for CacheStatsService::run` (line ~1000): change `let s = self.repository.stats();` to `let s = self.repository.stats().await;` — the call is already inside `Box::pin(async move)`. Do NOT change the `serde_json::json!` body: the eight keys `repository, hits, misses, evictions, entries, peek_stale_served, invalidations, bytes` stay exactly as-is.
2. In `mod test_utils` `MockCacheRepository` (line ~1195): change `fn stats(&self) -> CacheStats` to `async fn stats(&self) -> CacheStats` keeping the body `self.stats_override.lock().unwrap().clone()`.
3. Update the existing test `cache_stats_sets_json_body` (line ~2259): after the existing field assertions, add an exact-key-set assertion — deserialize the body JSON and assert its object key set equals exactly `{"repository","hits","misses","evictions","entries","peek_stale_served","invalidations","bytes"}` (e.g. compare `serde_json::Value::as_object().keys().collect::<BTreeSet<_>>()` against the expected set).

**Tests:** (executable spec)
- `cache_stats_sets_json_body` (updated): mock repo with `set_stats(CacheStats { hits: 2, misses: 1, invalidations: 1, .. })` → run `CacheStatsService` → body JSON has `"hits": 2`, `"misses": 1`, `"invalidations": 1`, `"bytes"` null-or-number, AND the key set is exactly the eight canonical keys (no extras, none missing). Command: `cargo test -p camel-processor --lib cache_eip::tests::cache_stats_sets_json_body`. Expected: passes after.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `rg -n 'self\.repository\.stats\(\)' crates/camel-processor/src/cache_eip.rs` shows `stats().await` only.

- [x] 3.1

## camel-config

### Task 4.1: Await `stats()` in config integration tests

**Files:**
- `crates/camel-config/tests/cache_repo_config.rs` (modified)

**Steps:**
1. Line 155: `let stats = repo.stats();` → `let stats = repo.stats().await;` (enclosing poll loop is inside a `#[tokio::test]` async fn).
2. Line 164: `repo.stats().entries` → `repo.stats().await.entries` (inside the `assert!` failure message argument).

**Tests:** (executable spec)
- `memory_max_capacity_supplied_via_config` (crates/camel-config/tests/cache_repo_config.rs:132, the test containing both edited lines): setup — context built with `max_capacity = 5`, ten entries inserted via `set(k0..k9)`; action — poll `repo.stats().await` up to 50 times at 10ms intervals; assert — `entries <= 5` before the poll budget ends. Command: `cargo test -p camel-config --test cache_repo_config`. Expected: passes after the edit; before the edit the file fails to compile (trait method now async).

**Acceptance:**
- `cargo test -p camel-config --test cache_repo_config` exits 0.
- `cargo test -p camel-test --test cache_admin_test` exits 0 (end-to-end regressions `cache_clear_then_lookup_misses` + `cache_stats_returns_json_snapshot` exercise the awaited step through real backends).
- `cargo test -p camel-dsl --test schema_validation` exits 0 and `cargo test -p camel-dsl --lib` exits 0 (canonical parity regressions).
- `cargo build --workspace --tests` exits 0 (workspace-wide compile check including integration-test targets — catches any call site missed by earlier tasks).

- [x] 4.1

## Docs

### Task 5.1: ADR-0056 amendment + CONTEXT refresh

**Files:**
- `docs/adr/0056-cache-repository-port.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-api/CONTEXT.md` (modified)
- `crates/camel-core/CONTEXT.md` (modified)
- `crates/camel-processor/CONTEXT.md` (modified)

**Steps:**
1. ADR-0056 §Interface stability (lines 240-250): append an amendment paragraph after the existing text: "Amendment (bd rc-22wj, pre-1.0): the `stats` method signature was corrected from sync `fn stats(&self) -> CacheStats` to `async fn stats(&self) -> CacheStats` (default body unchanged, still infallible). A synchronous signature made it structurally impossible for `RedbCacheRepository` to offload its payload-sum byte scan off the tokio worker. Call sites await; no twin sync/async pair was introduced. Ruled by escalation review (e_gpt) over the rejected twin-method and redb `stored_bytes()` alternatives."
2. CONTEXT-MAP.md line 119 (CacheRepository entry): after "`RedbCacheRepository` overrides it for namespace purges." insert "`stats` is an async default method (pre-1.0 signature correction, bd rc-22wj) — redb computes its `bytes: Some(payload-sum)` inside `spawn_blocking`."
3. crates/camel-api/CONTEXT.md line 171: change "`stats()` returns `CacheStats` with" to "`stats().await` (async default method, bd rc-22wj) returns `CacheStats` with".
4. crates/camel-core/CONTEXT.md line 91: change "redb reports `bytes: Some(sum)` while memory reports `bytes: None`" to "redb reports `bytes: Some(sum)` (scan inside `spawn_blocking`, bd rc-22wj) while memory reports `bytes: None`"; also change "Both implement `CacheRepository` from camel-api" to "Both implement `CacheRepository` from camel-api (async `stats`)".
5. crates/camel-processor/CONTEXT.md line 82 (export table row): change "clear, and stats" to "clear, and stats (awaited async `stats()` on the port, bd rc-22wj)".
6. Verify the ADR's implementation-reference table (line ~289) still cites valid line ranges for `CacheStats`; adjust the range only if the struct moved (it does not — this change touches no struct fields).

**Tests:** (executable spec)
- `cargo xtask lint-context-citations` exits 0 (CONTEXT files keep their citation discipline).
- `rg -n 'fn stats' docs/adr/0056-cache-repository-port.md` shows the amendment mentions `async fn stats`.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `cargo xtask lint-log-levels` exits 0 (docs touch cannot break it; run as smoke).
- All five files list `bd rc-22wj` or the async-`stats` fact per the exact edits above.

- [x] 5.1
