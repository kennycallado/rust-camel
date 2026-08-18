# Tasks: cache-admin

## Phase 1: Expose clear/stats + admin counters (P1)

**Coverage note (spec scenarios restated from the canonical spec):** the MODIFIED "CacheRepository port" scenarios for unchanged behavior (get miss/hit, in-band expiry + peek_stale, backend Err propagation, None-ttl, absent-key invalidate no-op, default zero stats) and the hit/miss OTel scenario are owned by EXISTING tests that this change must keep passing: `memory.rs` tests (`get_returns_none_on_miss_some_on_hit`, `stats_reflects_hits_misses_evictions_entries`, …), `redb.rs` tests, and `cache_eip.rs` OTel tests — gate `cargo test -p camel-core --lib cache::` + `cargo test -p camel-processor --lib` in every Phase 1 task.

### camel-api

#### Task 1.1: CacheStats extension + canonical step variants

**Files:**
- `crates/camel-api/src/cache.rs` (modified)
- `crates/camel-api/src/runtime.rs` (modified)
- `crates/camel-core/src/cache/memory.rs` (modified — mechanical literal fix)
- `crates/camel-core/src/cache/redb.rs` (modified — mechanical literal fix)
- `schemas/canonical-route-spec.json` (modified — regenerated)
- `schemas/ts/CanonicalStepSpec.ts` (modified — regenerated)

**Steps:**
1. In `CacheStats` (cache.rs:47) add fields `peek_stale_served: u64`, `invalidations: u64`, `bytes: Option<u64>` with doc comments ("Number of peek_stale serves (fresh or stale)", "Number of successful invalidation operations", "Total stored payload bytes when the backend can report it; None = cannot"). Add `serde::Serialize, serde::Deserialize` to the existing derive list. Keep `Default` derive (`Option<u64>` defaults to `None`).
2. Update the existing `cache_stats_default` test literal (cache.rs:119) AND the backend `stats()` literals (memory.rs:123, redb.rs:450) with the new fields set to `peek_stale_served: 0, invalidations: 0, bytes: None` — placeholder zeros only, real counting lands in 1.2 (keeps the workspace compiling).
3. In `CanonicalStepSpec` (runtime.rs:117), add two variants next to `CacheInvalidate`: `CacheClear { repository: Option<String> }` and `CacheStats { repository: Option<String> }`, matching the serde attributes of the neighboring `Cache`/`CacheInvalidate` variants.
4. In `validate_steps` (runtime.rs:463-472) add `CanonicalStepSpec::CacheClear { .. } | CanonicalStepSpec::CacheStats { .. }` to the no-op arm.
5. Update every in-workspace `CanonicalStepSpec` match that becomes non-exhaustive — grep for matches WITHOUT catch-all arms on `CanonicalStepSpec` (note: camel-dsl/compile.rs:4878/:4897 and camel-core/tests/peek_stale_on_miss_test.rs:133 use catch-all arms and need NO change).
6. Regenerate the committed schema artifacts: run `cargo xtask schema` (rewrites `schemas/canonical-route-spec.json`, `schemas/dsl/`, `schemas/ts/CanonicalStepSpec.ts`) and commit the drift.

**Tests:**
- `cache_stats_serialize_round_trip`: a `CacheStats { hits: 2, misses: 1, evictions: 0, entries: 3, peek_stale_served: 4, invalidations: 1, bytes: None }` → `serde_json::to_string` → `from_str` → assert `PartialEq` equality and that the JSON contains `"peek_stale_served"` and `"bytes":null`. Command: `cargo test -p camel-api --lib cache_stats`. Expected: fails before the fields/derive exist, passes after.
- `canonical_cache_clear_stats_round_trip`: build `CanonicalStepSpec::CacheClear { repository: Some("persistent".into()) }` and `CanonicalStepSpec::CacheStats { repository: None }`, serde round-trip each (pattern of `canonical_cache_peek_stale_on_miss_round_trip` at runtime.rs:988), assert equality and that serialization carries a `cache_clear`/`cache_stats` discriminant. Command: `cargo test -p camel-api --lib canonical_cache`. Expected: fails before variants exist, passes after.

**Acceptance:**
- `cargo check --workspace --all-targets` exits 0 (tests included).
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api --all-features -- -D warnings` exits 0.
- `cargo xtask schema --check` exits 0 (regenerated artifacts committed).

- [x] 1.1

### camel-core backends

#### Task 1.2: Backend counters (peek_stale_served, invalidations) + redb bytes

**Files:**
- `crates/camel-core/src/cache/memory.rs` (modified)
- `crates/camel-core/src/cache/redb.rs` (modified)

**Steps:**
1. In both backends add `peek_stale_served: Arc<AtomicU64>` and `invalidations: Arc<AtomicU64>` following the existing `hits`/`misses` pattern (memory.rs:26-27, redb.rs:55-56).
2. Increment `peek_stale_served` in `peek_stale` when it returns `Ok(Some(_))` (memory and redb).
3. Increment `invalidations` by 1 on every successful `invalidate` call (per-operation count, not per-entry).
4. Update both `stats()` literals (memory.rs:122, redb.rs:~450) with the new fields; memory sets `bytes: None`; redb computes `bytes` as `Some(sum)` by iterating the full table range at `stats()` time and summing each entry's `bytes.len()`.
5. Keep the existing `stats_reflects_hits_misses_evictions_entries` assertions passing (extend the literal sites it relies on).

**Tests:**
- `stats_reports_peek_stale_served` (memory.rs tests): `set("k", entry, Some(1ms))`, sleep 10ms, `get` (miss/expired), `peek_stale("k")` → `stats().peek_stale_served == 1`. Command: `cargo test -p camel-core --lib cache::memory`. Expected: fails before, passes after.
- `stats_reports_invalidations_per_operation` (memory.rs tests): `set("a", …)`, `invalidate("a")`, `invalidate("absent")` → `stats().invalidations == 2`. Command: `cargo test -p camel-core --lib cache::memory`. Expected: fails before, passes after.
- `stats_reports_bytes_sum` (redb.rs tests, temp-file redb): `set("a", 3-byte entry)`, `set("b", 5-byte entry)` → `stats().bytes == Some(8)`. Command: `cargo test -p camel-core --lib cache::redb`. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-core --lib cache::` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 1.2

