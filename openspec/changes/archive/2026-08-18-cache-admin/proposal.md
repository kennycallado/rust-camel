# Proposal: cache-admin

## Why

camel-cache (the downstream app) operates 6 geo tile proxies on the Cache EIP with a
persistent redb repository, TTLs 6min–48h, hand-rolled SWR, and 7-day stale-fallback.
Real operations currently need workarounds because the engine only exposes the
read/write face of ADR-0056:

- Total reset requires stopping the engine and deleting `var/cache.redb` — the trait
  has `clear()` but no DSL step reaches it.
- Inspection is a hack (`strings` over the redb file, etc/cache-keys.sh) — `stats()`
  exists but is unreachable from routes.
- `peek_stale_served` and `invalidations` are not counted anywhere — operators cannot
  tell whether the stale-fallback and invalidation paths actually fire.
- Namespace purge (all RainViewer keys, all GIBS keys) is impossible; the app carries a
  generation-token workaround (re-keying every key with `:gN:`) that adds one peek per
  request across 6 proxies.
- N concurrent requests for the same cold key fire N upstream fetches — the exact burst
  pattern that got the app rate-banned (RainViewer 429) and forced client-side
  serialization.

## What Changes

Two delivery phases (see design.md):

- **Phase 1 (P1)** — expose what the trait already has: `cache_clear` and `cache_stats`
  DSL steps (DSL AST, canonical spec, step compilers, schema, parity); extend
  `CacheStats` with `peek_stale_served`, `invalidations`, `bytes: Option<u64>`; emit
  `camel.cache.peek_stale_served` and `camel.cache.invalidations` OTel counters.
- **Phase 2 (P2)** — new capabilities: `cache_invalidate` gains `key_prefix` (namespace
  invalidation via a default `invalidate_prefix` trait method — the extension path
  ADR-0056 sanctions); `cache` gains `coalesce_misses` (per-key singleflight on
  concurrent misses).

**Excluded (YAGNI / deferred)**: P3 items — per-entry metadata inspection
(`CamelCacheEntryExpiresAt`/`Bytes` properties), native SWR step option, key listing
(`cache_keys`). Also excluded per ADRs: Redis backend, native backend TTL,
content-addressing.

## Affected crates

camel-api (CacheStats, trait default method, CanonicalStepSpec), camel-processor
(services, counters, singleflight), camel-core (BuilderStep, step compilers),
camel-dsl (route_ast, compile), camel-builder (step names), camel-test (integration).

## Acceptance criteria

- `cache_clear: { repository }` empties the repository; subsequent `get` misses.
- `cache_stats: { repository }` replaces the body with a JSON snapshot of `CacheStats`
  including the new counters.
- OTel exporter observes `camel.cache.peek_stale_served` / `camel.cache.invalidations`.
- `cache_invalidate` with `key_prefix: "ns:"` removes exactly the namespace, sets
  `CamelCacheInvalidatedCount`; both `key`+`key_prefix` or neither fails at compile time.
- `coalesce_misses: true`: 3 concurrent cold-key exchanges run `on_miss` exactly once;
  default remains per-exchange.
- All quality gates green (fmt, clippy, xtask lints, schema-check).

## Risk budget

Acceptable: additive DSL surface, one default async trait method (non-breaking path per
ADR-0056), `CacheStats` field additions (source-breaking only for external
struct-literal constructors; migration = append `..Default::default()`; in-workspace
sites updated). Out of bounds: touching the 7-method core trait contract, moka
eviction semantics, redb file format changes, any Redis work.
