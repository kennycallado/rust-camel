## MODIFIED Requirements

### Requirement: CacheRepository port with in-band expiry

The system SHALL provide a `CacheRepository` trait in `camel-api` that is object-safe,
`Result`-returning, and stores `CacheEntry { bytes: Vec<u8>, content_type: ContentType,
expires_at: Option<SystemTime> }`. The trait SHALL expose `get`, `set`, `peek_stale`,
`invalidate`, `clear`, and a default `stats` method. `get` SHALL return `Ok(None)` when the
key is absent OR when the entry's in-band `expires_at` has elapsed (NEVER silently swallow a
backend read failure as a miss — Contract C1 inherited from ADR-0023). `peek_stale` SHALL
return the entry IGNORING in-band expiry (it returns `Ok(None)` only when the key was never
stored). `set` SHALL compute `expires_at` from the supplied `ttl: Option<Duration>` and
store it inside the entry (the system SHALL NOT delegate expiration to a native backend
TTL eviction mechanism). The trait SHALL NOT extend `ClaimCheckRepository` or
`IdempotentRepository`. `ContentType` SHALL carry an `exhaustive-by-contract` exception note
(closed 4-variant set; CacheService matches all variants for content_type→Body reconstruction)
per ADR-0049 §Exceptions — it is NOT `#[non_exhaustive]`. `CacheStats` SHALL NOT be
`#[non_exhaustive]` (backends construct it via struct literal — ADR-0049 §Rule 3 governs
structs, not the enum mandate). `CacheEntry` SHALL NOT be `#[non_exhaustive]` (same
struct-literal exception). `CacheEntry.bytes` is `Vec<u8>` (NOT `bytes::Bytes`) because the
workspace `bytes` crate does not enable the `serde` feature; backends convert `Vec<u8>` ↔
`Bytes` at the boundary.

The trait SHALL additionally provide a default async method
`invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError>` that removes every
entry whose key starts with `prefix` and returns the removed count. This is the
"default method" extension path ADR-0056's interface-stability consequence sanctions — the
7 core methods stay untouched. The default implementation SHALL return `Err(CamelError)`
naming the backend (a backend without key-iteration support reports the limitation; it
SHALL NOT return `Ok(0)` pretending an empty namespace). Backends with ordered keys
(`RedbCacheRepository`) SHALL override it with range deletion.

`CacheStats` SHALL carry `hits`, `misses`, `evictions`, `entries` (as before) plus
`peek_stale_served: u64`, `invalidations: u64`, and `bytes: Option<u64>` (value = total
stored payload bytes when the backend can report it; `None` = cannot). `CacheStats` SHALL
derive `Serialize` so the `cache_stats` step can emit it as a JSON body.

#### Scenario: get returns None on miss and Some on hit

- **GIVEN** an empty `CacheRepository` implementation
- **WHEN** `set("k", entry_with_ttl_1h, Some(1h))` then `get("k")` is called
- **THEN** `set` returns `Ok(())` and `get` returns `Ok(Some(entry))`

#### Scenario: get returns None after in-band expiry, peek_stale returns the entry

- **GIVEN** a `CacheRepository` with `set("k", entry, Some(1ms))` and 10ms elapsed
- **WHEN** `get("k")` then `peek_stale("k")` are called
- **THEN** `get` returns `Ok(None)` and `peek_stale` returns `Ok(Some(entry))`

#### Scenario: get surfaces backend failure as Err, never as silent miss

- **GIVEN** a `CacheRepository` whose backing store is unavailable
- **WHEN** `get("k")` is called
- **THEN** the result is `Err(CamelError)` and is NOT `Ok(None)`

#### Scenario: set with None ttl stores entry without expiry

- **GIVEN** an empty `CacheRepository`
- **WHEN** `set("k", entry, None)` then `get("k")` is called after a long elapsed time
- **THEN** `get` returns `Ok(Some(entry))` (no in-band expiry applied)

#### Scenario: invalidate is a no-op on absent key