### camel-processor

#### Task 1.3: CacheClearService, CacheStatsService, peek/invalidate counters

**Files:**
- `crates/camel-processor/src/cache_eip.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified — constructor call sites only)

**Steps:**
1. Add `CacheClearService { repository: Arc<dyn CacheRepository> }` implementing `OutcomePipeline` (pattern of `CacheInvalidateService` at cache_eip.rs:369): `run` calls `repository.clear().await`; `Err(e)` → `Failed(e)`; `Ok(())` → `Completed(exchange)` with body unchanged. Provide `Clone` + `clone_box` following `CacheInvalidateService`.
2. Add `CacheStatsService { repository: Arc<dyn CacheRepository>, repository_name: String }`: `run` calls `repository.stats()` (sync), builds `serde_json::json!({ "repository": self.repository_name, "hits": s.hits, "misses": s.misses, "evictions": s.evictions, "entries": s.entries, "peek_stale_served": s.peek_stale_served, "invalidations": s.invalidations, "bytes": s.bytes })`, replaces `exchange.input.body` with the Json body (same `Body` variant `reconstruct_body` produces for `ContentType::Json`), returns `Completed`.
3. Extend `CachePeekStaleService` with `rt: Arc<dyn RuntimeObservability>` + `repository_name: String` (pattern of `CacheService` fields, cache_eip.rs:~60). On the entry-present path emit `rt.metrics().record_counter("camel.cache.peek_stale_served", 1.0, &[("repository", &self.repository_name)])` before body reconstruction. Update its `Clone` impl.
4. Extend `CacheInvalidateService` with `rt` + `repository_name`; on successful `invalidate` emit `record_counter("camel.cache.invalidations", 1.0, &[("repository", …)])`. Update `Clone`.
5. Update the `CachePeekStaleService::new`/`CacheInvalidateService::new` call sites in `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs:199` (peek) and `:178` (invalidate) to pass the observability handle the `Cache` arm already resolves and `repo.name().to_string()`.
6. Extend `test_utils::MockCacheRepository` with `clear_call_count: AtomicU64` (incremented in `clear`) and a `set_stats(CacheStats)` override feeding a field returned by `stats()`.

**Tests:**
- `cache_clear_calls_repository_clear`: MockCacheRepository with a seeded entry → run `CacheClearService` → assert `Completed`, `clear_call_count() == 1`, and `stored_entry("k").await.is_none()`. Command: `cargo test -p camel-processor --lib cache_clear`. Expected: fails before, passes after.
- `cache_clear_err_propagates_failed`: Mock configured to fail `clear` (extend the mock's failure injection like `set_should_fail_set`) → run → assert `Failed`. Command: `cargo test -p camel-processor --lib cache_clear`. Expected: fails before, passes after.
- `cache_stats_sets_json_body`: Mock with `set_stats(CacheStats { hits: 2, misses: 1, evictions: 0, entries: 3, peek_stale_served: 4, invalidations: 1, bytes: None })` → run `CacheStatsService::new(repo)` (name derives; mock name is "mock") → assert `Completed` and the body is JSON with `"repository":"mock"`, `"hits":2`, `"bytes":null`. Command: `cargo test -p camel-processor --lib cache_stats`. Expected: fails before, passes after.
- `peek_stale_hit_emits_peek_served_counter`: Mock seeded + `RecordingMetricsCollector` (cache_eip.rs:1541) → run peek service → assert one `camel.cache.peek_stale_served` counter with `repository=mock` label. Command: `cargo test -p camel-processor --lib peek_stale_hit_emits`. Expected: fails before, passes after.
- `invalidate_emits_invalidations_counter`: Mock seeded → run invalidate service → assert one `camel.cache.invalidations` with `repository=mock`. Command: `cargo test -p camel-processor --lib invalidate_emits`. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 1.3

### camel-core + camel-builder wiring

#### Task 1.4: BuilderStep variants, compiler arms, canonical converter

**Files:**
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified — BuilderStep enum lives here, :62; cache variants :281-296)
- `crates/camel-builder/src/lib.rs` (modified — `step_name` arms only)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)
- `crates/camel-core/src/lifecycle/application/commands.rs` (modified)
- `crates/camel-core/src/lifecycle/application/commands_tests.rs` (modified)

**Steps:**
1. Add `BuilderStep::CacheClear { repository: Option<String> }` and `BuilderStep::CacheStats { repository: Option<String> }` to the enum in `route_definition.rs` (next to `CacheInvalidate`, :290) and their `step_name` arms in camel-builder/src/lib.rs returning `"cache_clear"` / `"cache_stats"` (lib.rs:1300-1307).
2. Add compiler arms in `step_compilers/core.rs` following the `CacheInvalidate` arm (core.rs:170-184): resolve `ctx.cache_repositories.get(repo_name)` (default `"memory"`, unknown → `ComponentNotFound` naming `cache_clear`/`cache_stats` and the repository), construct `CacheClearService::new(repo)` / `CacheStatsService::new(repo)` (repository_name derives internally since the 1.3-review fix), return `CompileOutcome::Matched(CompiledStep::Segment { segment, body_contract: None, lifecycle: None })`.
3. Add canonical→builder arms in `commands.rs` next to `CanonicalStepSpec::CacheInvalidate` (commands.rs:1093): map `CacheClear { repository }` → `BuilderStep::CacheClear { repository }`, same for `CacheStats`.
4. Add the same two variants to every exhaustive `BuilderStep` match that the new variants make non-exhaustive — grep for matches WITHOUT catch-all arms on `BuilderStep` (note: camel-dsl/compile.rs:4878/:4897 and camel-core/tests/peek_stale_on_miss_test.rs:133 use catch-all arms and need NO change; the compiler arms in core.rs are handled by step 2).

**Tests:**
- `canonical_cache_clear_and_stats_convert` (commands_tests.rs, pattern of existing canonical tests): `CanonicalStepSpec::CacheClear { repository: Some("memory".into()) }` → converter → assert `BuilderStep::CacheClear { repository: Some("memory".into()) }`; same for `CacheStats { repository: None }` → `BuilderStep::CacheStats { repository: None }`. Command: `cargo test -p camel-core --lib canonical_cache_clear`. Expected: fails before, passes after.
- `cache_clear_unknown_repository_fails_compile` (core.rs tests if present, else commands_tests.rs): builder step with `repository: Some("nope")` against a compile ctx without it → assert `ComponentNotFound` mentioning `cache_clear` and `nope`. Command: `cargo test -p camel-core --lib cache_clear_unknown_repository`. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 1.4

### camel-dsl

#### Task 1.5: DSL AST, compile paths, schema, parity

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-dsl/src/parity_tests.rs` (modified)
- `crates/camel-dsl/tests/schema_validation.rs` (modified)
- `schemas/dsl/route-schema.json` (modified — regenerated)
- `schemas/canonical-route-spec.json` (modified — regenerated)

