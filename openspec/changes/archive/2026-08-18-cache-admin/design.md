# Design: cache-admin

## Approach

Expose the ADR-0056 admin surface (clear/stats) as DSL steps, extend observability
counters, then add namespace invalidation and miss coalescing. Every new step follows
the existing cache step pattern: `OutcomeSegment` (Segment-not-Process, ADR-0023),
repository resolved by name at compile time from `ctx.cache_repositories` (default
`"memory"`, unknown name → `ComponentNotFound` at route compile), processor-construction
monopoly stays in camel-core `StepCompilerRegistry`.

**Phase 1 pieces**

- `CacheClearService`: resolve repo, `clear().await`, `Err` → `Failed`, else `Completed`.
- `CacheStatsService`: `stats()` (sync pull), body ← JSON snapshot `{ repository, hits,
  misses, evictions, entries, peek_stale_served, invalidations, bytes }`; add
  `Serialize` on `CacheStats` (additive; camel-api already deps serde).
- `CacheStats` new fields: `peek_stale_served: u64`, `invalidations: u64`,
  `bytes: Option<u64>`. Source compatibility: additive for readers, but
  SOURCE-BREAKING for external struct-literal constructors — migration is appending
  `..Default::default()`; in-workspace literal sites (all updated by this change):
  `crates/camel-api/src/cache.rs` (trait-default test), `crates/camel-core/src/cache/memory.rs`,
  `crates/camel-core/src/cache/redb.rs`; the processor-test `MockCacheRepository` uses
  the inherited default `stats()` (no literal). Backends: memory/redb count the two
  counters via the existing `AtomicU64` pattern; `bytes` = `None` on memory (moka has
  no iteration; size-eviction counts entries, not bytes), `Some(sum)` on redb via range
  iteration at `stats()` call time (admin-frequency O(n), acceptable).
- OTel counters at segment level (per the existing boundary decision):
  `camel.cache.peek_stale_served` on `CachePeekStaleService` hit (fresh-or-stale),
  `camel.cache.invalidations` on `CacheInvalidateService` success — counted as
  successful OPERATIONS (+1 per exact-key or namespace call), not entries (the backend
  cannot report whether an exact-key call removed anything; entry counts surface only
  via `CamelCacheInvalidatedCount`). Labeled by repository, same as hits/misses.

**Phase 2 pieces**

- Namespace invalidation: default async method on `CacheRepository`:
  `async fn invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError>` returning
  `Err` naming the backend by default. This is the "default method" extension path
  ADR-0056's Interface-stability consequence sanctions; the 7 core methods stay
  untouched. redb overrides with ordered-range deletion (natural on a sorted KV);
  memory override returns `Err` (moka exposes no iteration — a shadow keyset index
  would desync under size-eviction; rejected). DSL: `cache_invalidate` accepts exactly
  one of `key` | `key_prefix` (both expressions; compile-time reject both/neither);
  resolved-`None` prefix → `Completed` (mirrors key behavior). Success sets exchange
  property `CamelCacheInvalidatedCount` (exact-key = 1 — the backend cannot report
  absence; prefix = returned count). Counter `camel.cache.invalidations` increments
  once per successful operation.
