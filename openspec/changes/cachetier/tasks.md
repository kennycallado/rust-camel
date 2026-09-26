# Tasks: cachetier

## camel-config (knob + validation)

### Task 1.1: PayloadMode gains the Redis tier with fail-closed validation rows

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Add variant `Redis` to `pub enum PayloadMode` (serde `rename_all = "lowercase"` already maps it to `"redis"`). Update the enum's doc comment and the env-allowlist doc comment near `config.rs:2806-2812` (currently says PayloadMode deserializes from `"inline"`/`"disk"`) to include `"redis"` and state the tier rules.
2. Locate the cache_repo payload validation (grep `payload` in the `validate` paths; the disk matrix reads `if payload == Some(Disk) {…} else { reject payload_dir/sweep_interval/max_ttl with "requires payload = \"disk\"" }`). Restructure:
   - Duration fields (`payload_sweep_interval`, `payload_max_ttl`): accepted when `matches!(payload, Some(PayloadMode::Disk) | Some(PayloadMode::Redis))` with the existing parse + ≥1s checks; still rejected for inline/unset with the existing message.
   - `payload_dir`: required non-empty on `Disk` (unchanged); rejected on `Redis` with a NEW message containing `payload_dir is inconsistent with payload = "redis"`; still rejected for inline/unset with the existing `requires payload = "disk"` message.
   - NEW row: `payload = Redis` with `backend` != `"redis"` → `CamelError::Config` with a message containing `payload = "redis" requires backend = "redis"` and naming the actual backend value.
3. Generalize the memory-backend payload rejection wording if it names only `"disk"`; it must reject every offload mode (`Disk` and `Redis`) — inline stays valid.
4. Do NOT touch wiring (`context_ext.rs`) in this task; the redis tier stays inert until task 1.4.

**Tests:** (in the existing `config.rs` cache-repo validation test module, following the house pattern of the disk-matrix tests)
- `cache_payload_redis_accepted_on_redis_backend`: valid `CacheRepoConfig { backend: "redis", url: Some("redis://127.0.0.1:6379"), payload: Some(PayloadMode::Redis) }` → `validate()` returns `Ok(())`. Also accepted with `payload_sweep_interval`/`payload_max_ttl` set (durations tunable on the redis tier).
- `cache_payload_redis_rejected_on_redb_backend`: `backend: "redb"`, `payload: Some(PayloadMode::Redis)` → `Err` whose message contains `requires backend = "redis"` and the string `redb`. Command: `cargo test -p camel-config --lib cache`. Expected: fails before step 2, passes after.
- `cache_payload_redis_rejected_on_memory_backend`: `backend: "memory"`, `payload: Some(PayloadMode::Redis)` → `Err` with the memory payload rejection.
- `cache_payload_redis_with_payload_dir_rejected`: `backend: "redis"`, `payload: Some(PayloadMode::Redis)`, `payload_dir: Some("/tmp/blobs")` → `Err` whose message contains `inconsistent with payload = "redis"` and names `payload_dir`.
- `cache_payload_disk_matrix_unchanged`: existing disk-matrix tests (`disk` without `payload_dir` errors; `disk` on memory errors; durations rejected for inline) still pass unmodified — add no new assertions, just verify green.

**Acceptance:**
- `cargo test -p camel-config --lib` green including the 4 new tests.
- `cargo clippy -p camel-config -- -D warnings` exits 0.
- `cargo fmt --check` clean for the touched file.

- [x] 1.1

## camel-core (pluggable offload store)

### Task 1.2: Extract `PayloadStore` + `OffloadRepository`; disk logic becomes `DiskPayloadStore`

**Files:**
- `crates/camel-core/src/cache/offload.rs` (new)
- `crates/camel-core/src/cache/disk_offload.rs` (modified)
- `crates/camel-core/src/cache/disk_offload_tests.rs` (modified)
- `crates/camel-core/src/cache/disk_offload_reclaim_tests.rs` (modified)
- `crates/camel-core/src/cache/mod.rs` (modified)
- `crates/camel-config/src/context_ext.rs` (modified — call-site only)

**Steps:**
1. Create `offload.rs` with the store trait:
   ```rust
   #[async_trait]
   pub trait PayloadStore: Send + Sync {
       async fn put(&self, name: &str, bytes: &[u8], death_epoch: SystemTime) -> Result<(), CamelError>;
       async fn read(&self, name: &str) -> Result<Option<Vec<u8>>, CamelError>;
       async fn unlink(&self, name: &str) -> Result<(), CamelError>;
   }
   ```
