# Tasks: add-cache-repository

<!--
  Multi-phase change. The WHOLE tasks.md (all Phase blocks) is written and
  plan-blessed ONCE. PHASE 3 iterates phase-groups in order, with an
  inter-phase r_glm review after Phase 1 and Phase 2 (each has >=2 tasks).
  Phase 3 (>=2 tasks) also gets an inter-phase review.
-->

## Phase 1: Foundation — port + types + memory backend + wiring + ADR

### camel-api

#### Task 1.1: CacheRepository trait + CacheEntry/ContentType/CacheStats types

**Files:**
- `crates/camel-api/src/cache.rs` (new)
- `crates/camel-api/src/lib.rs` (modified — add `pub mod cache;` and `pub use cache::{CacheRepository, CacheEntry, ContentType, CacheStats};`)
- `crates/camel-core/tests/content_type_match_test.rs` (new — downstream exhaustive-match test proving `ContentType` is NOT `#[non_exhaustive]`)

**Steps:**
1. Add `serde = { workspace = true }` to `crates/camel-api/Cargo.toml` `[dependencies]` if not already present (verify — it already is, per `serde.workspace = true` in the existing Cargo.toml). `CacheEntry` uses unconditional `#[derive(serde::Serialize, serde::Deserialize)]` — no feature gate needed (serde is a direct, unconditional dependency of camel-api).
2. Create `crates/camel-api/src/cache.rs` with:
   - `pub struct CacheEntry { pub bytes: Vec<u8>, pub content_type: ContentType, pub expires_at: Option<SystemTime> }` deriving `Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize`. Uses `Vec<u8>` (NOT `bytes::Bytes`) because the workspace `bytes = "1"` does not enable the `serde` feature, and `Vec<u8>` is unconditionally `Serialize`/`Deserialize`. Backends convert `Vec<u8>` ↔ `Bytes` at the boundary (`Bytes::from(vec)` / `vec.to_vec()`). NOT `#[non_exhaustive]` (struct-literal construction by backends — ADR-0049 §Rule 3 exception).
   - `pub enum ContentType { Bytes, Text, Json, Xml }` deriving `Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize`. NOT `#[non_exhaustive]` — backends and `CacheService` must match exhaustively on all 4 variants for content-type↔Body mapping (unlike `Body` which IS `#[non_exhaustive]` and gets wildcard arms).
   - `pub struct CacheStats { pub hits: u64, pub misses: u64, pub evictions: u64, pub entries: u64 }` deriving `Debug, Clone, Default, PartialEq`. NOT `#[non_exhaustive]` — backends construct it with struct literals in `stats()` impls (e.g. `CacheStats { hits: self.hits.load(Relaxed), .. }`).
   - `#[async_trait::async_trait] pub trait CacheRepository: Send + Sync + std::fmt::Debug + 'static` with methods: `fn name(&self) -> &str;`, `async fn get(&self, key: &str) -> Result<Option<CacheEntry>, CamelError>;`, `async fn set(&self, key: &str, value: CacheEntry, ttl: Option<Duration>) -> Result<(), CamelError>;` (contract: `set` computes `expires_at` from `ttl` — `Some(d)` ⇒ `entry.expires_at = Some(SystemTime::now() + d)` and the modified entry is stored; `None` ⇒ entry stored as-is with whatever `expires_at` the caller supplied. The caller constructs `CacheEntry { ..., expires_at: None }` by default; `set` is the authority that applies `ttl`. This matches spec R1 "set SHALL compute expires_at from the supplied ttl" and is uniform across backends.), `async fn peek_stale(&self, key: &str) -> Result<Option<CacheEntry>, CamelError>;`, `async fn invalidate(&self, key: &str) -> Result<(), CamelError>;`, `async fn clear(&self) -> Result<(), CamelError>;`, `fn stats(&self) -> CacheStats { CacheStats::default() }` (defaulted).
   - Module doc-comment citing ADR-0023 Contract C1 (backend failure surfaces as Err, never silent miss).
3. Wire in `lib.rs`: `pub mod cache;` and re-export the four public symbols.
4. Run `cargo fmt -p camel-api && cargo clippy -p camel-api -- -D warnings`.

**Tests:**
- `cache_entry_construction`: in `cache.rs` `#[cfg(test)] mod tests` — construct `CacheEntry { bytes: vec![b'x'], content_type: ContentType::Bytes, expires_at: None }`, assert `.bytes.len() == 1` and `.content_type == ContentType::Bytes`. → `cargo test -p camel-api --lib cache::tests::cache_entry_construction` → passes after step 2.
- `cache_stats_default`: assert `CacheStats::default() == CacheStats { hits: 0, misses: 0, evictions: 0, entries: 0 }`. → `cargo test -p camel-api --lib cache::tests::cache_stats_default` → passes. (NOTE: `CacheStats` is NOT `#[non_exhaustive]` — backends construct it with struct literals. See Task 1.1 step 2.)
- DOWNSTREAM test in `crates/camel-core/tests/content_type_match_test.rs` (new file): `content_type_can_be_matched_exhaustively_downstream` — a function matching `camel_api::ContentType` exhaustively with NO wildcard arm. This test is in `camel-core` (downstream from `camel-api`), where `#[non_exhaustive]` would actually enforce a wildcard arm if the attribute were present. If the attribute is accidentally added, this test fails to compile. → `cargo test -p camel-core --test content_type_match_test`

**Acceptance:**
- `cargo build -p camel-api` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo test -p camel-api --lib cache::` all pass.
- `cargo test -p camel-core --test content_type_match_test` passes.
- `cargo xtask lint-non-exhaustive` exits 0 (verifies NONE of the three types are `#[non_exhaustive]` — all need struct-literal or exhaustive-match construction by downstream backends).

- [x] 1.1

### camel-core

#### Task 1.2: MemoryCacheRepository (moka) — size-eviction only, in-band expiry

**Files:**
- `Cargo.toml` (workspace root, modified — add `moka = { version = "0.12", features = ["future"] }` under `[workspace.dependencies]`)
- `crates/camel-core/Cargo.toml` (modified — add `moka = { workspace = true }` under `[dependencies]`)
- `crates/camel-core/src/cache/mod.rs` (new)
- `crates/camel-core/src/cache/memory.rs` (new)
- `crates/camel-core/src/lib.rs` (modified — add `pub mod cache;`)