**Steps:**
1. In `route_ast.rs` add `CacheClearStep { cache_clear: CacheClearBody }` / `CacheClearBody { repository: Option<String> }` and `CacheStatsStep { cache_stats: CacheStatsBody }` / `CacheStatsBody { repository: Option<String> }`, with the same derives + `deny_unknown_fields` as `CacheInvalidateStep` (route_ast.rs:1160-1176). Add `CacheClear(CacheClearStep)` / `CacheStats(CacheStatsStep)` variants to `RouteDslStep` (route_ast.rs:479-481).
2. In `model.rs` add `CacheClearStepDef { repository: Option<String> }` / `CacheStatsStepDef { repository: Option<String> }` next to `CacheInvalidateStepDef` (model.rs:494) and wire them into the AST→model conversion where `CacheInvalidateStepDef` is produced.
3. In `compile.rs` add DSL→`BuilderStep` arms (`CacheClear`/`CacheStats` → the new builder variants) and DSL→canonical arms (→ `CanonicalStepSpec::CacheClear`/`CacheStats`), plus `step_kind` name arms (`"cache_clear"`/`"cache_stats"`, compile.rs:1634 pattern). Also add canonical→builder arms in `compile_canonical_step` (compile.rs:494-530, the `_ => Err("unsupported canonical step")` fail-closed match) mapping `CanonicalStepSpec::CacheClear/CacheStats` to the builder variants — otherwise canonical-form routes using the new steps are rejected on the DSL path.
4. Add parity tests in `parity_tests.rs` mirroring the existing cache parity entries: YAML `cache_clear: { repository: memory }` ↔ canonical, YAML `cache_stats: {}` ↔ canonical.
5. Add YAML smokes to `tests/schema_validation.rs` next to the existing `cache_peek_stale` case (schema_validation.rs:97): one valid route containing `cache_clear` and `cache_stats`, and one rejected case with an unknown field (`cache_clear: { repo: x }` → deserialization error).