- Singleflight: `CacheConfig.coalesce_misses` (bool, default false). Compile-time
  in-flight map `Arc<Mutex<HashMap<String, Arc<InFlight>>>>` shared across segment
  clones, keyed by resolved cache key. `InFlight` = condition-style cell:
  `Mutex<Option<Terminal>>` + `tokio::sync::Notify`. Cancellation-safety contract:
  (1) the leader writes the terminal slot BEFORE `notify_waiters`, and woken waiters
  re-read the slot — never rely on notify alone (it wakes only currently-registered
  waiters); (2) waiter registration happens under the map lock atomically with the
  lookup (find terminal → consume; find in-flight → register; none → become leader);
  (3) the leader installs a Drop guard publishing `Failed(cancellation)` and removing
  the map entry when its future is dropped, so a cancelled/shutting-down route never
  strands waiters. Semantics: waiters receive the leader's **body** on success (their
  own exchange continues `Completed`); leader `Failed` → waiters `Failed` with the same
  error (one attempt per wave — the anti-burst property); leader `Stopped` → waiters
  return `Stopped` for their own exchange (branch-filter semantics). Map entry removed
  on every leader terminal state (no leak, next wave re-fetches). Waiters do not write
  back (leader's `set` is the single write).

## Affected crates

- camel-api: `CacheStats` fields + `Serialize`, `invalidate_prefix` default method,
  `CanonicalStepSpec::{CacheClear, CacheStats}`, `CacheConfig.coalesce_misses` mirror.
- camel-processor: `CacheClearService`, `CacheStatsService`, counters in
  `CachePeekStaleService`/`CacheInvalidateService`, singleflight in `CacheService`,
  `CamelCacheInvalidatedCount`.
- camel-core: `BuilderStep::{CacheClear, CacheStats}`, `CacheInvalidate` prefix variant
  data, compiler arms.
- camel-dsl: `route_ast` step structs, `compile.rs` DSL→Builder→Canonical, schema
  derive, parity tests, schema_validation tests.
- camel-builder: `step_name` arms.
- camel-test: integration coverage (clear→miss, stats snapshot, prefix purge,
  coalesced concurrent misses).

## Architecture boundaries

DSL layer stays a thin AST→canonical translator (no processor construction); camel-core
keeps the processor-construction monopoly via `StepCompilerRegistry`; camel-processor
owns EIP semantics; camel-api owns the port. `cache_stats` is a data-plane step that
runs in-route (same as `cache_peek_stale` setting properties) — no control-plane/admin
API is introduced (explicitly out of scope per the request). Metrics stay at the
segment/EIP boundary, never on trait methods.

## Phases

### Phase 1: Expose clear/stats + counters (P1)

- **Goal:** DSL-reachable reset and inspection; peek/invalidation observability.
- **Dependencies:** none (pure exposure of existing trait surface).
- **Externally-visible types/interfaces:** `cache_clear`, `cache_stats` step kinds;
  `CacheStats { peek_stale_served, invalidations, bytes }`; OTel counters
  `camel.cache.peek_stale_served`, `camel.cache.invalidations`.
- **Deliverable:** merged code + schema + parity tests + CONTEXT-MAP.md step list
  update.
- **Exit-criteria:** parity DSL↔canonical for both steps; integration test
  clear→miss; stats JSON snapshot asserted; counters observed in RecordingMetrics;
  fmt/clippy/xtask lints/schema-check green.

### Phase 2: Namespace invalidation + singleflight (P2)

- **Goal:** purge by prefix; one upstream fetch per concurrent cold-key wave.
- **Dependencies:** Phase 1 (`invalidations` counter consumed by the prefix path).
- **Externally-visible types/interfaces:** `invalidate_prefix` default trait method;
  `cache_invalidate.key_prefix`; `CamelCacheInvalidatedCount` property;
  `cache.coalesce_misses` option.
- **Deliverable:** merged code + tests + CONTEXT-MAP.md/ADR-0056 consequence note
  (separate-trait anticipation → default-method choice).
- **Exit-criteria:** redb prefix purge removes exactly the namespace; memory backend
  returns `Err` naming itself; compile-time both/neither rejection; 3-concurrent-miss
  test runs `on_miss` once; default-off behavior unchanged; gates green.

## Alternatives considered

- **Separate `CacheKeyAdmin` trait + side registry** (ADR-0056's other anticipated
  path): rejected — dual registration plumbing (`Arc<dyn>` downcast impossible),
  contract duplication; the default-method path keeps one registry lookup.
- **Memory shadow keyset (DashSet) for prefix purge**: rejected — desyncs under moka
  size-eviction (stale index entries grow unbounded without eviction_listener wiring);
  anchor use case is persistent redb.
- **Sharing leader failure with waiters vs per-waiter retry**: chose sharing — one
  attempt per wave is the anti-burst property that motivated the feature (RainViewer
  429); a transient failure surfaces identically to an own-on_miss failure.
- **Wildcard pattern matching instead of prefix**: rejected — redb range iteration is
  prefix-natural; patterns need full scans (YAGNI).