- **GIVEN** an empty `CacheRepository`
- **WHEN** `invalidate("absent")` is called
- **THEN** it returns `Ok(())`

#### Scenario: stats returns hits/misses/evictions/entries snapshot for tracking backends

- **GIVEN** a `MemoryCacheRepository` or `RedbCacheRepository` (backends that track stats)
  after one hit and one miss
- **WHEN** `stats()` is called
- **THEN** it returns a `CacheStats` whose `hits == 1`, `misses == 1`, and whose
  `evictions`, `entries`, `peek_stale_served`, `invalidations`, and `bytes` fields
  reflect the backend's state (`bytes` is `None` on memory, `Some(total)` on redb)

#### Scenario: non-tracking backend returns default zero stats

- **GIVEN** a `CacheRepository` implementation that cannot cheaply track counters
- **WHEN** `stats()` is called
- **THEN** it returns `CacheStats::default()` (all fields zero, `bytes` `None`) — never `Err`

#### Scenario: invalidate_prefix removes exactly the namespace on ordered backends

- **GIVEN** a `RedbCacheRepository` holding `rainviewer:a`, `rainviewer:b`, `gibs:a`
- **WHEN** `invalidate_prefix("rainviewer:")` is called
- **THEN** it returns `Ok(2)`, `get("rainviewer:a")` and `get("rainviewer:b")` return
  `Ok(None)`, and `get("gibs:a")` still returns `Ok(Some(entry))`

#### Scenario: invalidate_prefix default reports unsupported backends honestly

- **GIVEN** a `CacheRepository` using the default `invalidate_prefix` (no key iteration)
- **WHEN** `invalidate_prefix("ns:")` is called
- **THEN** it returns `Err(CamelError)` naming the backend — NOT `Ok(0)`

### Requirement: Cache stats observability via OTel metrics

The `CacheSegment` (the compiled form of the `cache` DSL step) SHALL emit OpenTelemetry
counters `camel.cache.hits` and `camel.cache.misses` incremented on every cache-step
execution (hit and miss paths respectively) via the `RuntimeObservability::metrics()`
handle already injected into segments (camel-processor CONTEXT.md, ADR-0012). Emission
happens at the SEGMENT (EIP step) level, NOT at the `CacheRepository::get` level — the
trait method is pure storage; the step is the observability boundary. The counters SHALL
be labeled by repository name. The `CacheStats::evictions` and `entries` fields SHALL be
reported by backends that track them (memory/redb); backends that cannot cheaply track
counters SHALL return `CacheStats::default()` (all zero) rather than `Err` (pull-only via
`stats()` for CLI/tooling — eviction/entries OTel gauges are deferred to v1.1).

The `cache_peek_stale` segment SHALL emit `camel.cache.peek_stale_served` (incremented on
the entry-present path, whether the entry is still fresh or stale — both are serves) and
the `cache_invalidate` segment SHALL emit `camel.cache.invalidations` (incremented once
per successful invalidation operation — exact-key or namespace). Both counters SHALL be labeled by repository name and emitted at the
segment level, never on trait methods.

The `camel.cache.invalidations` counter SHALL count successful invalidation OPERATIONS
(+1 per successful exact-key or namespace call) — NOT entries removed. The backend
cannot report whether an exact-key removal deleted an entry (absent-key invalidate is
`Ok(())`), so entry counts are reported only via the `CamelCacheInvalidatedCount`
exchange property (namespace form).

#### Scenario: cache step hit and miss increment OTel counters

- **GIVEN** a route with a `cache:` step bound to repository `"memory"` that already holds
  a fresh entry under key `"k"`, and a test OTel exporter wired
- **WHEN** the route executes once with key `"k"` (hit), then once with key `"absent"` (miss)
- **THEN** the test exporter observes one increment of `camel.cache.hits{repository=memory}`
  and one increment of `camel.cache.misses{repository=memory}`, emitted by the CacheSegment
  (not by the repository trait method)