**Steps:**
1. Pin `moka = { version = "0.12", features = ["future"] }` in workspace `[workspace.dependencies]`. Add `moka = { workspace = true }` to `crates/camel-core/Cargo.toml`. (Pre-resolved: moka 0.12 `future` feature exposes `moka::future::Cache`. Wire the `eviction_listener` via moka's `CacheBuilder::eviction_listener` closure API — the method name is `eviction_listener` (NOT `evicted_listener`), verified in moka 0.12 docs. The closure signature is `Fn(Arc<K>, V, RemovalCause) + Send + Sync + 'static`. The `'static` bound means it CANNOT borrow a struct field — it must capture `Arc::clone` of an `Arc<AtomicU64>`. This makes `stats().evictions` non-zero and accurate for the memory backend.)
2. Create `crates/camel-core/src/cache/mod.rs` with `pub mod memory;` and re-exports. (Do NOT forward-declare `pub mod redb;` here — that file is created in Task 3.1; forward-declaring would break `cargo build -p camel-core` between Task 1.2 and Task 3.1.)
3. Create `crates/camel-core/src/cache/memory.rs`:
   - `pub struct MemoryCacheRepository { name: String, inner: moka::future::Cache<String, CacheEntry>, hits: Arc<AtomicU64>, misses: Arc<AtomicU64>, evictions: Arc<AtomicU64> }`. ALL counters are `Arc<AtomicU64>` (NOT bare `AtomicU64`) because the moka `eviction_listener` closure must capture a clone of the evictions counter, and the `'static` bound on the listener forbids borrowing a struct field. This pattern is already used in the codebase (`route_registry.rs:123` returns `Option<Arc<AtomicU64>>` for cross-task counters).
   - `impl MemoryCacheRepository { pub fn new(name: impl Into<String>, max_capacity: usize) -> Self }` — in `new`, construct `let hits = Arc::new(AtomicU64::new(0)); let misses = Arc::new(AtomicU64::new(0)); let evictions = Arc::new(AtomicU64::new(0));` first. Clone `evictions` into the listener closure, then build the cache: `moka::future::CacheBuilder::new(max_capacity as u64).eviction_listener({ let e = Arc::clone(&evictions); move |_k, _v, _cause| { e.fetch_add(1, Ordering::Relaxed); } }).build()`. Store all three `Arc<AtomicU64>` as struct fields. Do NOT call `.expire_after(...)` or `.time_to_live(...)`.
   - `#[async_trait] impl CacheRepository for MemoryCacheRepository`:
     - `name` returns `&self.name`.
      - `get(key)`: `let r = self.inner.get(key).await;` if `None` → incr `misses`, return `Ok(None)`. If `Some(entry)` → check `entry.expires_at`: if `Some(exp) && SystemTime::now() > exp` → incr `misses`, return `Ok(None)` (in-band expiry). Else incr `hits`, return `Ok(Some(entry))`.
      - `set(key, value, ttl)`: if `ttl` is `Some(d)` → `value.expires_at = Some(SystemTime::now() + d)`. If `ttl` is `None` → leave `value.expires_at` as-is. Then `self.inner.insert(key.to_string(), value).await;` return `Ok(())`.
      - `peek_stale(key)`: `let r = self.inner.get(key).await;` return `Ok(r)` (no expiry check, no counter increment).
      - `invalidate(key)`: `self.inner.invalidate(key).await;` return `Ok(())`.
      - `clear`: `self.inner.invalidate_all().await;` return `Ok(())`.
      - `stats`: return `CacheStats { hits: self.hits.load(Relaxed), misses: self.misses.load(Relaxed), evictions: self.evictions.load(Relaxed), entries: self.inner.entry_count() as u64 }`. (The `entries` count comes from moka's `entry_count()` — no separate counter needed for the memory backend.)
4. Run `cargo fmt && cargo clippy -p camel-core -- -D warnings`.

**Tests:** (in `memory.rs` `#[cfg(test)] mod tests`)
- `get_returns_none_on_miss_some_on_hit`: new repo `MemoryCacheRepository::new("m", 100)`, `set("k", entry, Some(1h)).await`, assert `get("k").await == Ok(Some(entry))`, assert `get("absent").await == Ok(None)`. → `cargo test -p camel-core --lib cache::memory::tests::get_returns_none_on_miss_some_on_hit`
- `get_returns_none_after_expiry_peek_stale_returns_entry`: `set("k", entry, Some(1ms)).await`, sleep 10ms, assert `get("k").await == Ok(None)` and `peek_stale("k").await == Ok(Some(entry))`. → `cargo test -p camel-core --lib cache::memory::tests::get_returns_none_after_expiry_peek_stale_returns_entry`
- `set_with_none_ttl_stores_without_expiry`: `set("k", entry, None).await`, sleep 10ms, assert `get("k").await == Ok(Some(entry))`. → `cargo test -p camel-core --lib cache::memory::tests::set_with_none_ttl_stores_without_expiry`
- `invalidate_is_noop_on_absent_key`: `invalidate("absent").await == Ok(())`. → `cargo test -p camel-core --lib cache::memory::tests::invalidate_is_noop_on_absent_key`
- `max_capacity_bounds_entry_count`: `let repo = MemoryCacheRepository::new("m", 2);`, `repo.set("a", ea, None)`, `repo.set("b", eb, None)`, `repo.set("c", ec, None)`, call `repo.inner.run_pending_tasks().await` (moka maintenance is async — `run_pending_tasks` drains pending evictions deterministically), assert `repo.inner.entry_count() <= 2`. → `cargo test -p camel-core --lib cache::memory::tests::max_capacity_bounds_entry_count`
- `get_surfaces_no_failure_as_miss_silently` (doc-only test — moka memory cannot fail; assert via a `#[doc]` comment that the impl has no `unwrap`/`expect` that could mask an error). Alternatively: skip this test for memory and rely on Task 3.1 redb test for Contract C1. → covered by Task 3.1.
- `stats_reflects_hits_misses_evictions_entries`: after 1 hit + 1 miss, assert `stats().hits == 1 && stats().misses == 1`. Assert `stats().entries >= 1` (at least the hit entry is resident). Evictions are exercised by a separate test (below). → `cargo test -p camel-core --lib cache::memory::tests::stats_reflects_hits_misses_evictions_entries`
- `clear_empties_repository`: `set("a", ea, None)`, `set("b", eb, None)`, `clear().await`, assert `get("a").await == Ok(None)` and `get("b").await == Ok(None)`. → `cargo test -p camel-core --lib cache::memory::tests::clear_empties_repository`
- `evictions_incremented_on_size_pressure`: `let repo = MemoryCacheRepository::new("m", 1);`, `repo.set("a", ea, None)`, `repo.set("b", eb, None)` (evicts "a"), call `repo.inner.run_pending_tasks().await` (drain moka's async eviction tasks), assert `repo.stats().evictions >= 1`. → `cargo test -p camel-core --lib cache::memory::tests::evictions_incremented_on_size_pressure`

**Acceptance:**
- `cargo build -p camel-core` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- All tests above pass.
- `cargo xtask lint-unwrap` exits 0 (no new `unwrap`/`expect`).
- moka is configured WITHOUT `expire_after` and WITHOUT `time_to_live` (grep `crates/camel-core/src/cache/memory.rs` for `expire_after` / `time_to_live` → 0 hits).

- [x] 1.2

#### Task 1.3: CamelContext wiring — register_cache_repository / cache_repository

**Files:**
- `crates/camel-core/src/registry.rs` (modified — add `register_or_replace` method to `NamedRegistry<T>` + `CacheRegistry`/`SharedCacheRegistry` type aliases mirroring lines 87-105)
- `crates/camel-core/src/context.rs` (modified — add `register_cache_repository`, `replace_cache_repository`, `cache_repository`, `shutdown_token` methods, mirror lines 826-857 for idempotent/claim_check)
- `crates/camel-core/src/context_builder.rs` (modified — register default `"memory"` MemoryCacheRepository with `max_capacity = 10_000` in `build()`, mirror lines 212-229)

**Steps:**
1. In `crates/camel-core/src/registry.rs`, add two things:
   - A method `pub(crate) fn register_or_replace(&self, name: &str, value: Arc<T>) -> Option<Arc<T>>` on `NamedRegistry<T>` (returns the evicted previous value if any). This is a small additive change to the generic utility, justified by the cache memory-capacity-config use case (Task 3.2 needs to override the default `"memory"` registration when `[default.cache_repo] backend = "memory"` supplies a custom `max_capacity`). The existing `register` method (check-then-insert, returns `Err(AlreadyRegistered)`) is unchanged.
   - Type aliases mirroring the Idempotent/ClaimCheck pattern (lines 87-105): `pub(crate) type CacheRegistry = NamedRegistry<dyn camel_api::CacheRepository>;` and `pub(crate) type SharedCacheRegistry = Arc<CacheRegistry>;`. Add a doc-comment mirroring the ClaimCheck one (line 103-104).
2. In `crates/camel-core/src/context.rs`, find the `register_idempotent_repository` / `idempotent_repository` block (lines ~826-845) and the `register_claim_check_repository` / `claim_check_repository` block (lines ~848-857). Add immediately after the claim_check block:
   - A field `cache_repositories: SharedCacheRegistry` on `CamelContext` (the existing `CamelContext` struct in `context.rs` already has `idempotent_repositories: SharedIdempotentRegistry` and `claim_check_repositories: SharedClaimCheckRegistry` fields — add `cache_repositories: SharedCacheRegistry` next to them; NO outer `Mutex`; the inner `NamedRegistry` has its own locking).
   - `pub fn register_cache_repository(&mut self, name: &str, repo: Arc<dyn CacheRepository>) -> Result<(), RegistryError>` mirroring `register_claim_check_repository` (line 848). Existing `register_*_repository` methods take `&mut self` (defensive API choice; `NamedRegistry` still carries its own lock) — mirror that exactly.
   - `pub fn replace_cache_repository(&mut self, name: &str, repo: Arc<dyn CacheRepository>) -> Option<Arc<dyn CacheRepository>>` — PUBLIC method that internally calls `self.cache_repositories.register_or_replace(name, repo)`. This is the public API that `camel-config/src/context_ext.rs` (a different crate, cannot access `pub(crate)` items) calls when `[default.cache_repo] backend = "memory"` supplies a custom `max_capacity`. Without this public method, camel-config cannot override the default registration.
    - `pub fn cache_repository(&self, name: &str) -> Option<Arc<dyn CacheRepository>>` mirroring `claim_check_repository` (line 857).
    - `pub fn shutdown_token(&self) -> CancellationToken { self.cancel_token.clone() }` — PUBLIC accessor returning a clone of the context's `CancellationToken`. Required by `RedbCacheRepository::new` (Task 3.1) to bind the sweep task to the context's shutdown signal. The field stays private; only the accessor is public. This satisfies spec R5 "the sweep task SHALL bind to the context's CancellationToken": `CamelContext::stop()` → `stop_context()` → `cancel_token.cancel()` → sweep task's `select!` on the token fires → sweep exits.
3. In `crates/camel-core/src/context_builder.rs`, find the block that registers default `"memory"` idempotent + claim_check (lines ~212-229). Add after, mirroring verbatim:
   ```rust
   let cache_repositories = {
       let reg = CacheRegistry::new();
       let memory = Arc::new(MemoryCacheRepository::new("memory", 10_000));
       reg.register("memory", memory)
           .expect("built-in memory cache repository registration must succeed"); // allow-unwrap
       reg
   };
   ```
   Then store `cache_repositories: Arc::new(cache_repositories)` into the `CamelContext` parts (mirror how `idempotent_repositories` is stored). The `// allow-unwrap` annotation is required by `lint-unwrap` (this is the established pattern — the built-in default CANNOT fail because the registry is freshly constructed).
4. Run `cargo fmt && cargo clippy -p camel-core -- -D warnings`.

**Tests:** (in `context.rs` or a new `crates/camel-core/src/cache/wiring_tests.rs`)
- `memory_cache_registered_as_default`: build a default `CamelContext`, call `cache_repository("memory")`, assert `Some(...)` whose `.name() == "memory"`. → `cargo test -p camel-core --lib cache::wiring_tests::memory_cache_registered_as_default`
- `custom_backend_registered_alongside_memory`: build ctx, register a mock `CacheRepository` named `"custom"`, assert `cache_repository("custom")` returns `Some` and `cache_repository("memory")` still returns `Some`. → `cargo test -p camel-core --lib cache::wiring_tests::custom_backend_registered_alongside_memory`
- `duplicate_registration_rejected`: register `"memory"` a second time, assert the result is `Err(RegistryError::AlreadyRegistered)`. → `cargo test -p camel-core --lib cache::wiring_tests::duplicate_registration_rejected`

**Acceptance:**
- `cargo build -p camel-core` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- All 3 wiring tests pass.
- The wiring code is a verbatim structural mirror of the claim_check wiring (same Mutex/Registry shape, same error type).

- [x] 1.3

### docs

#### Task 1.4: ADR-0056 + CONTEXT-MAP Key Terms

**Files:**
- `docs/adr/0056-cache-repository-port.md` (new)
- `CONTEXT-MAP.md` (modified — add Key Terms entries)

**Steps:**
1. Create `docs/adr/0056-cache-repository-port.md` with the standard ADR header (Date: 2026-08-11, Status: Accepted, References: ADR-0001, 0023, 0028, 0033, 0046, 0049, 0055). Record the 6 decisions from design.md §"ADR-0056 will record these 6 decisions" verbatim:
   - Decision 1: memory-default despite the anchor (consistency with Idempotent/ClaimCheck; EFFIS `[default.cache_repo] backend = "redb"` worked example).
   - Decision 2: in-band `expires_at` everywhere (no native TTL; correctness prerequisite for peek_stale; `SystemTime`-not-`Instant`; clock-skew caveat).
   - Decision 3: moka size-eviction only (no `Expiry`, no `time_to_live`; this is what makes `peek_stale` work on the memory tier).
   - Decision 4: mandatory `max_capacity` on memory default (ADR-0033 + D-A5; default 10_000).
   - Decision 5: retention window ≠ TTL (persistent tiers reclaim at `expires_at + retention`, not at `expires_at`; gives `peek_stale` post-expiry reach on redb/redis).
   - Decision 6: no `sweep()` on the trait; reclamation is per-backend (contract honesty; idempotent/claimcheck precedent).
   - Include a "Rejected alternatives" section listing: `cache://` component (ADR-0001 + ADR-0046), `Body` as stored type (not Serialize, Stream un-cacheable), `on_no_poll` reuse (passthrough, no write-back), native backend TTL eviction (breaks peek_stale), extending ClaimCheckRepository with TTL (breaks payload-ownership contract).
2. In `CONTEXT-MAP.md` Key Terms section, add entries (English per language policy):
   - `CacheRepository` — "Pluggable TTL cache port in camel-api. Stores `CacheEntry { bytes, content_type, expires_at }`. Distinct from `IdempotentRepository` (key-only, no time) and `ClaimCheckRepository` (payload-owning, no expiry). See ADR-0056."
   - `CacheEntry` — "The value stored in a `CacheRepository`: materialized bytes + content-type discriminant + in-band expiry timestamp. NOT `Body` (which is not Serialize and includes the un-cacheable `Stream` variant). See ADR-0056."
   - `Cache EIP` — "The `cache`/`cache_invalidate`/`cache_peek_stale` DSL steps, compiled to runtime `CacheService`/`CacheInvalidateService`/`CachePeekStaleService` (`OutcomeSegment` impls, mirroring how `idempotent_consumer` maps to `IdempotentConsumerSegment`). Disambiguation: this is UNRELATED to `stream_cache` (which is OOM protection for streaming bodies, 128 KB materialization threshold — see `StreamCacheService`)."
3. Run `cargo xtask lint-context-citations` to verify citations are well-formed.

**Tests:**
- (Non-Rust; verify by file existence and lint pass.) `ADR-0056 exists and has 6 decisions`: `test -f docs/adr/0056-cache-repository-port.md && grep -c "^### Decision" docs/adr/0056-cache-repository-port.md` ≥ 6 (or equivalent structural check). → manual / `cargo xtask lint-context-citations` exits 0.
- `CONTEXT-MAP has the three new terms`: `grep -c "CacheRepository\|CacheEntry\|Cache EIP" CONTEXT-MAP.md` ≥ 3.

**Acceptance:**
- `docs/adr/0056-cache-repository-port.md` exists and records 6 decisions.
- `CONTEXT-MAP.md` has the 3 new Key Terms entries (English).
- `cargo xtask lint-context-citations` exits 0.
- All prose is English (language policy).

- [x] 1.4

## Phase 2: EIP face — DSL steps + step compiler + CacheService runtime

### camel-dsl

#### Task 2.1: Route AST + model types + canonical spec + declarative→canonical compile arms for cache steps

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified — add `CacheStep`, `CacheInvalidateStep`, `CachePeekStaleStep` structs + `CacheBody`/`CacheConfig`/`CacheInvalidateBody`/`CachePeekStaleBody` + 3 variants to `RouteDslStep`)
- `crates/camel-dsl/src/model.rs` (modified — add `CacheStepDef`, `CacheInvalidateStepDef`, `CachePeekStaleStepDef` structs + 3 variants to `DeclarativeStep` enum at line 532)
- `crates/camel-dsl/src/contract.rs` (modified — add `Cache`, `CacheInvalidate`, `CachePeekStale` to `DeclarativeStepKind` at line 2)
- `crates/camel-api/src/runtime.rs` (modified — add 3 variants to `CanonicalStepSpec` at line 114)
- `crates/camel-dsl/src/compile.rs` (modified — add compile arms for RouteDslStep→DeclarativeStep AND declarative→canonical lowering)

**Steps:**
1. In `route_ast.rs`, define step body structs mirroring `IdempotentConsumerStep`/`EnrichStep` style:
   - `pub struct CacheStep { pub cache: CacheBody }` where `CacheBody` is `Uri(String) | Full(CacheConfig)` (mirror `EnrichBody`).
   - `pub struct CacheConfig { pub repository: Option<String>, pub key: String, pub ttl: Option<String> (humantime duration string parsed by the step compiler via `humantime::parse_duration`), pub max_entry_bytes: Option<usize>, pub on_miss: Vec<RouteDslStep> }`.
   - `pub struct CacheInvalidateStep { pub cache_invalidate: CacheInvalidateBody }` with `CacheInvalidateBody { repository: Option<String>, key: String }`.
   - `pub struct CachePeekStaleStep { pub cache_peek_stale: CachePeekStaleBody }` with `CachePeekStaleBody { repository: Option<String>, key: String }`.
   - Add `Cache(CacheStep)`, `CacheInvalidate(CacheInvalidateStep)`, `CachePeekStale(CachePeekStaleStep)` to `RouteDslStep`.
2. In `model.rs`, add the declarative step-definition structs mirroring `IdempotentConsumerStepDef` (line 447) and `EnrichStepDef` (line 510):
   - `pub struct CacheStepDef { pub repository: Option<String>, pub key: String, pub ttl: Option<String>, pub max_entry_bytes: Option<usize>, pub on_miss: Vec<DeclarativeStep> }`.
   - `pub struct CacheInvalidateStepDef { pub repository: Option<String>, pub key: String }`.
   - `pub struct CachePeekStaleStepDef { pub repository: Option<String>, pub key: String }`.
   - Add variants `Cache(CacheStepDef)`, `CacheInvalidate(CacheInvalidateStepDef)`, `CachePeekStale(CachePeekStaleStepDef)` to `DeclarativeStep` (line 532, after `IdempotentConsumer`).
3. In `contract.rs`, add `Cache`, `CacheInvalidate`, `CachePeekStale` to `DeclarativeStepKind` (line 2). Mirror how `IdempotentConsumer` is represented there. Also check for `MANDATORY_DECLARATIVE_STEP_KINDS` const array (around line 46) — if `ClaimCheck`/`IdempotentConsumer` are listed there, add the 3 cache kinds too (mirror their inclusion); update the length annotation accordingly.
4. In `camel-api/src/runtime.rs`, add 3 variants to `CanonicalStepSpec` (line 114) mirroring the existing variants (each variant carries the fields needed for canonical serialization — likely `Cache { repository: Option<String>, key: String, ttl: Option<String>, max_entry_bytes: Option<usize>, on_miss: Vec<CanonicalStepSpec> }`, etc.). This is required for `cargo xtask schema --check`.
5. In `compile.rs`, add two sets of compile arms:
   - `RouteDslStep::Cache(...)` → `DeclarativeStep::Cache(CacheStepDef { ... })` (mirror how `RouteDslStep::Enrich` → `DeclarativeStep::Enrich` at the existing enrich arm). Default `repository` to `None` here (the step compiler resolves `"memory"` default later).
   - `DeclarativeStep::Cache(...)` → `CanonicalStepSpec::Cache { ... }` (mirror the declarative→canonical lowering for enrich/idempotent_consumer).
   - Repeat for `CacheInvalidate` and `CachePeekStale`.
6. Run `cargo fmt && cargo clippy -p camel-api -p camel-dsl -- -D warnings`.

**Tests:**
- `cache_step_compiles_route_to_declarative`: build a `RouteDslStep::Cache` with `repository=Some("memory")`, `key="k"`, `ttl=Some("1h")`, `on_miss=[Log(...)]`, compile to `DeclarativeStep`, assert variant is `DeclarativeStep::Cache(CacheStepDef { repository: Some("memory"), key: "k", .. })`. → `cargo test -p camel-dsl --lib cache_step_compiles_route_to_declarative`
- `cache_step_compiles_declarative_to_canonical`: build a `DeclarativeStep::Cache(...)`, lower to `CanonicalStepSpec`, assert variant and fields round-trip. → `cargo test -p camel-dsl --lib cache_step_compiles_declarative_to_canonical`
- `cache_invalidate_and_peek_stale_compile`: same pair of compile tests for the two leaf step kinds. → `cargo test -p camel-dsl --lib cache_invalidate_and_peek_stale_compile`
- `cache_step_repository_defaults_to_none_at_dsl_layer`: build `RouteDslStep::Cache` with `repository=None`, compile, assert the declarative form preserves `repository: None` (the `"memory"` default is resolved at the step-compiler layer in Task 2.5, NOT here). → `cargo test -p camel-dsl --lib cache_step_repository_defaults_to_none_at_dsl_layer`

**Acceptance:**
- `cargo build -p camel-api -p camel-dsl` exits 0.
- `cargo clippy -p camel-api -p camel-dsl -- -D warnings` exits 0.
- `cargo xtask schema --check` exits 0 (the 3 new `CanonicalStepSpec` variants are reflected in the generated schema).
- All 4 tests above pass.

- [x] 2.1

#### Task 2.2: YAML parser for cache / cache_invalidate / cache_peek_stale steps

**Files:**
- `crates/camel-dsl/src/yaml.rs` (modified — add parsing arms for the 3 new step kinds)

**Steps:**
1. In `yaml.rs`, find the step-kind dispatcher (the `match` on step name that routes to `enrich`, `pollEnrich`, `idempotentConsumer`, etc.). Add three arms:
   - `"cache"` → parse a `CacheBody` (map form only in v1 — REJECT the string shorthand form; the cache step requires at minimum `key` and `on_miss`). Parse the `CacheConfig` map: `repository` (optional), `key` (required string), `ttl` (optional, parse as humantime duration string), `max_entry_bytes` (optional integer), `on_miss` (required, list of steps parsed recursively).
   - `"cache_invalidate"` → parse `CacheInvalidateBody` map (`repository` optional, `key` required).
   - `"cache_peek_stale"` → parse `CachePeekStaleBody` map (`repository` optional, `key` required).
2. Add validation: `key` is non-empty (return `CamelError::Config` on empty). Do NOT add `on_miss`-non-empty validation (beyond spec — leave that to user discretion).
3. Add parity tests in `parity_tests.rs` if that file asserts YAML↔canonical round-trip for existing step kinds (mirror how `idempotent_consumer` parity is asserted).
4. Run `cargo fmt && cargo clippy -p camel-dsl -- -D warnings`.

**Tests:**
- `yaml_parses_cache_step_full`: parse YAML `- cache: { repository: persistent, key: "${header.k}", ttl: 24h, max_entry_bytes: 1048576, on_miss: [{ log: "miss" }] }`, assert all fields populated. → `cargo test -p camel-dsl --lib yaml_parses_cache_step_full`
- `yaml_parses_cache_step_minimal`: parse `- cache: { key: "k", on_miss: [{ log: "x" }] }`, assert `repository == None`, `ttl == None`, `max_entry_bytes == None`. → `cargo test -p camel-dsl --lib yaml_parses_cache_step_minimal`
- `yaml_rejects_empty_key`: parse `- cache: { key: "", on_miss: [...] }`, assert `Err`. → `cargo test -p camel-dsl --lib yaml_rejects_empty_key`
- `yaml_parses_cache_invalidate_and_peek_stale`: parse both step forms, assert fields. → `cargo test -p camel-dsl --lib yaml_parses_cache_invalidate_and_peek_stale`

**Acceptance:**
- `cargo build -p camel-dsl` exits 0.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.
- All 4 tests above pass.

- [x] 2.2

### camel-processor

#### Task 2.3: CacheService — the cache step runtime (materialization, write-back, error propagation)

**Files:**
- `crates/camel-processor/src/cache_eip.rs` (new)
- `crates/camel-processor/src/lib.rs` (modified — add `pub mod cache_eip;` and re-exports)

**Steps:**
1. Create `crates/camel-processor/src/cache_eip.rs`. Define `CacheService` (the compiled segment) and a shared `MockCacheRepository` test helper. Mirror how `IdempotentConsumerSegment` is structured in this crate (read `crates/camel-processor/src/idempotent_consumer.rs` first — it is the precedent for OutcomeSegment-with-sub-pipeline + repository lookup).
2. `CacheService` holds: `repository: Arc<dyn CacheRepository>`, `repository_name: String` (for OTel tagging — added in Task 3.3), `key_expr: camel_processor::MessageIdExpression` (the type is `Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>`, verified at `crates/camel-processor/src/idempotent_consumer.rs:34`; produced by `compile_message_id_expression` in Task 2.5), `ttl: Option<Duration>`, `max_entry_bytes: usize` (default `DEFAULT_MATERIALIZE_LIMIT` = 10 MiB from `crates/camel-api/src/body.rs:6`), `on_miss: camel_api::OutcomeSegment` (the compiled sub-pipeline — mirror `IdempotentConsumerSegment`'s child_pipeline field type).
3. Implement the cache step semantics as an `OutcomeSegment` (mirror `IdempotentConsumerSegment`'s impl — read how it returns `PipelineOutcome::{Continue, Stopped}` and how it handles `Err`):
   - **Evaluate `key_expr`** against the exchange: `let key = match (self.key_expr)(&exchange) { Some(k) => k, None => return self.on_miss.run(exchange).await };` — a `None` key means the exchange is not cacheable; bypass lookup/write-back and run the miss branch directly, mirroring `IdempotentConsumerSegment` (`idempotent_consumer.rs:112-114`). Do NOT error on `None`.
   - **`repository.get(&key).await`**: on `Err(e)` → propagate `Err(e)`. On `Ok(Some(entry))` (fresh) → reconstruct `Body` from `entry.content_type` + `entry.bytes` (note: `entry.bytes` is `Vec<u8>` per Task 1.1). Map: `ContentType::Bytes → Body::Bytes(Bytes::from(entry.bytes))`, `ContentType::Text → Body::Text(String::from_utf8(entry.bytes)?)`, `ContentType::Json → Body::Json(serde_json::from_slice(&entry.bytes)?)`, `ContentType::Xml → Body::Xml(String::from_utf8(entry.bytes)?)`. `ContentType` is NOT `#[non_exhaustive]` (Task 1.1), so this match is exhaustive without a wildcard arm. Set exchange body, return `PipelineOutcome::Continue` (skip on_miss). On `Ok(None)` (miss or expired) → proceed to on_miss.
   - **Run `on_miss`** as a sub-pipeline against the exchange (mirror `IdempotentConsumerSegment`'s child pipeline invocation). On `PipelineOutcome::Stopped` → propagate `Stopped` WITHOUT calling `set`. On `Err(e)` → propagate `Err(e)` WITHOUT calling `set`. On `Continue` → proceed to write-back.
   - **Write-back policy** (apply to the body now in the exchange after on_miss ran):
      - Match on `exchange.body()` (note: `Body` IS `#[non_exhaustive]`, so the match MUST include a `_` wildcard arm for forward compatibility — future variants pass through uncached):
        - `Body::Bytes(b)` / `Body::Text(_)` / `Body::Json(_)` / `Body::Xml(_)`: extract bytes via the existing `Body::bytes()` or equivalent accessor (check `body.rs` for the non-consuming accessor). Check `bytes.len() <= max_entry_bytes`. If fits → construct `CacheEntry { bytes: bytes.to_vec(), content_type: <map from variant: Bytes→ContentType::Bytes, Text→ContentType::Text, Json→ContentType::Json, Xml→ContentType::Xml>, expires_at: None }` and call `repository.set(&key, entry, ttl).await` (`set` applies `ttl` to `expires_at`). On `set` `Err` → propagate `Err`. Return `Continue`. If exceeds → log at `debug!` with `// log-policy: g:cache:oversized-skip` annotation ("cache key {} exceeds max_entry_bytes {}, skipping write-back"), pass body through unchanged, return `Continue`.
        - `Body::Stream(_)`: call `body.into_bytes(max_entry_bytes).await` (this CONSUMES the body — `body.rs:215` returns `Result<Bytes, CamelError>`). On `Ok(materialized_bytes)` → construct `CacheEntry { bytes: materialized_bytes.to_vec(), content_type: ContentType::Bytes, expires_at: None }`, replace exchange body with `Body::Bytes(materialized_bytes)`, call `repository.set(&key, entry, ttl).await`. On `Err` → propagate the `CamelError` unchanged (it is already `CamelError::StreamLimitExceeded(max_size)` per `body.rs:220` — do NOT remap to a different variant).
        - `Body::Empty` / `_` (future `#[non_exhaustive]` variants): pass through uncached, return `Continue`.
4. Add `pub struct MockCacheRepository` in a `#[cfg(test)] mod test_utils` subsection (or a sibling `cache_eip_test_utils.rs` if the crate separates test helpers) implementing `CacheRepository` with controllable `Arc<AtomicBool>` / `Arc<Mutex<...>>` knobs for `get`/`set`/`peek_stale`/`invalidate` behavior, a call counter, and entry storage via `Arc<Mutex<HashMap<String, CacheEntry>>>`. This mock is shared with Task 2.4's leaf-segment tests.
5. Run `cargo fmt && cargo clippy -p camel-processor -- -D warnings`.

**Tests:** (in `cache_eip.rs` `#[cfg(test)] mod tests`, using `MockCacheRepository`)
- `cache_hit_short_circuits_on_miss`: mock returns `Ok(Some(fresh_entry))` for `get` → run `CacheService` → assert body replaced, `on_miss` sub-pipeline NOT executed (track via a side-effect counter in the mock or a shared `Arc<AtomicU32>`), result `Continue`. → `cargo test -p camel-processor --lib cache_eip::tests::cache_hit_short_circuits_on_miss`
- `cache_miss_runs_on_miss_sets_continues`: mock `get` returns `Ok(None)`; `on_miss` sets body to `Body::Bytes(b"x")`; run → assert `set` called with key + entry containing `b"x"` + ttl, body is `b"x"`, result `Continue`. → `cargo test -p camel-processor --lib cache_eip::tests::cache_miss_runs_on_miss_sets_continues`
- `cache_miss_oversized_materialized_body_skips_writeback`: `max_entry_bytes=4`; `on_miss` produces `Body::Bytes(b"oversized")` (9 bytes); run → assert `set` NOT called (track via mock counter), body unchanged, result `Continue`. → `cargo test -p camel-processor --lib cache_eip::tests::cache_miss_oversized_materialized_body_skips_writeback`
- `cache_miss_oversized_stream_propagates_err`: `max_entry_bytes=4`; `on_miss` produces `Body::Stream(...)` exceeding 4 bytes; run → assert result is `Err(CamelError::StreamLimitExceeded(4))` (the variant from `body.rs:220`), `set` NOT called. → `cargo test -p camel-processor --lib cache_eip::tests::cache_miss_oversized_stream_propagates_err`
- `cache_on_miss_stopped_propagates_without_writeback`: `on_miss` returns `Stopped`; run → assert result `Stopped`, `set` NOT called. → `cargo test -p camel-processor --lib cache_eip::tests::cache_on_miss_stopped_propagates_without_writeback`
- `cache_on_miss_err_propagates_without_writeback`: `on_miss` returns `Err`; run → assert result `Err`, `set` NOT called. → `cargo test -p camel-processor --lib cache_eip::tests::cache_on_miss_err_propagates_without_writeback`
- `cache_repository_get_err_propagates`: mock `get` returns `Err`; run → assert result `Err`. → `cargo test -p camel-processor --lib cache_eip::tests::cache_repository_get_err_propagates`
- `cache_repository_set_err_propagates`: mock `get` returns `Ok(None)`, mock `set` returns `Err`; `on_miss` produces small body; run → assert result `Err`. → `cargo test -p camel-processor --lib cache_eip::tests::cache_repository_set_err_propagates`
- `cache_content_type_reconstruction`: for each of `ContentType::{Bytes,Text,Json,Xml}`, set a cached entry, run hit path, assert the reconstructed `Body` variant matches. → `cargo test -p camel-processor --lib cache_eip::tests::cache_content_type_reconstruction`

**Acceptance:**
- `cargo build -p camel-processor` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- All 9 tests above pass.
- `cargo xtask lint-unwrap` exits 0.
- `cargo xtask lint-log-levels` exits 0 (the `debug!` log for oversized body has a `// log-policy:` annotation).
- The cache_eip module does NOT reuse `EnrichmentStrategy::on_no_poll` (grep `on_no_poll` in `cache_eip.rs` → 0 hits).
- The stream-error path propagates `CamelError::StreamLimitExceeded` unchanged (grep `PayloadTooLarge` in `cache_eip.rs` → 0 hits).

- [x] 2.3

#### Task 2.4: CacheInvalidateSegment + CachePeekStaleSegment — leaf segments + shared test utils

**Files:**
- `crates/camel-processor/src/cache_eip.rs` (modified — add the two leaf segments; reuse `MockCacheRepository` from Task 2.3's test_utils)

**Steps:**
1. In `crates/camel-processor/src/cache_eip.rs` (created in Task 2.3), add two leaf segment structs:
   - `pub struct CacheInvalidateService { repository: Arc<dyn CacheRepository>, key_expr: camel_processor::MessageIdExpression }`. Implement `OutcomeSegment`: evaluate `key_expr`, if `None` → return `Continue` (no key = nothing to invalidate, mirror `IdempotentConsumerSegment` None-handling). If `Some(key)` → call `repository.invalidate(&key).await`, on `Err` → propagate, on `Ok` → return `PipelineOutcome::Continue`.
   - `pub struct CachePeekStaleService { repository: Arc<dyn CacheRepository>, key_expr: camel_processor::MessageIdExpression }`. Implement `OutcomeSegment`: evaluate `key_expr`, if `None` → return `PipelineOutcome::Stopped` (no key = no stale available). If `Some(key)` → call `repository.peek_stale(&key).await`. On `Err` → propagate. On `Ok(Some(entry))` → reconstruct body (same content_type mapping as Task 2.3), set exchange body, return `Continue`. On `Ok(None)` → return `PipelineOutcome::Stopped` (absence = no stale available; honest outcome in CircuitBreaker.fallback context — per spec R6).
2. Both services are `OutcomeSegment`s (NOT Process-mode) because `CachePeekStaleService` returns `Stopped` on absence — `PipelineOutcome` MUST cross the segment boundary (same reasoning as ADR-0023 Segment-not-Process for IdempotentConsumer).
3. Reuse the `MockCacheRepository` from Task 2.3's test_utils.
4. Run `cargo fmt && cargo clippy -p camel-processor -- -D warnings`.

**Tests:**
- `cache_peek_stale_serves_post_expiry_entry`: mock `peek_stale` returns `Ok(Some(entry))`; run `CachePeekStaleService` → assert body replaced, result `Continue`. → `cargo test -p camel-processor --lib cache_eip::tests::cache_peek_stale_serves_post_expiry_entry`
- `cache_peek_stale_on_absence_stops_branch`: mock `peek_stale` returns `Ok(None)`; run → assert result `Stopped`. → `cargo test -p camel-processor --lib cache_eip::tests::cache_peek_stale_on_absence_stops_branch`
- `cache_invalidate_calls_repository_invalidate`: mock tracks `invalidate` calls; run `CacheInvalidateService` → assert `invalidate("k")` called, result `Continue`. → `cargo test -p camel-processor --lib cache_eip::tests::cache_invalidate_calls_repository_invalidate`

**Acceptance:**
- `cargo build -p camel-processor` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- All 3 tests above pass.

- [x] 2.4

### camel-core (step compiler)

#### Task 2.5: BuilderStep variants + CoreCompiler arms + CompilationContext field + integration smoke test

**Files:**
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified — add 3 variants to `BuilderStep` at line 62)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs` (modified — add `cache_repositories: &'a CacheRegistry` to `CompilationContext` at line 108)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified — add 3 arms to `CoreCompiler::compile` dispatching on the new `BuilderStep` variants, mirror the `BuilderStep::IdempotentConsumer` arm at lines 87-108)
- `crates/camel-core/src/lifecycle/adapters/step_resolution.rs` (modified — thread `cache_repositories` into the compile fn at line 202 and into the `CompilationContext { .. }` literal at line 217; this is the SOLE production construction site for `CompilationContext`)
- `crates/camel-core/src/lifecycle/application/route_controller.rs` (modified — add `cache_repositories` field at line 81 mirroring `idempotent_repositories`, add `set_cache_repositories` setter mirroring line 209, pass `&self.cache_repositories` at line 318 mirroring `&self.idempotent_repositories`)
- `crates/camel-dsl/src/compile.rs` (modified — add 3 `DeclarativeStep → BuilderStep` lowering arms at line 1145 adjacent to `DeclarativeStep::IdempotentConsumer(def)`)
- `crates/camel-test/tests/cache_eip_smoke.rs` (new — end-to-end smoke test)

**Steps:**
1. In `crates/camel-core/src/lifecycle/application/route_definition.rs`, add 3 variants to `BuilderStep` (line 62), mirroring `BuilderStep::IdempotentConsumer { repository, expression, steps, eager, remove_on_failure }`:
   - `Cache { repository: Option<String>, key: String, ttl: Option<String>, max_entry_bytes: Option<usize>, on_miss: Vec<BuilderStep> }`.
   - `CacheInvalidate { repository: Option<String>, key: String }`.
   - `CachePeekStale { repository: Option<String>, key: String }`.
2. In `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs`, add `pub cache_repositories: &'a CacheRegistry,` to `CompilationContext` (line 108, after `claim_check_repositories`). Update ALL `CompilationContext` construction sites in the test helpers of mod.rs (lines ~474, 565, 646, 733, 862), `step_compilers/core.rs` (lines ~517, 627, 699), `step_compilers/endpoints.rs` (line ~326), and `step_resolution.rs` test module — each passes `&CacheRegistry::new()` (or a shared default). NOTE: `CompilationContext` is NOT constructed in `camel-dsl/src/compile.rs`; the sole production construction site is `step_resolution.rs:208` (verified by e_opus).
3. In `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs`, add 3 arms to `CoreCompiler::compile` (the `match step { ... }` block starting at line 34), mirroring the `BuilderStep::IdempotentConsumer` arm (lines 87-108):
   - `BuilderStep::Cache { repository, key, ttl, max_entry_bytes, on_miss } => { ... }`:
     - Resolve repo: `let repo_name = repository.as_deref().unwrap_or("memory"); let repo = ctx.cache_repositories.get(repo_name).ok_or_else(|| CamelError::ComponentNotFound(format!("cache: repository '{repo_name}' is not registered")))?;`
     - Compile key expression: `let key_expr = compile_message_id_expression(ctx.languages, &key)?;` (or the equivalent expression compiler — mirror how IdempotentConsumer compiles its `expression`).
     - Parse ttl: `let ttl = ttl.as_ref().map(|s| humantime::parse_duration(s)).transpose()?;`
     - Compile on_miss children: `let (child_segments, child_lifecycles) = ctx.compile_children_segments(&on_miss, registry)?; let child_pipeline = compose_outcome_segment(child_segments);` (mirror lines 105-107).
     - Construct: `let svc = camel_processor::CacheService::new(repo, repo_name.to_string(), key_expr, ttl, max_entry_bytes.unwrap_or(DEFAULT_MATERIALIZE_LIMIT), child_pipeline);`
     - Return: `Ok(CompileOutcome::Matched(CompiledStep::Segment { segment: camel_api::OutcomeSegment::new(Box::new(svc)) }))` (mirror line 116 — Segment mode, NOT Process mode).
   - `BuilderStep::CacheInvalidate { repository, key } => { ... }` — same repo+key resolution, construct `CacheInvalidateService`, return `CompileOutcome::Matched(CompiledStep::Segment { segment: OutcomeSegment::new(Box::new(svc)) })`.
   - `BuilderStep::CachePeekStale { repository, key } => { ... }` — same, construct `CachePeekStaleService`, return Segment mode.
4. In `crates/camel-dsl/src/compile.rs`, add 3 lowering arms in `compile_declarative_step` at line 1145 (adjacent to `DeclarativeStep::IdempotentConsumer(def)` at `:1145`, which calls `compile_idempotent_consumer_step` helper at `:1277`):
   - `DeclarativeStep::Cache(def) => compile_cache_step(def, stream_cache_threshold)` where the helper mirrors `compile_idempotent_consumer_step` at `:1277` (it has child steps — lower each `on_miss` child recursively). Returns `Ok(BuilderStep::Cache { repository: def.repository, key: def.key, ttl: def.ttl, max_entry_bytes: def.max_entry_bytes, on_miss: def.on_miss.into_iter().map(compile_declarative_step).collect::<Result<Vec<_>, _>>()? })`.
   - `DeclarativeStep::CacheInvalidate(def)` and `DeclarativeStep::CachePeekStale(def)` → inline `Ok(BuilderStep::CacheInvalidate { repository: def.repository, key: def.key })` / `Ok(BuilderStep::CachePeekStale { .. })` (no children, mirror the inline `ClaimCheck` arm at `:1148`).
   Also add the 3 new variants to: the `DeclarativeStep → &str` name mapper at `:1498` (where `IdempotentConsumer(_) => "idempotent_consumer"`), the not-allowed-here error mappers at `:1368`, and the route-level mapper at `:1842`.
5. Thread `cache_repositories` through the production compile path: add `cache_repositories: crate::SharedCacheRegistry` field to `RouteControllerHandle` at `route_controller.rs:81` (mirror `idempotent_repositories`), add a `set_cache_repositories` setter mirroring `:209`, and pass `&self.cache_repositories` at `:318` where `&self.idempotent_repositories` is currently passed. Add the corresponding parameter to the compile fn at `step_resolution.rs:202` and the field to the `CompilationContext { .. }` literal at `:217`. Test construction sites that need the new field: `step_compilers/mod.rs:{474,565,646,733,862}`, `step_compilers/core.rs:{517,627,699}`, `step_compilers/endpoints.rs:326`, `step_resolution.rs` test module.
6. Create `crates/camel-test/tests/cache_eip_smoke.rs` with an end-to-end test: build a `CamelContext` with default `"memory"` cache; define a route `from("direct:start").cache(key="k", ttl="1h", on_miss=[setBody("fresh")]).to("mock:result")` (use the builder API or YAML); send an exchange; assert first call hits on_miss (body="fresh"), second call hits cache (body="fresh", on_miss NOT run — track via mock counter or a shared atomic).
7. Run `cargo fmt && cargo clippy` on the affected crates.

**Tests:**
- `cache_step_compiles_and_executes_end_to_end`: the integration test in `cache_eip_smoke.rs` described above. → `cargo test -p camel-test --test cache_eip_smoke`
- `cache_invalidate_step_compiles_and_executes`: route with `cache_invalidate` then `cache` → second step misses. → `cargo test -p camel-test --test cache_eip_smoke::cache_invalidate_step_compiles_and_executes`
- `cache_peek_stale_step_compiles_and_executes`: route with `cache_peek_stale` against a pre-populated stale entry → body replaced. → `cargo test -p camel-test --test cache_eip_smoke::cache_peek_stale_step_compiles_and_executes`
- `unregistered_repository_returns_component_not_found`: compile a route with `repository: "absent"` → assert `Err(CamelError::ComponentNotFound(...))`. → `cargo test -p camel-test --test cache_eip_smoke::unregistered_repository_returns_component_not_found`

**Acceptance:**
- `cargo build --workspace` exits 0.
- `cargo clippy -p camel-core -p camel-test -- -D warnings` exits 0.
- All 4 tests above pass.
- The step compiler resolves `repository` from `ctx.cache_repositories` (grep `ctx.camel_context` in `step_compilers/` → 0 hits — that field does not exist).
- All three cache step arms return `CompileOutcome::Matched(CompiledStep::Segment { .. })` (grep `CompiledStep::Process` in the new arms → 0 hits — Segment mode is required because `CachePeekStaleService` returns `Stopped`).

- [x] 2.5

## Phase 3: Persistence + observability — redb + Camel.toml + OTel + integration test

### camel-core

#### Task 3.1: RedbCacheRepository — persistent backend with sweep task

**Files:**
- `crates/camel-core/src/cache/redb.rs` (new)
- `crates/camel-core/src/cache/mod.rs` (modified — add `pub mod redb;`)

**Steps:**
1. Create `crates/camel-core/src/cache/redb.rs`. `redb = "4"` is already a workspace dep (verify `crates/camel-core/Cargo.toml` has it; add if missing). `CacheEntry` derives `serde::Serialize`/`Deserialize` unconditionally (Task 1.1), so no feature flag is needed — `serde` is already a direct dep of camel-api.
2. Add `pub mod redb;` to `crates/camel-core/src/cache/mod.rs` (the module was NOT forward-declared in Task 1.2 to avoid a build break — add it here now that the file exists).
3. Define the redb table schema mirroring `crates/camel-core/src/idempotent/redb_repository.rs:21` (`const KEYS_TABLE: TableDefinition<&str, ()>`): `const CACHE_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("cache_entries");`. Values are `serde_json::to_vec(&entry)?`-serialized `CacheEntry` bytes (use `serde_json` — it is already a workspace dep, human-debuggable in redb inspector, and the performance difference vs bincode is negligible for cache entries). Write with `table.insert(key.as_str(), serialized.as_slice())`; read via `table.get(key.as_str())?` then `serde_json::from_slice(guard.value())`. NOTE: `serde_json::Error` has no `From<serde_json::Error> for CamelError` impl — map explicitly: `.map_err(|e| CamelError::Io(format!("cache serialization: {e}")))?` on both `to_vec` and `from_slice`. Do NOT use `Table<String, Bytes>`.
4. Define `pub struct RedbCacheRepository { name: String, db: Arc<redb::Database>, stale_retention: Duration, max_entries: Option<usize>, hits: Arc<AtomicU64>, misses: Arc<AtomicU64>, evictions: Arc<AtomicU64>, entries: Arc<AtomicU64>, shutdown_token: CancellationToken, sweep_handle: Mutex<Option<JoinHandle<()>>> }`. ALL counters are `Arc<AtomicU64>` (NOT bare `AtomicU64>) because the spawned sweep task captures clones of them — `tokio::spawn` requires `'static` captures (verified pattern at `route_registry.rs:123`). The `shutdown_token` is the CONTEXT's token (passed in from `CamelContext::shutdown_token()` at Task 1.3). `sweep_interval` is NOT stored as a field — it is consumed by the constructor to build the `tokio::time::interval(sweep_interval)` ticker passed into the spawned sweep loop; no method reads it afterward, so storing it would trigger `dead_code` under `-D warnings`.
5. `impl RedbCacheRepository { pub async fn new(name: impl Into<String>, path: impl Into<PathBuf>, stale_retention: Duration, max_entries: Option<usize>, sweep_interval: Duration, shutdown_token: CancellationToken) -> Result<Self, CamelError> }` — **ASYNC**, mirroring `RedbIdempotentRepository::new` at `redb_repository.rs:45` which is `pub async fn new`. The `shutdown_token` parameter binds the sweep task to the context's shutdown signal, satisfying spec R5: `CamelContext::stop()` → `stop_context()` → `cancel_token.cancel()` → sweep's `select!` on the cloned token fires → sweep exits.
   - **Construction sequence** (all blocking ops via `spawn_blocking`, mirroring `RedbIdempotentRepository::new`):
     a. Create parent dir (`std::fs::create_dir_all`).
     b. `tokio::task::spawn_blocking(move || { let db = redb::Database::create(&path)?; let db = Arc::new(db); let txn = db.begin_write()?; let table = txn.open_table(CACHE_TABLE)?; let len = table.len()?; // ReadableTableMetadata::len txn.commit()?; Ok((db, len)) }).await??` — returns `(Arc<Database>, u64)`.
     c. Initialize the `Arc<AtomicU64>` counters: `let entries = Arc::new(AtomicU64::new(len));` (from step b), `let hits = Arc::new(AtomicU64::new(0)); let misses = Arc::new(AtomicU64::new(0)); let evictions = Arc::new(AtomicU64::new(0));`.
     d. **Spawn the sweep loop** capturing CLONED Arc state + a clone of the shutdown token (NOT `&self` — `self` does not exist yet): `let db_clone = Arc::clone(&db); let evictions_clone = Arc::clone(&evictions); let entries_clone = Arc::clone(&entries); let token_clone = shutdown_token.clone(); let handle = tokio::spawn(async move { let mut ticker = tokio::time::interval(sweep_interval); loop { tokio::select! { _ = ticker.tick() => { let db = Arc::clone(&db_clone); let ev = Arc::clone(&evictions_clone); let en = Arc::clone(&entries_clone); let reclaimed = tokio::task::spawn_blocking(move || { /* open txn, scan, delete expired+retention, return count */ }).await.unwrap_or(0); ev.fetch_add(reclaimed, Relaxed); en.fetch_sub(reclaimed, Relaxed); } _ = token_clone.cancelled() => break, } } });`
     e. Construct `Self { name, db, stale_retention, max_entries, hits, misses, evictions, entries, shutdown_token, sweep_handle: Mutex::new(Some(handle)) }`.
   - Add `pub(crate) async fn sweep_once(&self) -> Result<u64, CamelError>` running one reclamation pass in `spawn_blocking`: open a write txn, scan all entries, delete those whose `expires_at + stale_retention < now`, return reclaimed count. The method clones `Arc::clone(&self.db)`, `Arc::clone(&self.evictions)`, `Arc::clone(&self.entries)` into the `spawn_blocking` closure. Unit tests call this directly instead of racing the interval timer.
6. `#[async_trait] impl CacheRepository for RedbCacheRepository`:
   - All methods wrap blocking redb I/O in `tokio::task::spawn_blocking` (mirror how `RedbIdempotentRepository` does it in `crates/camel-core/src/idempotent/redb_repository.rs` — read that file first).
   - `get`: read entry via `spawn_blocking`, deserialize, check `expires_at`: if expired → incr `misses`, return `Ok(None)`. Else incr `hits`, return `Ok(Some(entry))`.
   - `set`: serialize entry, apply `ttl` to `expires_at` if `Some`, then perform **existence check + capacity enforcement + insert ALL in ONE write transaction** inside a single `spawn_blocking` closure (eliminates TOCTOU): `spawn_blocking(move || { let mut txn = db.begin_write()?; let mut table = txn.open_table(CACHE_TABLE)?; let prior = table.get(key.as_str())?; if prior.is_none() { let count = table.len()? as usize; // ReadableTableMetadata::len — actual row count INSIDE the locked txn if let Some(max) = max_entries { if count >= max { return Err(CamelError::Config(format!("cache: max_entries ({max}) exceeded"))); } } } let was_new = prior.is_none(); table.insert(key.as_str(), serialized.as_slice())?; txn.commit()?; Ok(was_new) }).await??`. Increment `entries` atomic only if the returned `was_new == true` (best-effort post-commit update for stats — the authoritative cap enforcement is `table.len()` inside the txn lock).
   - `peek_stale`: read entry via `spawn_blocking`, return `Ok(Some(entry))` ignoring expiry.
   - `invalidate`: delete key via `spawn_blocking` using `table.remove(key.as_str())?` (verified API at `redb_repository.rs:164`; returns `Option<AccessGuard<[u8]>>` — decr `entries` only if the return is `Some(..)`, i.e. a row was actually removed).
   - `clear`: delete all via `spawn_blocking`, set `entries` to 0.
   - `stats`: return `CacheStats { hits, misses, evictions, entries }` all from atomic loads (NO redb transaction needed — the sync method reads atomics only).
7. `impl Drop for RedbCacheRepository`: call `self.sweep_handle.lock().take().abort()` (abort the JoinHandle — non-blocking). Do NOT call `self.shutdown_token.cancel()` — the token is owned by the `CamelContext` and shared with other subsystems; cancelling it here would shut down the ENTIRE context when a single repository is dropped or replaced. The sweep task's `select!` already listens on the context token; it will exit when `CamelContext::stop()` cancels the token.
8. Run `cargo fmt && cargo clippy -p camel-core -- -D warnings`.

**Tests:**
- `entries_survive_handle_drop_and_reopen`: create a `CancellationToken`, `RedbCacheRepository::new("p", tmpdir/"c.redb", 24h, None, 10ms, token.clone()).await`, `set("k", entry, Some(1h)).await`, drop handle, create a new token, `RedbCacheRepository::new("p", tmpdir/"c.redb", 24h, None, 10ms, new_token.clone()).await`, assert `get("k").await == Ok(Some(entry))` and `stats().entries >= 1`. → `cargo test -p camel-core --lib cache::redb::tests::entries_survive_handle_drop_and_reopen`
- `peek_stale_returns_post_expiry_entry_on_redb`: `set("k", entry, Some(1ms)).await`, sleep 10ms, assert `peek_stale("k").await == Ok(Some(entry))`. → `cargo test -p camel-core --lib cache::redb::tests::peek_stale_returns_post_expiry_entry_on_redb`
- `sweep_once_removes_entries_past_stale_retention`: `set("k", entry, Some(1ms)).await` with `stale_retention = 10ms`, sleep 50ms, call `sweep_once().await` directly, assert return value `>= 1` and `peek_stale("k").await == Ok(None)` and `stats().evictions >= 1`. → `cargo test -p camel-core --lib cache::redb::tests::sweep_once_removes_entries_past_stale_retention`
- `sweep_stops_on_context_shutdown`: construct with a `CancellationToken`, assert sweep `JoinHandle` is alive, call `token.cancel()` (simulating `CamelContext::stop()`), assert the sweep `JoinHandle` completes within 5s (`tokio::time::timeout`). This covers the blessed spec R5 scenario. → `cargo test -p camel-core --lib cache::redb::tests::sweep_stops_on_context_shutdown`
- `redb_errors_surface_as_err`: remove the backing file beneath the handle, call `get("k")`, assert `Err(CamelError::Io(..))`. → `cargo test -p camel-core --lib cache::redb::tests::redb_errors_surface_as_err`
- `overwrite_does_not_inflate_entries`: `set("k", entry_a, None).await`, `set("k", entry_b, None).await`, assert `stats().entries == 1`. → `cargo test -p camel-core --lib cache::redb::tests::overwrite_does_not_inflate_entries`
- `max_entries_rejects_new_key_allows_overwrite`: `new("m", tmp, 24h, Some(2), 10ms, token)`, `set("a", ea, None).await`, `set("b", eb, None).await`, assert `set("c", ec, None).await == Err(CamelError::Config(..))` (at capacity), but `set("a", ea2, None).await == Ok(())` (overwrite existing key succeeds). → `cargo test -p camel-core --lib cache::redb::tests::max_entries_rejects_new_key_allows_overwrite`

**Acceptance:**
- `cargo build -p camel-core` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- All 7 tests pass.
- All redb I/O wrapped in `spawn_blocking` (grep `.await` in redb-txn paths → 0 hits; all inside `spawn_blocking` closures).
- All counters are `Arc<AtomicU64>` (grep `AtomicU64` without `Arc<` in redb.rs → 0 hits).
- `cargo xtask lint-unwrap` exits 0.

- [x] 3.1

### camel-config

#### Task 3.2: CacheRepoConfig — Camel.toml [default.cache_repo] with backend discriminator

**Files:**
- `crates/camel-config/src/config.rs` (modified — add `cache_repo: Option<CacheRepoConfig>` field, `CacheRepoConfig` struct, validation)
- `crates/camel-config/src/context_ext.rs` (modified — register `"persistent"` redb when `backend == "redb"`)

**Steps:**
1. In `config.rs`, define:
   ```rust
   pub struct CacheRepoConfig {
       pub backend: String,  // "memory" | "redb"
       pub max_capacity: Option<usize>,      // memory only; default 10_000
       pub path: Option<String>,              // redb only; required when backend=redb
       pub stale_retention: Option<humantime::Duration>,  // redb only; default 168h (7 days)
       pub max_entries: Option<usize>,        // redb only; default 1_000_000
   }
   ```
   Mirror `RedbIdempotentConfig` for field style, env-var flattening (`cache_repo_backend`, `cache_repo_path`, etc.), profile section parsing (`[default.cache_repo]`), and defaults population.
2. Add `pub cache_repo: Option<CacheRepoConfig>` to `CamelConfig` (mirror `idempotent_repo`).
3. Add validation in `CamelConfig::validate()`: if `cache_repo.backend == "redb"` and `path` is empty/None → return error naming the field. If `backend` is not `"memory"` or `"redb"` → return error.
4. In `context_ext.rs`, find the block that registers redb idempotent when `idempotent_repo` is set (lines ~219-229). Add an analogous block for cache:
   - If `config.cache_repo` is `Some` and `backend == "redb"`: construct `RedbCacheRepository::new("persistent", path, stale_retention, max_entries, DEFAULT_SWEEP_INTERVAL, ctx.shutdown_token()).await` (ASYNC constructor + the context's shutdown token). Call `ctx.register_cache_repository("persistent", Arc::new(repo))?`.
   - If `backend == "memory"` and `max_capacity` is `Some`: the default `"memory"` (capacity 10_000) was already registered by `context_builder.rs` (Task 1.3). To override its capacity, call `ctx.replace_cache_repository("memory", Arc::new(MemoryCacheRepository::new("memory", configured_capacity)))` (the PUBLIC `replace_cache_repository` method added to CamelContext in Task 1.3 — camel-config cannot access the `pub(crate)` `NamedRegistry::register_or_replace` directly). If `max_capacity` is `None`, leave the default 10_000 registration untouched.
5. Run `cargo fmt && cargo clippy -p camel-config -- -D warnings`.

**Tests:**
- `redb_registered_when_backend_redb`: build context from config with `cache_repo.backend="redb"`, `.path=Some(tmpdir)`, assert `cache_repository("persistent")` returns `Some` and `cache_repository("memory")` returns `Some`. → `cargo test -p camel-config --test cache_repo_config -- redb_registered_when_backend_redb`
- `redb_absent_when_backend_memory_or_unset`: build from default config (no `cache_repo`), assert `cache_repository("persistent")` returns `None` and `cache_repository("memory")` returns `Some`. → `cargo test -p camel-config --test cache_repo_config -- redb_absent_when_backend_memory_or_unset`
- `empty_redb_path_rejected_at_validation`: `CacheRepoConfig { backend: "redb".into(), path: None, .. }`, call `validate()`, assert `Err` naming `path`. → `cargo test -p camel-config --test cache_repo_config -- empty_redb_path_rejected_at_validation`
- `profile_section_loads`: parse `[default.cache_repo]\nbackend="redb"\npath="x"\nstale_retention="168h"` via the profile mechanism (mirror the `redb_idempotent_config_loads_via_profile_section` test at config.rs:1908). → `cargo test -p camel-config --test cache_repo_config -- profile_section_loads`
- `memory_max_capacity_supplied_via_config`: config with `backend="memory"`, `max_capacity=Some(5000)`, build context, insert 5001 distinct keys via `cache_repository("memory")`, bounded-poll `stats().entries` (retry every 10ms up to 1s) until it stabilizes at 5000 (moka evicts asynchronously; the `Arc<dyn CacheRepository>` trait face does not expose `run_pending_tasks` — use the public `stats()` API with polling). Compare against default-capacity context where all 5001 fit and `stats().entries == 5001`. → `cargo test -p camel-config --test cache_repo_config -- memory_max_capacity_supplied_via_config`
- `memory_max_capacity_defaults_when_omitted`: config with `backend="memory"`, `max_capacity=None`, build context, insert 10001 distinct keys, bounded-poll `stats().entries` until it stabilizes at 10000 (default capacity). → `cargo test -p camel-config --test cache_repo_config -- memory_max_capacity_defaults_when_omitted`

**Acceptance:**
- `cargo build -p camel-config` exits 0.
- `cargo clippy -p camel-config -- -D warnings` exits 0.
- All 6 tests pass.
- `cargo xtask lint-non-exhaustive` exits 0 (CacheStats, ContentType, CacheEntry all NOT `#[non_exhaustive]`).

- [x] 3.2

### camel-processor

#### Task 3.3: OTel metrics at the cache segment boundary — camel.cache.hits / camel.cache.misses

**Files:**
- `crates/camel-processor/src/cache_eip.rs` (modified — add OTel emission at the segment boundary)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified — pass `ctx.rt` clone into `CacheService::new(...)` at the `BuilderStep::Cache` arm)

**Steps:**
1. Read `crates/camel-processor/src/cache_eip.rs` (created in Task 2.3). Find the point where the cache step determines hit vs miss (the `repository.get` result branch).
2. Read how existing segments emit OTel metrics (search for `RuntimeObservability::metrics` or `camel.cache` or `u64Counter` in `crates/camel-processor/src/`). Mirror that pattern.
3. Add `rt: Arc<dyn RuntimeObservability>` field to `CacheService` (constructed in `core.rs` step compiler where `ctx.rt: Arc<dyn RuntimeObservability>` is already available — the same `rt` that other segments use). Update `CacheService::new(...)` signature to accept `rt` and pass `ctx.rt.clone()` from `core.rs` at the `BuilderStep::Cache` arm.
4. Inside `CacheService::run`, at the hit/miss determination point, call the `MetricsCollector` API (verified at `crates/camel-api/src/metrics.rs`):
   - On hit: `self.rt.metrics().record_counter("camel.cache.hits", 1.0_f64, &[("repository", &self.repository_name)]);`
   - On miss: `self.rt.metrics().record_counter("camel.cache.misses", 1.0_f64, &[("repository", &self.repository_name)]);`
   The `MetricsCollector` trait has `fn record_counter(&self, _name: &str, _value: f64, _labels: &[(&str, &str)])` (verified — parameter is `f64`, NOT `u64`). The emission lives in `CacheService`.
5. Run `cargo fmt && cargo clippy -p camel-processor -p camel-core -- -D warnings`.

**Tests:** (in `crates/camel-processor/src/cache_eip.rs` `#[cfg(test)] mod tests`)
- `cache_step_hit_increments_otel_counter`: construct a `RecordingMetricsCollector` mock (a test struct implementing `MetricsCollector` that stores `record_counter` calls in a `Arc<Mutex<Vec<(String, f64, Vec<(String, String)>)>>>`). Wrap it in a test `RuntimeObservability` impl. Construct `CacheService` with this `rt`. Run a hit. Assert the recording contains `("camel.cache.hits", 1.0, [("repository", ...)])`. → `cargo test -p camel-processor --lib cache_eip::tests::cache_step_hit_increments_otel_counter`
- `cache_step_miss_increments_otel_counter`: same setup, run a miss, assert `("camel.cache.misses", 1, ...)`. → `cargo test -p camel-processor --lib cache_eip::tests::cache_step_miss_increments_otel_counter`

**Acceptance:**
- `cargo build -p camel-processor` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- Both OTel tests pass.
- The emission is at the cache segment boundary (grep the repository trait impl files — `camel.cache.hits` should NOT appear in `memory.rs` or `redb.rs`).

- [x] 3.3

### camel-test

#### Task 3.4: Integration test — circuitBreaker.fallback{ cache_peek_stale } resilience (EFFIS shape)

**Files:**
- `crates/camel-test/tests/cache_resilience.rs` (new)

**Steps:**
1. Create `crates/camel-test/tests/cache_resilience.rs`. This test demonstrates the anchor use case: a route that fetches an upstream tile, caches it persistently, and on upstream failure serves the stale cached value via CircuitBreaker fallback.
2. Build a `CamelContext` with `[cache_repo] backend = "redb"`, `path = <tmpdir>`, `stale_retention = "168h"`. Register a mock upstream component `"mock:upstream"` that fails after the first call (returns `Err` on the second call, simulating EFFIS stress).
3. Define a route (YAML or programmatic) — this is the EFFIS anchor shape: the cache step wraps the upstream fetch inside `on_miss`, and on upstream failure the circuitBreaker routes to the `cache_peek_stale` fallback:
   ```yaml
   - from:
       uri: "direct:tile"
       steps:
         - circuitBreaker:
             threshold: 1
             steps:
               - cache:
                   repository: persistent
                   key: "${header.tile_path}"
                   ttl: 1ms          # force quick expiry so peek_stale is the only path on retry
                   on_miss:
                     - to: "mock:upstream"
             fallback:
               - cache_peek_stale:
                   repository: persistent
                   key: "${header.tile_path}"
   ```
4. First exchange: cache miss → on_miss runs → upstream succeeds (`mock:upstream` returns 200) → entry cached (ttl 1ms) → body returned. Second exchange (after 10ms, upstream now failing): cache `get` returns `Ok(None)` (entry expired) → on_miss runs → upstream FAILS (`mock:upstream` returns 500) → circuitBreaker opens → fallback runs → `cache_peek_stale` returns the expired-but-retained entry → body = stale entry → exchange succeeds (no error propagates).
5. Assert: second exchange returns the stale body (NOT an error).
6. Run `cargo fmt && cargo clippy -p camel-test -- -D warnings`.

**Tests:**
- `circuit_breaker_fallback_serves_stale_on_upstream_failure`: the scenario above. First exchange returns fresh body; second exchange (upstream failing) returns stale body (the cached entry), not an error. → `cargo test -p camel-test --test cache_resilience -- circuit_breaker_fallback_serves_stale_on_upstream_failure`

**Acceptance:**
- `cargo build -p camel-test` exits 0.
- `cargo clippy -p camel-test -- -D warnings` exits 0.
- The integration test passes.
- `cargo xtask lint-context-citations` exits 0.

- [x] 3.4