2. Move the decorator from `disk_offload.rs` into `offload.rs` as `OffloadRepository`, replacing the `dir: PathBuf` field with `store: Arc<dyn PayloadStore>`. Everything ADR-0065 fixed moves verbatim: blob naming (`blob_filename`), `content_fingerprint`, death-epoch math (`expires_at + stale_retention + sweep_interval`, fabricated `payload_max_ttl` for entries without expiry), bytes-empty index row with `payload_path`, `hydrate`, eager `reclaim_predecessor` on overwrite, inline fallback on store-put failure (WARN + `inner.set` of the original entry), MISS+WARN on `read() == None`, `Err` on store-read failure, `sanitize_blob_name` direct-child guard, `OffloadClock` seam. Constructor: `OffloadRepository::new(inner, store, stale_retention, sweep_interval, payload_max_ttl)` plus `with_clock(...)` test seam. The decorator no longer spawns/aborts any task and takes no shutdown token.
3. Reshape `disk_offload.rs` to `pub struct DiskPayloadStore { dir: PathBuf }` implementing `PayloadStore`: `write_blob` (tmp+`O_EXCL`+fsync+rename, `TMP_NAME_ATTEMPTS`), `read` (rejects non-direct-child names), `unlink` (ENOENT counts as success), plus `new(dir, sweep_interval, shutdown_token)` that spawns `spawn_sweeper` (real clock, `sweep_payload_dir`, `.tmp` GC, death-epoch unlink) and `impl Drop` aborting the sweeper. Move `sweep_payload_dir`/`SweepStats`/`spawn_sweeper` here unchanged.
4. `mod.rs`: `pub use offload::{OffloadRepository, PayloadStore};` and `pub use disk_offload::DiskPayloadStore;`; remove the `DiskOffloadRepository` re-export.
5. Fix the single external call site: in `camel-config/src/context_ext.rs` `wrap_disk_offload`, construct `Arc::new(DiskPayloadStore::new(dir, sweep_interval, shutdown_token))` and wrap with `OffloadRepository::new(backend, store, stale_retention, sweep_interval, payload_max_ttl)`. Keep the existing disk WARN and the `payload != Some(PayloadMode::Disk)` early return. No behavior change.
6. Update the existing tests to construct through `DiskPayloadStore` + `OffloadRepository` (clock seam moves to the decorator); keep every test's assertions identical. This includes `disk_offload_tests.rs` and `disk_offload_reclaim_tests.rs`, which mount via `#[path]` includes inside `disk_offload.rs` (~line 764) — the `#[path]` mounts must survive the reshape.

**Tests:**
- Existing disk suite is the refactor guard: `cargo test -p camel-core --lib cache::disk_offload` — every pre-existing test passes with unchanged assertions. Expected: red mid-refactor, green at step 6.
- `offload_repository_inline_fallback_on_store_put_failure` (new, in `offload.rs` tests): a test-local `FailingPutStore` implementing `PayloadStore` whose `put` returns `Err(CamelError::Io(...))`, wrapping `MemoryCacheRepository` → `set("k", entry_with_bytes)` returns `Ok(())` and a subsequent `get("k")` returns the entry with bytes present (stored inline). Command: `cargo test -p camel-core --lib offload_repository_inline_fallback_on_store_put_failure`. Expected: fails before step 2, passes after.
- `offload_repository_missing_payload_is_miss`: a `ForgetfulStore` whose `read` returns `Ok(None)` and whose `put` returns `Ok(())` → after `set`, `get("k")` returns `Ok(None)` (MISS+WARN path). Same command pattern.

**Acceptance:**
- `cargo test -p camel-core --lib cache::` green (all pre-existing disk tests + 2 new).
- `cargo test -p camel-config --lib` green (call-site fix compiles; existing tests pass).
- `cargo clippy -p camel-core -p camel-config -- -D warnings` exits 0.
- No `DiskOffloadRepository` symbol remains: `grep -rn "DiskOffloadRepository" crates/` returns nothing.

- [x] 1.2

## camel-redis-repo (redis payload store)