**Tests:**
- `cache_clear_and_stats_compile` (compile.rs tests, pattern `cache_invalidate_and_peek_stale_compile` at compile.rs:4837): `DeclarativeStep::CacheClear`/`CacheStats` → `compile_declarative_step_to_canonical` → assert `CanonicalStepSpec::CacheClear { .. }` / `CacheStats { .. }` matches. Command: `cargo test -p camel-dsl`. Expected: fails before, passes after.
- `cache_clear_stats_parity` (parity_tests.rs): YAML route string containing both steps → compile → canonical JSON → recompile → assert identical `CanonicalRouteSpec`. Command: `cargo test -p camel-dsl cache_clear_stats_parity`. Expected: fails before, passes after.
- `schema_validation.rs` accepts the valid cache-admin route and rejects the unknown-field route. Command: `cargo test -p camel-dsl --test schema_validation`. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-dsl` exits 0.
- `cargo clippy -p camel-dsl --all-features -- -D warnings` exits 0.
- `cargo xtask schema` regenerated + `cargo xtask schema --check` exits 0 (route_ast schema derives feed `schemas/dsl/`).

- [x] 1.5

### integration + docs

#### Task 1.6: camel-test integration + CONTEXT-MAP

**Files:**
- `crates/camel-test/tests/cache_admin_test.rs` (new)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. New integration test file using the camel-test harness patterns from `cache_eip_smoke.rs`: build a context with the default memory repository and a route whose `cache: { repository: memory, key: "k", on_miss: [ <set body "v1"> ] }` step runs, followed by probes.
2. Test A (clear): warm the cache via one exchange (on_miss runs), send an exchange through a route with `cache_clear: { repository: memory }`, then a fresh lookup exchange on the cache route → assert `on_miss` ran again (counter via a shared AtomicU64 in a custom on_miss step or an inline set-body + route assertion pattern already used in smoke tests).
3. Test B (stats): after known operations (2 hits, 1 miss, 1 `cache_invalidate` on a seeded key), run `cache_stats: { repository: memory }` → assert the resulting exchange body parses as JSON with `"hits": 2`, `"misses": 1`, `"invalidations": 1`, `"repository": "memory"`, and `"bytes"` present with value null.
4. Update `CONTEXT-MAP.md` line 117 (Cache EIP entry): extend the step list to `cache`/`cache_invalidate`/`cache_peek_stale`/`cache_clear`/`cache_stats` and the services (`CacheClearService`/`CacheStatsService`).

**Tests:**
- `cache_clear_then_lookup_misses`: warmed entry + `cache_clear` step → next cache-step exchange re-runs on_miss. Command: `cargo test -p camel-test --test cache_admin_test`. Expected: fails before, passes after.
- `cache_stats_returns_json_snapshot`: 2 hits + 1 miss → `cache_stats` exchange body JSON asserts `hits=2`, `misses=1`, `invalidations=1`, `repository="memory"`, `bytes` null. Command: `cargo test -p camel-test --test cache_admin_test cache_stats_returns`. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-test --test cache_admin_test` exits 0.
- `cargo xtask lint-context-citations` exits 0 (CONTEXT-MAP edit keeps citation format).

- [x] 1.6

## Phase 2: Namespace invalidation + singleflight (P2)

### camel-api

#### Task 2.1: invalidate_prefix default method + canonical/builder surface (shape change)

**Files:**
- `crates/camel-api/src/cache.rs` (modified)
- `crates/camel-api/src/runtime.rs` (modified)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified — BuilderStep shape)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified — transitional arm fixes)
- `crates/camel-core/src/lifecycle/application/commands.rs` (modified)
- `crates/camel-core/src/lifecycle/application/commands_tests.rs` (modified — test constructions at :612/:646/:728/:737)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-core/tests/peek_stale_on_miss_test.rs` (modified — exhaustive match at :133)
- `schemas/canonical-route-spec.json` (modified — regenerated)
- `schemas/ts/CanonicalStepSpec.ts` (modified — regenerated)

**Steps:**
1. Add to `CacheRepository` (inside the existing `#[async_trait::async_trait]` block) a default method:
   `async fn invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError> { Err(CamelError::Config(format!("cache backend '{}' does not support invalidate_prefix (no key iteration)", self.name()))) }`
2. Change `CanonicalStepSpec::CacheInvalidate { repository, key }` (runtime.rs:160) to `{ repository: Option<String>, key: Option<String>, key_prefix: Option<String> }` with `#[serde(default, skip_serializing_if = "Option::is_none")]` on `key_prefix` (and on `key`) so the wire form `{"key": "k"}` still deserializes.
3. Add `coalesce_misses: Option<bool>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` to `CanonicalStepSpec::Cache` (runtime.rs:153).
4. Change `BuilderStep::CacheInvalidate` (route_definition.rs:290) to `{ repository: Option<String>, key: Option<LanguageExpressionDef>, key_prefix: Option<LanguageExpressionDef> }` and add `coalesce_misses: bool` to `BuilderStep::Cache` — do this in THIS task so canonical and builder shapes land together.
5. Update every in-workspace construction/match site of the changed variants so the workspace compiles with tests: `commands.rs:1093` (CacheInvalidate arm — construct `key: key.map(|k| camel_api::LanguageExpressionDef { language: "simple".into(), source: k })`, `key_prefix: key_prefix.map(|p| camel_api::LanguageExpressionDef { language: "simple".into(), source: p })`; Cache arm adds `coalesce_misses: coalesce_misses.unwrap_or(false)`), the COMPILER ARMS in `step_compilers/core.rs` (`BuilderStep::Cache` arm at :125 destructures named fields — add `coalesce_misses` to the pattern and ignore it for now; `CacheInvalidate` arm at :170 — make the pattern tolerate `Option` fields by rejecting the transitional state: match `key: Some(key), key_prefix: None` to the existing behavior and return `Err(CamelError::Config("cache_invalidate: exactly one of 'key' or 'key_prefix' is required"))` for both-None; both-Some stays unreachable until 2.5), `commands_tests.rs` test constructions (:612/:646/:728/:737), and `camel-dsl/src/compile.rs` DSL→canonical AND canonical→builder arms (the `compile_canonical_step` match at :494-530). This task makes it COMPILE; behavior lands in 2.3/2.4/2.5/2.6.
6. Regenerate schema artifacts (`cargo xtask schema`) and commit the drift.

