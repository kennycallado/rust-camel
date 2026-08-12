# Proposal: add-cache-repository

## Why

rust-camel has two storage repositories — `IdempotentRepository` (ADR-0023, key-only, no
TTL) and `ClaimCheckRepository` (ADR-0028, payload-owning, no expiry). **Neither crosses the
time axis.** The missing framework verb: *lookup-or-compute with autonomous expiration*.

The motivating anchor (bd `rc-wp3s`): a consumer of EFFIS/GIBS geo layers (PNG tiles +
GeoJSON) whose upstream is stressed during peak hours. The cache must serve tiles within
their TTL window, and serve the *last-good* tile after TTL expiry when the upstream fails
(resilience). The cache must also survive process restart so the "last-good" tier is not
lost on redeploy. This anchor generalizes: any integration that wants TTL-based
lookup-or-compute with pluggable backends (memory, persistent disk, future distributed) is
a consumer.

Architectural rulings: `docs/rulings/cache-repository-ruling-2026-08-11.md` (e_opus,
2026-08-11) and its addendum (kenny's 4 revisions, same date). Both bless the design below.

## What Changes

**In scope (v1):**

- New port `CacheRepository` in `camel-api` — `Result`-returning, object-safe, stores
  `CacheEntry { bytes, content_type, expires_at }` (NOT `Body` — `Body` is not `Serialize`
  and `Body::Stream` is single-consumption). Methods: `get`, `set`, `peek_stale`,
  `invalidate`, `clear`, `stats`. `peek_stale` ignores in-band expiry — the resilience hook
  consumed by `CircuitBreaker.fallback`.
- New types in `camel-api`: `CacheEntry`, `ContentType` (`#[non_exhaustive]` per ADR-0049),
  `CacheStats` (`#[non_exhaustive]` per ADR-0049).
- `MemoryCacheRepository` in `camel-core` — `moka`-backed (new workspace dep), size-eviction
  only (no native TTL — see design), registered as default `"memory"` with mandatory
  `max_capacity` (ADR-0033 safe-defaults, default 10_000 entries).
- `RedbCacheRepository` in `camel-core` — opt-in persistent backend via new
  `[default.cache_repo]` Camel.toml section with a `backend = "redb"` discriminator
  (mirrors `[default.idempotent_repo]`). The EFFIS anchor case configures persistence with:
  ```toml
  [default.cache_repo]
  backend = "redb"
  path = "data/cache.redb"
  stale_retention = "168h"   # keep stale entries 7 days past expires_at for resilience
  ```
- In-band `expires_at` everywhere (no native backend TTL eviction) — uniform `peek_stale`
  semantics across backends; prerequisite for stale-on-error composition.
- New EIP face: `cache`, `cache_invalidate`, `cache_peek_stale` DSL steps. `cache` uses a
  new `CacheSegment` (Filter/doTry-shaped, NOT `EnrichmentStrategy.on_no_poll` reuse — that
  hook is passthrough, no write-back).
- `NamedRegistry<dyn CacheRepository>` wiring on `CamelContext`, copied verbatim from
  ADR-0028.
- `stats()` returning `CacheStats { hits, misses, evictions, entries }` + OTel metrics
  `camel.cache.{hits,misses}`.
- New ADR: `CacheRepository` as a **separate port** (not extending `ClaimCheckRepository`),
  records 6 decisions (see design §6).
- `CONTEXT-MAP.md` Key Terms: `CacheRepository`, `CacheEntry`, "Cache EIP vs stream_cache"
  disambiguation.
- Integration test demonstrating `circuitBreaker{ fetch }.fallback{ cache_peek_stale }`.

**Explicitly OUT:**

- `cache://` component (rejected — ADR-0001 write-only producer invariant + ADR-0046; the
  `camel-redis` `value_to_redis_arg.to_string()` antipattern).
- `Body` as stored type (not `Serialize`, `Stream` un-cacheable).
- `camel-cache-redis` crate (deferred to v1.1).
- `mget`/`mset`, `invalidate_pattern`, `grace`/SWR semantics (deferred).
- `moka` custom `Expiry` (rejected — conflicts with `peek_stale`; see design).

## Acceptance criteria

- `CacheRepository` trait + types in `camel-api`, `#[non_exhaustive]` where ADR-0049 mandates.
- `MemoryCacheRepository` (moka) and `RedbCacheRepository` in `camel-core`, both wired via
  `NamedRegistry` and resolvable by name on `CamelContext`.
- `cache`, `cache_invalidate`, `cache_peek_stale` DSL steps compile and execute end-to-end.
- `[default.cache_repo]` Camel.toml section opt-in works (mirrors `[default.idempotent_repo]`).
- `peek_stale` returns entries post-expiry on ALL backends (regression test).
- `circuitBreaker.fallback{ cache_peek_stale }` integration test demonstrates resilience.
- New ADR-0056 records the 6 decisions; CONTEXT-MAP Key Terms updated.
- `moka` workspace dep pinned; no `cache://` component introduced.
- `cargo fmt --check`, `cargo clippy -D warnings`, `cargo xtask lint-{unwrap,secrets,
  non-exhaustive,log-levels,ignore}`, `cargo test -p camel-core --lib` all green.

## Risk budget

- **Acceptable:** new workspace dep `moka` (Tokio-native, widely adopted, well-maintained);
  in-band-expiry complexity (sweep task per backend); minor DSL parser surface addition.
- **Out of bounds:** distributed cache semantics (redis) in v1; bulk operations; breaking
  changes to `IdempotentRepository` or `ClaimCheckRepository` (neither is touched); changes
  to `Body` (we materialize, not mutate).
- bd: `rc-wp3s`.
