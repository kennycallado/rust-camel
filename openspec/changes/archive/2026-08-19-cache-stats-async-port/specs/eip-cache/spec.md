## MODIFIED Requirements

### Requirement: CacheRepository port with in-band expiry

The system SHALL provide an object-safe `CacheRepository` trait in `camel-api`,
implemented with `#[async_trait]`, whose implementations are `Send + Sync` and whose
fallible operations return `Result`, and stores
`CacheEntry { bytes: Vec<u8>, content_type: ContentType,
expires_at: Option<SystemTime> }`. The trait SHALL expose `get`, `set`, `peek_stale`,
`invalidate`, `clear`, and a default async `stats` method. `get` SHALL return `Ok(None)` when the
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
"default method" extension path ADR-0056's interface-stability consequence sanctions —
when introduced, this default-method extension left the seven pre-existing methods
unchanged; this change separately amends the `stats` signature. The default
implementation SHALL return `Err(CamelError)`
naming the backend (a backend without key-iteration support reports the limitation; it
SHALL NOT return `Ok(0)` pretending an empty namespace). Backends with ordered keys
(`RedbCacheRepository`) SHALL override it with range deletion.

The `stats` method SHALL be asynchronous: `async fn stats(&self) -> CacheStats` under the
trait's `#[async_trait]`, infallible (no `Result`), with a default body returning
`CacheStats::default()`. A synchronous signature makes it structurally impossible for a
backend to offload I/O-bound byte accounting off the tokio worker (bd rc-22wj), so the port
SHALL NOT reintroduce a synchronous stats surface. This is a pre-1.0 source-breaking
correction to the port recorded as an ADR-0056 amendment; call sites await `stats().await`.

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
- **WHEN** `stats().await` is called
- **THEN** it returns a `CacheStats` whose `hits == 1`, `misses == 1`, and whose
  `evictions`, `entries`, `peek_stale_served`, `invalidations`, and `bytes` fields
  reflect the backend's state (`bytes` is `None` on memory, `Some(total)` on redb)

#### Scenario: non-tracking backend returns default zero stats

- **GIVEN** a `CacheRepository` implementation that cannot cheaply track counters
- **WHEN** `stats().await` is called
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

### Requirement: RedbCacheRepository opt-in persistent backend

The system SHALL provide a `RedbCacheRepository` in `camel-core` that implements
`CacheRepository` by persisting `CacheEntry` values (with their in-band `expires_at`) to a
redb file on disk, surviving process restart. Every operation that performs blocking
redb I/O SHALL use `tokio::task::spawn_blocking`. Fallible operations SHALL map redb
errors to `CamelError::Io`, satisfying Contract C1. For infallible `stats()`, the
payload-sum byte scan SHALL run inside
`spawn_blocking`, never on the tokio worker; scan or join failure SHALL instead produce
`bytes: None` while preserving eagerly maintained counters. A
background sweep task SHALL remove entries whose
`expires_at + stale_retention` has elapsed; the task SHALL bind to the context's
`CancellationToken` so it stops cleanly on shutdown. The constructor SHALL take a
**required** cache size in bytes (`usize`) and SHALL open the database through
`redb::Builder` with `set_cache_size(bytes)` — redb's own default cache size (currently
1GiB) SHALL NOT be reachable through any code path. The repository SHALL record the
configured cache size in a field observable to in-crate tests as the propagation seam.

#### Scenario: entries survive handle drop and reopen

- **GIVEN** a `RedbCacheRepository` opened on file `X` with `set("k", entry, Some(1h))`
- **WHEN** the handle is dropped and a new `RedbCacheRepository` is opened on the same file `X`
- **THEN** `get("k")` on the new handle returns `Ok(Some(entry))`

#### Scenario: peek_stale returns post-expiry entry on redb

- **GIVEN** a `RedbCacheRepository` with `set("k", entry, Some(1ms))` and 10ms elapsed but
  within `stale_retention`
- **WHEN** `peek_stale("k")` is called
- **THEN** it returns `Ok(Some(entry))` (sweep has not yet reclaimed it)

#### Scenario: sweep removes entries past stale_retention

- **GIVEN** a `RedbCacheRepository` whose sweep interval has fired and whose entry `"k"` is
  past `expires_at + stale_retention`
- **WHEN** `peek_stale("k")` is called after sweep
- **THEN** it returns `Ok(None)`

#### Scenario: sweep stops on context shutdown

- **GIVEN** a `RedbCacheRepository` whose sweep task is running and bound to a
  `CancellationToken`
- **WHEN** the token is cancelled
- **THEN** the sweep task exits within a bounded grace period and no sweep task lingers

#### Scenario: redb errors surface as Contract C1 Err

- **GIVEN** a `RedbCacheRepository` whose backing file has been removed beneath it
- **WHEN** `get("k")` is called
- **THEN** the result is `Err(CamelError::Io(..))` and is NOT `Ok(None)`

#### Scenario: configured cache size is observable on the repository

- **GIVEN** a `RedbCacheRepository` constructed with `cache_size = 536870912`
- **WHEN** an in-crate test reads the repository's recorded cache size field
- **THEN** it equals 536870912 (the propagation seam proving the value reached the
  repository that owns the `Builder::set_cache_size` call)

#### Scenario: explicit cache size supports the full round-trip

- **GIVEN** a `RedbCacheRepository` constructed with an explicit cache size
- **WHEN** `set("k", entry, Some(1h))` then `get("k")` are called
- **THEN** the round-trip succeeds on the database opened through the builder

#### Scenario: stats computes bytes off the tokio worker

- **GIVEN** a `RedbCacheRepository` holding entries whose payloads total `N` bytes
- **WHEN** `stats().await` is called
- **THEN** it returns `bytes == Some(N)` (payload-byte sum, unchanged semantics) with the
  byte scan executed inside `spawn_blocking`, and a scan failure yields `bytes == None`
  with all other fields still reported

### Requirement: Cache admin steps — cache_clear and cache_stats

The system SHALL provide two DSL step kinds: `cache_clear` and `cache_stats`. Both SHALL
accept a single optional `repository:` name (default `"memory"`) and SHALL be compiled as
`OutcomeSegment`s following the existing cache-step pattern (unknown repository name
fails at route compile time with `ComponentNotFound` naming the step and repository).

`cache_clear` SHALL call `repository.clear()`. `Err` propagates as `Failed`; success
returns `Completed` with the exchange body unchanged.

`cache_stats` SHALL await `repository.stats()` and replace the exchange
body with a JSON object. The JSON object SHALL contain exactly `repository`, `hits`,
`misses`, `evictions`, `entries`, `peek_stale_served`, `invalidations`, and `bytes`;
`bytes` SHALL be a number or JSON `null` (null when the backend cannot report bytes).
`stats()` never returns `Err`, so the step always
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
  `"misses": 1`, `"invalidations": 1`, and a `bytes` field (null or number), and the
  JSON object contains exactly the key set `repository`, `hits`, `misses`, `evictions`,
  `entries`, `peek_stale_served`, `invalidations`, `bytes` — no additional keys

#### Scenario: cache_clear and cache_stats reach canonical parity

- **GIVEN** DSL YAML routes using `cache_clear`/`cache_stats` and equivalent canonical
  `RegisterRoute` commands
- **WHEN** both are compiled
- **THEN** they produce the same `CanonicalStepSpec` variants and the DSL schema accepts
  both step keys