**Tests:**
- `default_invalidate_prefix_returns_err_naming_backend` (cache.rs tests): a minimal `struct NoIter;` implementing `CacheRepository` with `name() == "noiter"` and stub methods; `invalidate_prefix("ns:")` → assert `Err` whose message contains `noiter`. Command: `cargo test -p camel-api --lib invalidate_prefix`. Expected: fails before, passes after.
- `canonical_cache_invalidate_prefix_round_trip` (runtime.rs tests): `CacheInvalidate { repository: Some("persistent".into()), key: None, key_prefix: Some("ns:".into()) }` serde round-trip; also assert the legacy JSON `{"repository":null,"key":"k"}` still deserializes with `key_prefix: None`. Command: `cargo test -p camel-api --lib canonical_cache_invalidate_prefix`. Expected: fails before, passes after.

**Acceptance:**
- `cargo check --workspace --all-targets` exits 0 (tests included).
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api --all-features -- -D warnings` exits 0.
- `cargo xtask schema --check` exits 0.

- [x] 2.1

### camel-core backends

#### Task 2.2: redb invalidate_prefix override

**Files:**
- `crates/camel-core/src/cache/redb.rs` (modified)
- `crates/camel-core/src/cache/memory.rs` (modified — tests only)

**Steps:**
1. Override `invalidate_prefix` in `RedbCacheRepository`: open a read txn, range-scan the table (keys are `&str`, redb.rs:41 — UTF-8 scalars) from `prefix` to its successor bound, collect matching keys, then delete them in one write txn; return the deleted count. Successor computation (small pure helper `fn successor_bound(prefix: &str) -> Bound<String>` with unit tests): take the last `char` c; if `c < U+D7FF` → increment to `c+1`; if `c == U+D7FF` → jump to `U+E000` (surrogate gap); if `U+E000 <= c < U+10FFFF` → increment to `c+1` (e.g. `U+E000 → U+E001`); if `c == U+10FFFF` → carry (drop the last char and recurse on the remainder); if every char is `U+10FFFF` → return `Bound::Unbounded`. Increment the backend `invalidations` stat by 1 (per-operation, matching `invalidate`).
2. Add one memory.rs test documenting the default-Err contract (no impl change — memory inherits the default).

**Tests:**
- `invalidate_prefix_removes_namespace_only` (redb.rs tests, temp file): seed `rainviewer:a`, `rainviewer:b`, `gibs:a` → `invalidate_prefix("rainviewer:")` returns `Ok(2)`; `get("rainviewer:a")`/`get("rainviewer:b")` miss; `get("gibs:a")` still hits. Command: `cargo test -p camel-core --lib cache::redb`. Expected: fails before, passes after.
- `successor_bound_unit_tests` (redb.rs tests, pure helper): `"ns:"` → `Bound::Excluded("ns;")`; a prefix ending `U+D7FF` → jumps to `U+E000` (no surrogate produced); a prefix ending `U+E000` → `U+E001`; suffix-carry `"a\u{10FFFF}"` → `"b"`; a prefix of all `U+10FFFF` chars → `Bound::Unbounded`. Command: `cargo test -p camel-core --lib successor_bound`. Expected: fails before, passes after.
- `invalidate_prefix_empty_prefix_removes_all_seeded` (redb.rs tests): seed 3 entries across namespaces → `invalidate_prefix("")` returns `Ok(3)`. Command: `cargo test -p camel-core --lib cache::redb`. Expected: fails before, passes after.
- `invalidate_prefix_empty_namespace_returns_zero` (redb.rs tests): empty table → `Ok(0)`. Command: `cargo test -p camel-core --lib cache::redb`. Expected: fails before, passes after.
- `invalidate_prefix_on_memory_fails_naming_backend` (memory.rs tests): `invalidate_prefix("ns:")` → `Err` containing the repository name (default method). Command: `cargo test -p camel-core --lib cache::memory`. Expected: passes immediately (default behavior) — the test pins the contract.

**Acceptance:**
- `cargo test -p camel-core --lib cache::` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 2.2

### camel-processor

#### Task 2.3: CacheInvalidateService prefix support + CamelCacheInvalidatedCount

**Files:**
- `crates/camel-processor/src/cache_eip.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified — call site only)

**Steps:**
1. Add `pub const CAMEL_CACHE_INVALIDATED_COUNT: &str = "CamelCacheInvalidatedCount";`.
2. Replace `CacheInvalidateService`'s `key_expr` with `target: CacheInvalidateTarget` where `pub enum CacheInvalidateTarget { Key(MessageIdExpression), Prefix(MessageIdExpression) }`. `run`:
   - `Key`: expression `None` → `Completed`; `Some(k)` → `invalidate(&k)`; success → set `CAMEL_CACHE_INVALIDATED_COUNT` to `serde_json::Value::from(1u64)` + emit `camel.cache.invalidations` +1 → `Completed`; `Err` → `Failed`.
   - `Prefix`: expression `None` → `Completed`; `Some(p)` → `invalidate_prefix(&p)`; success → set the property to the returned count + counter +1 → `Completed`; `Err` → `Failed` (unsupported backend surfaces as failure — fail-closed).