#### Scenario: peek_stale serve and invalidate increment their OTel counters

- **GIVEN** a route with a `cache_peek_stale:` step and a `cache_invalidate:` step bound
  to repository `"memory"` holding a seeded entry under `"k"`, and a test metrics
  recorder wired
- **WHEN** the peek step serves the entry (hit, fresh or stale) and the invalidate step
  removes key `"k"`
- **THEN** the recorder observes `camel.cache.peek_stale_served{repository=memory}` == 1
  and `camel.cache.invalidations{repository=memory}` == 1

## ADDED Requirements

### Requirement: Cache admin steps — cache_clear and cache_stats

The system SHALL provide two DSL step kinds: `cache_clear` and `cache_stats`. Both SHALL
accept a single optional `repository:` name (default `"memory"`) and SHALL be compiled as
`OutcomeSegment`s following the existing cache-step pattern (unknown repository name
fails at route compile time with `ComponentNotFound` naming the step and repository).

`cache_clear` SHALL call `repository.clear()`. `Err` propagates as `Failed`; success
returns `Completed` with the exchange body unchanged.

`cache_stats` SHALL call `repository.stats()` (synchronous pull) and replace the exchange
body with a JSON object carrying at minimum `repository`, `hits`, `misses`, `evictions`,
`entries`, `peek_stale_served`, `invalidations`, and `bytes` (JSON `null` when the
backend cannot report bytes). `stats()` never returns `Err`, so the step always
completes.

#### Scenario: cache_clear empties the repository

- **GIVEN** a route with a `cache: { repository: memory, key: "k" }` step that has stored
  an entry under `"k"`, followed by a `cache_clear: { repository: memory }` step, and a
  probe consumer
- **WHEN** the route executes a new exchange after the clear
- **THEN** the subsequent `cache` lookup on `"k"` is a miss (the `on_miss` sub-pipeline
  runs) and the clear step completed without altering the clearing exchange's body

#### Scenario: cache_stats emits a JSON snapshot body

- **GIVEN** a repository `"memory"` after operations that produced 2 hits, 1 miss, and
  1 invalidation
- **WHEN** a route step `cache_stats: { repository: memory }` executes
- **THEN** the exchange body is JSON with `"repository": "memory"`, `"hits": 2`,
  `"misses": 1`, `"invalidations": 1`, and a `bytes` field (null or number)

#### Scenario: cache_clear and cache_stats reach canonical parity

- **GIVEN** DSL YAML routes using `cache_clear`/`cache_stats` and equivalent canonical
  `RegisterRoute` commands
- **WHEN** both are compiled
- **THEN** they produce the same `CanonicalStepSpec` variants and the DSL schema accepts
  both step keys

### Requirement: cache_invalidate namespace invalidation via key_prefix

The `cache_invalidate` step SHALL accept `key:` (exact) OR `key_prefix:` (namespace) —
both simple-language expressions. Supplying both or neither SHALL fail at route compile
time with a `Config` error naming the step. On execution:

- Resolved key/prefix is `None` → `Completed` (nothing to invalidate), mirroring the
  exact-key `None` behavior.
- Exact-key path: unchanged — `invalidate(&key)`, then
  `CamelCacheInvalidatedCount = 1` on the exchange (the backend treats an absent key as
  a successful no-op and cannot report absence, so a successful call reports 1).
- Prefix path: `invalidate_prefix(&prefix)`. `Err` (including a backend that does not
  support iteration) → `Failed`. Success → `CamelCacheInvalidatedCount = <returned
  count>` and `camel.cache.invalidations` incremented once (successful operation).

`CamelCacheInvalidatedCount` SHALL be a serde_json number property on the exchange.

#### Scenario: prefix purge removes one namespace only

- **GIVEN** a `RedbCacheRepository` route repository holding `ns:one`, `ns:two`, `other:x`
  and a route step `cache_invalidate: { repository: persistent, key_prefix: "${header.ns}" }`
  with header `ns = "ns:"`