### Task 1.3: `RedisPayloadStore` over the shared executor seam + `connect_with_payload_store`

**Files:**
- `crates/services/camel-redis-repo/src/payload_store.rs` (new)
- `crates/services/camel-redis-repo/src/cache_repo.rs` (modified)
- `crates/services/camel-redis-repo/src/lib.rs` (modified)
- `crates/services/camel-redis-repo/Cargo.toml` (modified — add `camel-core` dependency)
- `crates/camel-api/src/component_metrics.rs` (modified — `#[derive(Clone)]` on `ComponentMetrics` only: `connect_with_payload_store` hands one metrics facade to both the index and the store; additive, no new types)

**Steps:**
1. Add `camel-core` to dependencies in `Cargo.toml` (service→core direction, consistent with the hexagonal boundary; check `lint-component-deps`/`lint-publish-cycles` still pass).
2. Create `payload_store.rs`:
   ```rust
   pub struct RedisPayloadStore {
       executor: Arc<dyn RepoCommandExecutor>,
       metrics: ComponentMetrics,
       key_prefix: String,
       repo_name: String,
   }
   ```
   - Key shape: `keyspace::namespaced(&self.key_prefix, &self.repo_name, &format!("payload:{name}"))` → `{prefix}:{repo}:payload:{blob-name}` (inside the repository namespace so `clear()`'s SCAN+UNLINK reclaims payloads; blob names are keys, not namespace tokens, so the charset guard is not applied to them).
   - `put`: `SET key bytes` with `SetOptions::default().with_expiration(SetExpiry::EXAT(death_epoch_secs))`; seconds via checked `duration_since(UNIX_EPOCH)` — on overflow return `Err(CamelError::Io(...))` from `put` so the ADR-0065 inline fallback (WARN + inline `inner.set`, bounded by the index EXAT) applies; a payload blob is never stored without a deadline. Dispatch through `execute_retry_safe(&self.executor, cmd, &self.metrics, "set")`.
   - `read`: `GET key`; `Value::Nil` → `Ok(None)`, `Value::BulkString(bytes)` → `Ok(Some(bytes))`, transport error → `Err` (mapped to `Io` by the executor, per C1 — never a silent `None`).
   - `unlink`: `UNLINK key` through `execute_retry_safe(..., "unlink")`; nil (already gone) counts as success.
   - `#[async_trait] impl camel_core::cache::PayloadStore for RedisPayloadStore`.
3. In `cache_repo.rs` add:
   ```rust
   pub async fn connect_with_payload_store(
       name: &str, endpoint: &RedisEndpointConfig, key_prefix: &str,
       stale_retention: Duration, metrics: ComponentMetrics,
   ) -> Result<(RedisCacheRepository, RedisPayloadStore), CamelError>
   ```
   built on the same internals as `connect` but constructing ONE executor handed to both the repository and the store (ADR-0063: one multiplexed connection per repository). Add a `pub(crate)` test seam `with_executor_and_payload_store` (or extend the existing `with_executor`) so both objects build from one `FakeRepoExecutor`.
4. `lib.rs`: `mod payload_store; pub use payload_store::RedisPayloadStore;` (match the crate's existing visibility style for `RedisCacheRepository`).

**Tests:** (in `payload_store.rs` `#[cfg(test)]`, following the `FakeRepoExecutor` scripting pattern from `cache_repo.rs` tests — store built via the test seam with `key_prefix = "camel:cache"`, `repo_name = "default"`)
- `payload_put_sets_namespaced_key_with_exat`: scripted `Value::Okay`; `put("abc.blob", b"payload-bytes", epoch)` → `Ok(())`; recorded `redis::Cmd` contains key `camel:cache:default:payload:abc.blob` and the EXAT option carrying `epoch.as_secs()`. Command: `cargo test -p camel-redis-repo`. Expected: fails before step 2, passes after.
- `payload_read_returns_bytes`: scripted `Value::BulkString(b"payload-bytes".to_vec())` → `read("abc.blob")` → `Ok(Some(vec![..same..]))`.
- `payload_read_nil_is_none`: scripted `Value::Nil` → `Ok(None)`.
- `payload_unlink_uses_unlink_and_tolerates_nil`: scripted `Value::Int(1)` and `Value::Nil` → both `Ok(())`; recorded cmd is `UNLINK` with the namespaced payload key.
- `payload_put_transient_retries_once`: first `execute` returns a transient error, `refresh` + retry succeed (mirror `transient()` helper in `cache_repo.rs` tests) → `put` returns `Ok(())`.
- `payload_read_transient_error_is_err_not_none`: scripted transient error on `execute` (and on the retry after `refresh`) → `read` returns `Err` (C1: never a silent miss), matching the precedent of `get_err_on_transient_never_silent_miss`.
- `payload_put_overflow_epoch_is_err`: `put` with `death_epoch` before the Unix epoch (checked `duration_since` fails) → `Err` from `put` (no EXAT-less `SET` is ever issued; assert the recorded cmd list is empty).

**Acceptance:**
- `cargo test -p camel-redis-repo` green: 51 baseline + 7 new = 58+.
- `cargo clippy -p camel-redis-repo -- -D warnings` exits 0.
- `cargo xtask lint-component-deps` and `cargo xtask lint-publish-cycles` exit 0.

- [x] 1.3

## camel-config (wiring)

### Task 1.4: Generalize `wrap_disk_offload` → `wrap_payload_offload` with the redis tier

**Files:**
- `crates/camel-config/src/context_ext.rs` (modified)

**Steps:**
1. Rename `wrap_disk_offload` to `wrap_payload_offload(ccfg, backend, shutdown_token, redis_store: Option<RedisPayloadStore>)`. Behavior by tier: `None`/`Some(Inline)` → bare backend; `Some(Disk)` → disk wrap exactly as today (step 1.2.5 body); `Some(Redis)` → `redis_store.take().ok_or_else(|| CamelError::Config("cache_repo.payload = \"redis\" requires the redis payload store".into()))` then `OffloadRepository::new(backend, Arc::new(store), stale_retention, sweep_interval, payload_max_ttl)`. Reuse `parse_stale_retention`/`payload_durations` for all tiers; no startup WARN on the redis tier.
2. Update the redb call site: pass `None` for the store (validation already rejects `redis` tier on redb).
3. Update the redis call site: when `ccfg.payload == Some(PayloadMode::Redis)`, build via `RedisCacheRepository::connect_with_payload_store(...)` (same endpoint/prefix/name/metrics as `build_redis_cache_repo`), else keep `build_redis_cache_repo`; pass the store (or `None`) into `wrap_payload_offload`. The decorator still registers under the bare backend's name.
4. Update the function's doc comment: same-name registration, per-tier WARN policy (disk warns on the resolved dir; redis shares the index by construction).

**Tests:** (following existing `context_ext.rs` unit-test patterns)
- `wrap_payload_offload_redis_tier_without_store_is_config_error`: `ccfg { backend: "redis", payload: Some(PayloadMode::Redis) }`, `redis_store = None`, any `Arc<dyn CacheRepository>` backend → `Err` whose message contains `requires the redis payload store`. Command: `cargo test -p camel-config --lib wrap_payload_offload_redis_tier_without_store_is_config_error`. Expected: fails before step 1, passes after.
- `wrap_payload_offload_inline_returns_bare_backend`: `payload: None` → returns the same `Arc` (identity by `name()` or pointer equality) unchanged.
- End-to-end redis-tier wiring (boot + roundtrip + index/payload split) is owned by task 1.5's live suite; no duplicate here.

**Acceptance:**
- `cargo test -p camel-config --lib` green including the 2 new tests.
- `cargo clippy -p camel-config -- -D warnings` exits 0.

- [x] 1.4

## camel-test (live tiering suite)

### Task 1.5: TestContainers redis tier — roundtrip, index/payload split, reclaim, disk regression

**Files:**
- `crates/camel-test/tests/cache_payload_offload.rs` (modified)

**Steps:**
1. Read the existing suite's container/repo plumbing (how it boots a redis container and constructs repositories) and reuse it; follow the file's established naming and locking conventions (ADR-0054: no `#[ignore]`).
2. Add live tests for the redis tier in TWO layers:
   - decorator-level (build index + store via `connect_with_payload_store`, wrap with `OffloadRepository`, same `key_prefix`/name the suite already uses): roundtrip + split + TTL, overwrite reclaim
   - boot-path (production wiring): a `redis_offload_toml` variant of the existing `disk_offload_toml` helper (~cache_payload_offload.rs:83-103) writing `backend = "redis"`, `url`, `payload = "redis"`, booted through the same `boot_context` plumbing — this covers the context_ext redis arm → `connect_with_payload_store` → `wrap_payload_offload` path end-to-end.
   Disk-tier regression is the existing suite staying green (add no disk test).
3. Where a raw redis client is needed to inspect keys (index row JSON, payload key bytes, `TTL`), use the same client seam the suite already establishes for its container (`raw_connection` or equivalent).

**Tests:** (feature `integration-tests`, testcontainers; run: `cargo test -p camel-test --test cache_payload_offload --features integration-tests`)
- `redis_payload_tier_roundtrip_index_split_and_ttl`: container up; `set("k", entry with bytes "hello-tier")` via the decorated repository → `get("k")` returns the entry with `bytes == "hello-tier"`; raw GET of the index key `{prefix}:{name}:k` parses as `CacheEntry` JSON with empty `bytes` and a `payload_path`; raw EXISTS/GET on `{prefix}:{name}:payload:{payload_path}` returns exactly `"hello-tier"`; raw `TTL` on that payload key is > 0 and finite (EXAT bound). Expected: fails before tasks 1.2–1.4, passes after.
- `redis_payload_tier_overwrite_reclaims_predecessor`: `set("k", v1)` then `set("k", v2)` → predecessor payload key is gone (raw EXISTS = 0), the successor payload key holds `v2`, and `get("k")` returns `v2`.
- `redis_payload_tier_boots_from_toml`: `Camel.toml` with `backend = "redis"`, container URL, `payload = "redis"` → context boots; the registered repository roundtrips an entry (set then get with bytes) and the raw index row stays bytes-empty with `payload_path` (boot-path coverage for task 1.4's wiring).
- Existing disk-offload tests in the same file stay green unmodified (disk default unchanged).

**Acceptance:**
- `cargo test -p camel-test --test cache_payload_offload --features integration-tests` green (existing + 3 new).
- `cargo test -p camel-redis-repo` still green (no unit fallout).

- [x] 1.5

## docs (ADR + CONTEXT)

### Task 1.6: ADR-0065 amendment + CONTEXT/config doc alignment

**Files:**
- `docs/adr/0065-cache-payload-offload.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-core/CONTEXT.md` (modified)
- `crates/services/camel-redis-repo/CONTEXT.md` (modified)
- `crates/camel-config/CONTEXT.md` (modified)
- `docs/src/configuration/index.md` (modified — only if it documents `cache_repo.payload`; grep first and skip if absent)

**Steps:**
1. ADR-0065: add `## Amendment (bd rc-6b88t): payload location tiering` under Consequences, mirroring the rc-uteoa amendment's shape: NFS motivation (demo-team report), the pluggable `PayloadStore` decision, the redis tier contract (`{prefix}:{repo}:payload:` keyspace inside the repository namespace, `SET ... EXAT death-epoch`, UNLINK reclaim on overwrite + eager `clear()` scavenging + native EXAT bound replacing the sweeper, MISS+WARN on vanished payload, inline fallback on payload-write failure, one shared connection per ADR-0063), and the fail-closed matrix additions. Refresh the load-bearing citation table rows that moved (`DiskOffloadRepository` → `OffloadRepository`/`DiskPayloadStore`/`payload_store.rs`) and the CONTEXT-MAP.md ADR index line for 0065.
2. CONTEXT.md updates: camel-core cache section (offload decorator + store trait + disk store), camel-redis-repo (RedisPayloadStore, connect_with_payload_store, payload keyspace, TTL discipline), camel-config (tier knob + matrix rows + wiring). English, house CONTEXT.md citation style (`path:line`), only sections that changed.
3. Verify with `grep -rn "DiskOffloadRepository" docs/ CONTEXT-MAP.md crates/*/CONTEXT.md` → no stale references to the removed symbol (historical ADR decision text may keep the old name where it describes the 2026-08-24 original decision; the amendment + citation table must use the new names).

**Tests:**
- `cargo xtask lint-context-citations` exits 0 (all CONTEXT.md citations resolve).
- `grep -c "Amendment (bd rc-6b88t)" docs/adr/0065-cache-payload-offload.md` = 1.

**Acceptance:**
- `cargo xtask lint-context-citations` 0 violations.
- ADR amendment present; CONTEXT docs cite the new symbols with correct paths.

- [x] 1.6