3. Update `Clone` impl and the construction call site in `step_compilers/core.rs:178` to build `CacheInvalidateTarget::Key(key_expr)` (prefix wiring lands in 2.5).
4. Extend `MockCacheRepository` with a seeded-prefix-deletions map OR make `invalidate_prefix` remove every mock key starting with the prefix and return the count (mock models an ordered backend), plus a `set_prefix_unsupported(bool)` flag forcing the default-style `Err` for the fail-closed test.

**Tests:**
- `cache_invalidate_prefix_removes_namespace_sets_count`: mock seeded `ns:one`, `ns:two`, `other:x` → run Prefix service with expression resolving `"ns:"` → `Completed`, property `CamelCacheInvalidatedCount == 2`, both `ns:*` gone, `other:x` present, one `camel.cache.invalidations` counter recorded. Command: `cargo test -p camel-processor --lib cache_invalidate_prefix`. Expected: fails before, passes after.
- `cache_invalidate_prefix_none_expr_completes`: Prefix with `none_key()` → `Completed`, no invalidate calls. Command: `cargo test -p camel-processor --lib cache_invalidate_prefix_none`. Expected: fails before, passes after.
- `cache_invalidate_prefix_unsupported_fails_closed`: mock with `set_prefix_unsupported(true)` → run Prefix → `Failed` whose error names the backend. Command: `cargo test -p camel-processor --lib cache_invalidate_prefix_unsupported`. Expected: fails before, passes after.
- `cache_invalidate_key_sets_count_one`: existing exact-key path now also sets `CamelCacheInvalidatedCount == 1` (extend `cache_invalidate_calls_repository_invalidate`). Command: `cargo test -p camel-processor --lib cache_invalidate_calls`. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 2.3

#### Task 2.4: CacheService singleflight (coalesce_misses)

**Files:**
- `crates/camel-processor/src/cache_eip.rs` (modified)

**Steps:**
1. Add to cache_eip.rs:
   - `pub(crate) enum CoalesceTerminal { Completed(Body), Failed(CamelError), Stopped }`
   - `struct InFlight { terminal: std::sync::Mutex<Option<CoalesceTerminal>>, notify: tokio::sync::Notify }`
   - `type InFlightMap = std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<InFlight>>>`