- **WHEN** the route executes
- **THEN** both `ns:*` entries are gone, `other:x` remains, and the exchange property
  `CamelCacheInvalidatedCount` equals 2

#### Scenario: both key and key_prefix rejected at compile time

- **GIVEN** a route step `cache_invalidate: { key: "k", key_prefix: "ns:" }`
- **WHEN** the route compiles
- **THEN** compilation fails with a `Config` error naming `cache_invalidate`

#### Scenario: unsupported backend prefix purge fails closed

- **GIVEN** a `cache_invalidate: { repository: memory, key_prefix: "ns:" }` step (memory
  backend uses the default `invalidate_prefix`)
- **WHEN** the route executes
- **THEN** the step returns `Failed` carrying the backend-naming error — it does NOT
  complete pretending the namespace was purged

### Requirement: Cache singleflight miss coalescing (coalesce_misses)

The `cache` step full form SHALL accept `coalesce_misses: bool` (default `false`). When
enabled, concurrent misses on the same resolved key within one route-step instance SHALL
run the `on_miss` sub-pipeline exactly once:

- The first exchange (leader) runs `on_miss` and performs the single write-back `set`.
- Later exchanges arriving while the leader is in flight (waiters) do NOT run `on_miss`;
  they await the leader's terminal state.
- Leader `Completed` → waiters receive the leader's resulting body on their own exchanges
  and return `Completed` (waiters do not `set`).
- Leader `Failed(e)` → waiters return `Failed(e)` (one upstream attempt per wave — the
  anti-burst property).
- Leader `Stopped` → waiters return `Stopped` for their own exchanges (branch-filter
  semantics).
- The in-flight entry is removed on every leader terminal state; a later miss starts a
  new wave.

The coalescing mechanism SHALL be cancellation-safe and race-free:

- Each in-flight entry carries a terminal-state slot filled BEFORE waiters are woken,
  and woken waiters re-read the slot (no lost wakeup: `notify_waiters` alone wakes only
  currently-registered waiters).
- Waiter registration is atomic with the in-flight map lookup (a waiter either finds a
  terminal slot, registers under the map lock, or becomes the leader).
- A dropped leader future (route shutdown, cancellation) SHALL NOT strand waiters: the
  leader installs a cancellation guard (Drop) that publishes a cancellation terminal
  state (`Failed`) and removes the map entry.

With `coalesce_misses` absent or `false`, behavior is exactly the current per-exchange
execution. The in-flight map SHALL be scoped per compiled route-step (shared across
segment clones) and keyed by resolved cache key. Key-expression `None` exchanges bypass
coalescing entirely (not cacheable, straight to `on_miss` as today).

#### Scenario: concurrent cold-key misses fetch once

- **GIVEN** a route with `cache: { repository: memory, key: "k", coalesce_misses: true,
  on_miss: [ <fetch counting invocations> ] }` on an empty repository, and 3 exchanges
  executed concurrently with key `"k"`
- **WHEN** all 3 complete
- **THEN** the fetch ran exactly once, all 3 exchanges carry the fetched body, and
  `repository.set` was called exactly once

#### Scenario: leader failure fails the wave once

- **GIVEN** the same route with an `on_miss` that returns `Failed`
- **WHEN** 3 exchanges execute concurrently
- **THEN** the fetch ran once and all 3 exchanges return `Failed` with the leader's error

#### Scenario: default off keeps per-exchange misses

- **GIVEN** the same route without `coalesce_misses` on an empty repository
- **WHEN** 3 exchanges execute concurrently
- **THEN** the fetch ran 3 times and 3 `set` calls occurred (current behavior unchanged)

#### Scenario: leader cancellation does not strand waiters

- **GIVEN** the coalescing route with a slow `on_miss`, one leader in flight and one
  waiter registered
- **WHEN** the leader's future is dropped before `on_miss` completes (cancellation)
- **THEN** the waiter terminates with `Failed` (cancellation terminal state) instead of
  hanging, and the in-flight map no longer contains the key