2. Extend `CacheService` with `coalesce_misses: bool` and `inflight: Arc<InFlightMap>` (default empty map; `new` keeps current signature — add `with_coalesce(self, coalesce_misses: bool)` builder method; `Clone` clones the Arcs).
3. Leader path (coalesce on, MISS): under the map lock insert the key if absent (else this exchange is a waiter — see step 4). Leader wraps its future body in a cancellation guard struct `LeaderGuard { key: String, map: Arc<InFlightMap>, cell: Arc<InFlight> }` implementing `Drop`: publish `CoalesceTerminal::Failed(CamelError::Config("cache coalesce leader cancelled".into()))` into the slot if still empty, `notify_waiters()`, remove the map entry ONLY when `Arc::ptr_eq(cell, map_entry)` (identity check — a late guard must not evict a newer wave's entry). Leader runs the existing MISS flow (on_miss + write-back `set`); on terminal outcome publish the matching `CoalesceTerminal` (`Completed(resulting_body)`, `Failed(e)`, `Stopped`) under the slot lock BEFORE `notify_waiters()`, then remove the map entry with the same `Arc::ptr_eq` identity check, then return its own outcome unchanged. The slot is write-once: once filled it is never cleared or overwritten.
4. Waiter path: found an existing in-flight entry under the lock. Register BEFORE checking the slot using the pinned-Notified enable pattern — while STILL holding the map lock: `let mut notified = cell.notify.notified(); tokio::pin!(notified); notified.as_mut().enable();` — THEN release the lock and read the slot; if already filled, clone-read the terminal; if empty, await `notified` and re-read. All waiters CLONE-READ the terminal (`Body` and `CamelError` are `Clone`); no waiter consumes/removes it, so N waiters all receive the terminal state: `Completed(body)` → set own `exchange.input.body = body.clone()` → `Completed`; `Failed(e)` → `Failed(e.clone())`; `Stopped` → `Stopped(own exchange)`.
5. HIT path, key-`None` path, and `coalesce_misses == false` bypass the map entirely (identical to current behavior).
6. Waiters do NOT call `set` (the leader's write-back is the single write).

**Tests:**
- `coalesce_three_concurrent_misses_fetch_once`: two Notify gates — `leader_entered` (on_miss signals `leader_entered.notify_one()` immediately after starting) and `release` (on_miss then awaits `release.notified()` before setting the body and bumping the invocation counter). Test: `tokio::spawn` the leader; await `leader_entered.notified()` (deterministic proof the leader is inside on_miss); `tokio::spawn` both waiters on the SAME service clone; assert each waiter is parked (`tokio::time::timeout(50ms, &mut waiter_handle)` returns `Err(Elapsed)` for both — borrow the handle so it stays awaitable); `release.notify_waiters()`; await all 3 handles → all `Completed` with the fetched body; on_miss invocation counter == 1; mock `set` call count == 1. Command: `cargo test -p camel-processor --lib coalesce_three`. Expected: fails before, passes after.
- `coalesce_leader_failure_fails_waiters_once`: same two gates; on_miss signals `leader_entered`, awaits `release.notified()`, THEN returns `Failed(CamelError)` (so waiters register while the wave is in flight); await `leader_entered`, spawn 2 waiters, prove both parked (`timeout(50ms, &mut handle)` Elapsed), `release.notify_waiters()`, await all → all 3 `Failed` with equal error display strings; on_miss ran once; `set` never called. Command: `cargo test -p camel-processor --lib coalesce_leader_failure`. Expected: fails before, passes after.
- `coalesce_leader_stopped_stops_waiters`: same two gates; on_miss signals `leader_entered`, awaits `release.notified()`, then returns `Stopped`; await `leader_entered`, spawn 1 waiter, prove it parked, release, await both → leader `Stopped(leader exchange)`, waiter `Stopped(waiter exchange)` (its own exchange object, body untouched). Command: `cargo test -p camel-processor --lib coalesce_leader_stopped`. Expected: fails before, passes after.
- `coalesce_leader_dropped_does_not_strand_waiters`: same gate setup; await `leader_entered.notified()`, spawn the waiter, assert the waiter is parked (`timeout(50ms, &mut waiter_handle)` Elapsed), then `leader_handle.abort()`; the waiter completes within `tokio::time::timeout(1s, waiter_handle)` with `Failed` (cancellation terminal); assert `inflight.lock().unwrap().is_empty()`. Command: `cargo test -p camel-processor --lib coalesce_leader_dropped`. Expected: fails before, passes after.
- `no_coalesce_runs_per_exchange`: `coalesce_misses == false` (default `new`), 3 concurrent misses (plain join, no gate) → on_miss ran 3 times, `set` called 3 times. Command: `cargo test -p camel-processor --lib no_coalesce`. Expected: passes before AND after (regression pin).

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- No `unwrap()` outside `#[cfg(test)]` (guard uses `match`/`?`-style handling on the slot lock; `cargo xtask lint-unwrap` clean).

- [x] 2.4

### camel-core + camel-builder wiring

#### Task 2.5: Compiler prefix/coalesce wiring + compile-time validation

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified; compile-level tests landed in this file's tests — compile registry ctx required)
- `crates/camel-core/src/lifecycle/application/commands_tests.rs` (modified — test constructions at :612/:646/:728/:737)

**Steps:**
1. Compiler arm (core.rs:170): if both `key` and `key_prefix` are `Some`, or both `None`, return `Err(CamelError::Config("cache_invalidate: exactly one of 'key' or 'key_prefix' is required".into()))`. Build `CacheInvalidateTarget::Key(expr)` or `::Prefix(expr)` via `compile_message_id_expression`. (The `BuilderStep::CacheInvalidate`/`Cache` shape change landed in 2.1 — this task only adds behavior.)
2. Cache compiler arm: append `.with_coalesce(coalesce_misses)` to the `CacheService::new` call in the `BuilderStep::Cache` arm (core.rs:~155).
3. Verify no other `BuilderStep::CacheInvalidate`/`Cache` construction sites remain unmigrated (grep across the workspace; all were enumerated in 2.1).

**Tests:**
- `cache_invalidate_both_key_and_prefix_rejected` (core.rs tests): canonical/builder step with both `Some` → compile → `Err(CamelError::Config)` message contains `exactly one of`. Command: `cargo test -p camel-core --lib cache_invalidate`. Expected: pin (transitional 2.1 arm already rejected both-Some; 2.5 must preserve).
- `cache_invalidate_prefix_compiles_to_target` (core.rs tests): builder with `key_prefix: Some(expr)` → compiler arm → segment constructed (assert via successful `CompileOutcome::Matched`). Command: `cargo test -p camel-core --lib cache_invalidate_prefix_compiles`. Expected: fails before, passes after.
- `cache_coalesce_misses_compiles`: builder Cache with `coalesce_misses: true` → compile → `CompileOutcome::Matched` (service-level behavior covered by 2.4 unit tests). Command: `cargo test -p camel-core --lib cache_coalesce_misses`. Expected: pin (compile-level; route-level threading proven in 2.7).

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0; `cargo build --workspace` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 2.5

### camel-dsl

#### Task 2.6: DSL key_prefix + coalesce_misses

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-dsl/src/parity_tests.rs` (modified)
- `crates/camel-dsl/tests/schema_validation.rs` (modified)
- `schemas/dsl/route-schema.json` (modified — regenerated)
- `schemas/canonical-route-spec.json` (modified — regenerated)

**Steps:**
1. `route_ast.rs` `CacheInvalidateBody` (route_ast.rs:1171): `key: Option<String>`, add `key_prefix: Option<String>` (`#[serde(default)]` on both), keep `deny_unknown_fields`. `CacheConfig` (route_ast.rs:1144): add `#[serde(default)] coalesce_misses: Option<bool>`.
2. `model.rs` `CacheInvalidateStepDef`: `key: Option<LanguageExpressionDef>` + `key_prefix: Option<LanguageExpressionDef>`; `CacheConfigDef` (model.rs:~480): add `coalesce_misses: bool` (default false at AST→model conversion). Update the AST→model conversion sites.
3. `compile.rs`: in the DSL→builder/cache_invalidate path, validate both/neither at DSL compile time (before builder construction) with the same `CamelError::Config("cache_invalidate: exactly one of 'key' or 'key_prefix' is required")`; map `key`/`key_prefix` source strings through the existing language-expression wrapping (`simple` default when the bare-string form is used); pass `coalesce_misses` through DSL→canonical too.
4. Parity tests: YAML `cache_invalidate: { key_prefix: "ns:" }` ↔ canonical `CacheInvalidate { key: None, key_prefix: Some("ns:") }`; YAML `cache: { key: "k", coalesce_misses: true, on_miss: [ { log: "miss" } ] }` ↔ canonical carrying `coalesce_misses: Some(true)`.
5. Schema smokes: valid route with `key_prefix`; valid route with `coalesce_misses: true`; rejected route with both `key` and `key_prefix`.
6. Regenerate schema artifacts (`cargo xtask schema` — rewrites `schemas/dsl/` and `schemas/canonical-route-spec.json` from the changed schema-derived types) and commit the drift.

**Tests:**
- `cache_invalidate_prefix_compiles` (compile.rs tests): DSL step with `key_prefix` → canonical match `CanonicalStepSpec::CacheInvalidate { key: None, key_prefix: Some(p), .. }`. Command: `cargo test -p camel-dsl`. Expected: fails before, passes after.
- `cache_invalidate_both_or_neither_rejected_at_dsl_compile`: two cases (both present; both absent) → `compile_declarative_step` returns `Err` containing `exactly one of`. Command: `cargo test -p camel-dsl cache_invalidate_both_or_neither`. Expected: fails before, passes after.
- `cache_coalesce_misses_compiles`: DSL `coalesce_misses: Some(true)` → canonical `coalesce_misses: Some(true)`; absent → `None`. Command: `cargo test -p camel-dsl cache_coalesce_misses_compiles`. Expected: fails before, passes after.
- `cache_invalidate_prefix_parity` (parity_tests.rs): YAML ↔ canonical round-trip equality. Command: `cargo test -p camel-dsl cache_invalidate_prefix_parity`. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-dsl` exits 0.
- `cargo xtask schema --check` exits 0 (after regeneration).
- `cargo clippy -p camel-dsl --all-features -- -D warnings` exits 0.

- [x] 2.6

### integration + docs

#### Task 2.7: Phase-2 integration tests + ADR/CONTEXT notes

**Files:**
- `crates/camel-test/tests/cache_admin_test.rs` (modified)
- `crates/camel-test/Cargo.toml` (modified — redb dev-dependency, only if absent)
- `CONTEXT-MAP.md` (modified)
- `docs/adr/0056-cache-repository-port.md` (modified)

**Steps:**
1. Integration test: register a redb repository (temp file) with keys `ns:one`, `ns:two`, `other:x`. Check `crates/camel-test/Cargo.toml` for a redb dev-dependency first; if absent, add `redb` as a dev-dependency matching the workspace version and verify `cargo xtask lint-component-deps` stays green (construct `RedbCacheRepository` directly and register it via the context builder's cache-repository registration API). Route step `cache_invalidate: { repository: persistent, key_prefix: "ns:" }` → assert `CamelCacheInvalidatedCount == 2` property and that a subsequent `cache` lookup on `other:x` still hits while `ns:one` re-runs on_miss.
2. Integration test: memory repository + `cache_invalidate: { key_prefix: "ns:" }` route → assert the exchange fails (fail-closed) with the backend-naming error.
3. Integration test: `cache: { key: "k", coalesce_misses: true, on_miss: [ counting gate ] }` with 3 concurrent exchanges → assert the gate counted exactly 1 invocation and all 3 responses carry the fetched body.
4. CONTEXT-MAP.md: extend the Cache EIP entry (line 117) with `key_prefix`/`CamelCacheInvalidatedCount`/`coalesce_misses` and extend `CacheRepository` (line 119) with the `invalidate_prefix` default-async-method note.
5. ADR-0056: under Consequences → Interface stability, add one sentence recording the outcome: the anticipated `len()`/`keys()` extension materialized as a default async method `invalidate_prefix` (chosen over a separate trait — single registry lookup, no downcast); `CacheStats` grew `peek_stale_served`/`invalidations`/`bytes` (source-breaking for external struct literals; migration `..Default::default()`).

**Tests:**
- `cache_invalidate_prefix_purges_namespace_redb`: as step 1. Command: `cargo test -p camel-test --test cache_admin_test`. Expected: fails before, passes after.
- `cache_invalidate_prefix_memory_fails_closed`: as step 2. Command: `cargo test -p camel-test --test cache_admin_test cache_invalidate_prefix_memory`. Expected: fails before, passes after.
- `coalesce_misses_single_fetch_under_concurrency`: as step 3 (use `tokio::time::timeout(5s, …)` to fail fast on a stranded waiter). Command: `cargo test -p camel-test --test cache_admin_test coalesce_misses_single_fetch`. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-test --test cache_admin_test` exits 0.
- `cargo xtask lint-context-citations` exits 0.
- `cargo xtask lint-log-levels` exits 0.

- [x] 2.7
