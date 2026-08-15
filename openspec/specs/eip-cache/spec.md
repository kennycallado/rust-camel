# eip-cache Specification

## Purpose
TBD - created by archiving change add-cache-repository. Update Purpose after archive.
## Requirements
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
  `evictions` and `entries` fields reflect the backend's state

#### Scenario: non-tracking backend returns default zero stats

- **GIVEN** a `CacheRepository` implementation that cannot cheaply track counters
- **WHEN** `stats()` is called
- **THEN** it returns `CacheStats::default()` (all fields zero) — never `Err`

### Requirement: MemoryCacheRepository backed by moka with size-eviction only

The system SHALL provide a `MemoryCacheRepository` in `camel-core` that implements
`CacheRepository` using the `moka` crate for TinyLFU size-eviction. The repository
constructor SHALL take `max_capacity: usize` as a required argument (no default in the
constructor — the config layer supplies the default of 10_000 when parsing Camel.toml, per
ADR-0033 safe-defaults + AggregatorConfig::validate() D-A5 precedent). The system SHALL NOT
configure moka with a custom `Expiry` or a `time_to_live` — moka SHALL NOT time-evict
entries. In-band expiration SHALL be enforced by the `MemoryCacheRepository::get`
implementation (returns `Ok(None)` when `expires_at` has elapsed). `peek_stale` SHALL
delegate to moka's `get` so it retrieves entries regardless of in-band expiry until
size-eviction removes them.

#### Scenario: max_capacity bounds the entry count

- **GIVEN** a `MemoryCacheRepository` constructed with `max_capacity = 2`
- **WHEN** `set` is called for three distinct keys `"a"`, `"b"`, `"c"` (all with no expiry)
- **THEN** at most 2 entries are resident (moka TinyLFU evicts the least-frequently-used)

#### Scenario: get honors in-band expiry while peek_stale does not

- **GIVEN** a `MemoryCacheRepository` with `set("k", entry, Some(1ms))` and 10ms elapsed
- **WHEN** `get("k")` then `peek_stale("k")` are called
- **THEN** `get` returns `Ok(None)` (in-band expiry) and `peek_stale` returns
  `Ok(Some(entry))` (moka did not time-evict; only size pressure may evict)

#### Scenario: config layer supplies default max_capacity when omitted

- **GIVEN** a `CacheRepoConfig` parsed from `[default.cache_repo] backend = "memory"` without
  an explicit `max_capacity`
- **WHEN** the config is validated and the `MemoryCacheRepository` is constructed
- **THEN** the repository is constructed with `max_capacity = 10_000` (the documented default)

### Requirement: CacheRepository wiring on CamelContext with memory default

The system SHALL expose `CamelContext::register_cache_repository` and
`CamelContext::cache_repository` methods mirroring the existing
`register_idempotent_repository`/`idempotent_repository` API (ADR-0028 wiring pattern,
verbatim). `CamelContextBuilder::build` SHALL register a `MemoryCacheRepository` under the
name `"memory"` as the default cache repository.

#### Scenario: memory cache registered as default

- **GIVEN** a `CamelContext` built with default configuration
- **WHEN** `cache_repository("memory")` is called
- **THEN** an `Arc<dyn CacheRepository>` is returned whose `name()` is `"memory"`

#### Scenario: custom backend registered alongside memory default

- **GIVEN** a `CamelContext` and a custom `CacheRepository` impl named `"custom"`
- **WHEN** `register_cache_repository("custom", Arc::new(impl))` then
  `cache_repository("custom")` are called
- **THEN** registration returns `Ok(())` and the lookup returns the registered instance

#### Scenario: duplicate registration is rejected

- **GIVEN** a `CamelContext` with `"memory"` already registered
- **WHEN** `register_cache_repository("memory", Arc::new(other))` is called
- **THEN** the result is `Err(RegistryError::AlreadyRegistered)`

### Requirement: RedbCacheRepository opt-in persistent backend

The system SHALL provide a `RedbCacheRepository` in `camel-core` that implements
`CacheRepository` by persisting `CacheEntry` values (with their in-band `expires_at`) to a
redb file on disk, surviving process restart. Every trait operation SHALL wrap blocking
redb I/O in `tokio::task::spawn_blocking` and SHALL map redb errors to `CamelError::Io`,
satisfying Contract C1. A background sweep task SHALL remove entries whose
`expires_at + stale_retention` has elapsed; the task SHALL bind to the context's
`CancellationToken` so it stops cleanly on shutdown.

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

### Requirement: cache_repo Camel.toml configuration

The system SHALL accept a `cache_repo: Option<CacheRepoConfig>` field on `CamelConfig`,
mirroring the existing `idempotent_repo: Option<RedbIdempotentConfig>` field. The
`CacheRepoConfig` SHALL carry a `backend: "memory" | "redb"` discriminator (default
`"memory"`) and backend-specific sub-fields. When `backend = "redb"`, the context builder
SHALL register a `RedbCacheRepository` under the name `"persistent"` in addition to the
default `"memory"`. When `backend = "memory"` or `cache_repo` is unset, only `"memory"` is
registered. The configuration SHALL be expressible via the profile section
`[default.cache_repo]` (mirrors `[default.idempotent_repo]`), SHALL carry `path`,
`stale_retention`, and an optional `max_entries` cap (default 1_000_000 entries) for the
redb backend, SHALL carry `max_capacity` for the memory backend, and SHALL fail validation
when `backend = "redb"` and `path` is empty.
The EFFIS anchor case configures persistence with a one-liner:
`[default.cache_repo] backend = "redb"`, `path = "data/cache.redb"`,
`stale_retention = "168h"`.

#### Scenario: redb registered when backend = redb

- **GIVEN** a `CamelConfig` whose `cache_repo` field has `backend = "redb"`, a path, a
  retention, and a cap
- **WHEN** the context is built from that config
- **THEN** a cache repository is resolvable by name `"persistent"` and a repository is
  still resolvable by name `"memory"`

#### Scenario: redb absent when backend = memory or cache_repo unset

- **GIVEN** a `CamelConfig` with `cache_repo` unset, OR with `backend = "memory"`
- **WHEN** the context is built from that config
- **THEN** no cache repository is resolvable by name `"persistent"` and a repository is
  resolvable by name `"memory"`

#### Scenario: empty redb path rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"` and `cache_repo.path` is empty
- **WHEN** the config is validated
- **THEN** validation returns an error naming the offending field

#### Scenario: memory max_capacity supplied via config

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` and
  `cache_repo.max_capacity = 5000`
- **WHEN** the context is built from that config
- **THEN** the `"memory"` cache repository is constructed with `max_capacity = 5000`

### Requirement: Cache EIP face — cache, cache_invalidate, cache_peek_stale steps

The system SHALL provide three new DSL step kinds: `cache`, `cache_invalidate`, and
`cache_peek_stale`. The `cache` step SHALL accept a `repository:` name (defaulting to
`"memory"`), a `key:` expression, an optional `ttl:` duration, an optional
`max_entry_bytes:` size (default 10 MiB = `DEFAULT_MATERIALIZE_LIMIT`), and an `on_miss:`
sub-pipeline. On hit (entry present and not expired) the step SHALL replace the exchange
body with the cached entry's reconstructed `Body` (reconstructed via `content_type`:
`Bytes → Body::Bytes`, `Text → Body::Text`, `Json → Body::Json`, `Xml → Body::Xml`) and
SHALL NOT run `on_miss`. On miss the step SHALL run the `on_miss` sub-pipeline, then apply
the **write-back materialization policy** to the resulting body:

- **Already-materialized body** (`Body::Bytes`/`Text`/`Json`/`Xml`): the step checks
  `bytes.len() <= max_entry_bytes`. If it fits, the step constructs a `CacheEntry` and
  `set`s it under the key with the supplied `ttl`, then continues with the fresh body. If
  it exceeds the limit, the step passes the original body through **unchanged** (the body
  is still intact), SHALL NOT call `set`, and SHALL log at `debug` level (per ADR-0012 —
  not an error; the oversized entry degrades to uncached).
- **Streaming body** (`Body::Stream`): the step calls `Body::into_bytes(max_entry_bytes)`.
  If materialization succeeds, the step constructs a `CacheEntry` with `content_type =
  ContentType::Bytes` (the materialization yields raw bytes with no higher type info) and
  `set`s it, then continues with the materialized `Body::Bytes` (replacing the consumed
  stream). If materialization fails with `StreamLimitExceeded`, the stream has been
  partially consumed and cannot be re-served — the step SHALL propagate
  `Err(CamelError)` (an oversized stream is a hard error, not a silent passthrough, because
  the consumed stream cannot be recovered).

`cache_invalidate` SHALL accept a `repository:` and `key:` and SHALL remove the key.

`cache_peek_stale` SHALL accept a `repository:`, a `key:`, and an optional `on_miss:`
policy (`"stop"` — the default — or `"continue"`; any other value SHALL be rejected at
route compile time). The step SHALL evaluate the key expression first:

- Key expression resolves to `None`: the step SHALL set `PipelineOutcome::Stopped` for
  the current branch (an anomalous key resolution is fail-closed, not a miss) and SHALL
  emit one `debug`-level log record naming the step and repository.
- `peek_stale` returns `Err`: the step SHALL propagate `Err`.
- Entry present (ignoring expiry): the step SHALL replace the body with the
  reconstructed `Body`, SHALL set the exchange properties `CamelCachePeekHit=true` and
  `CamelCachePeekStale` (true when the entry's `expires_at` has elapsed at evaluation
  time; false when absent or not elapsed), and SHALL continue the pipeline.
- Entry absent (MISS):
  - `on_miss="stop"`: the step SHALL set `CamelCachePeekHit=false` and
    `CamelCachePeekStale=false` on the exchange, SHALL emit one `debug`-level log
    record naming the step and repository (raw keys SHALL NOT be logged — key
    expressions may resolve credential-bearing exchange data), and SHALL set
    `PipelineOutcome::Stopped` for the current branch (the step is used in
    `CircuitBreaker.fallback` where absence means "no stale available" — silently passing
    through would mask the missing fallback).
  - `on_miss="continue"`: the step SHALL set `CamelCachePeekHit=false` and
    `CamelCachePeekStale=false` on the exchange, SHALL leave the body unchanged, and
    SHALL continue the pipeline.

All three steps SHALL use `OutcomeSegment`
(Segment-not-Process per ADR-0023) so that `PipelineOutcome` propagates correctly through
sub-pipelines. If `on_miss` returns `PipelineOutcome::Stopped`, the cache step SHALL
propagate `Stopped` WITHOUT writing back to the repository (no point caching a stopped
branch). If `on_miss` returns `Err`, the cache step SHALL propagate `Err` WITHOUT writing
back. If the repository `get` or `set` returns `Err`, the step SHALL propagate `Err`.

#### Scenario: cache hit short-circuits on_miss

- **GIVEN** a route with `cache: { repository: memory, key: "k", ttl: 1h, on_miss: [ <expensive fetch> ] }`
  and the memory repository already holds a fresh entry under `"k"`
- **WHEN** the route executes one exchange
- **THEN** the body is the cached entry and the `on_miss` sub-pipeline does not run

#### Scenario: cache miss runs on_miss, sets, and continues

- **GIVEN** the same route and an empty repository
- **WHEN** the route executes one exchange
- **THEN** the `on_miss` sub-pipeline runs, the resulting body is `set` under `"k"` with
  the ttl, and the exchange continues downstream with the fresh body

#### Scenario: cache miss with oversized materialized body skips write-back

- **GIVEN** a route with `cache: { ..., max_entry_bytes: 1024, on_miss: [ <produces Body::Bytes of 2 KiB> ] }`
- **WHEN** the route executes on an empty repository
- **THEN** the `on_miss` sub-pipeline runs, the original `Body::Bytes` passes through
  unchanged, no `set` is called, a `debug`-level log record is emitted, and the exchange
  continues with the fresh body

#### Scenario: cache miss with oversized stream propagates Err

- **GIVEN** a route with `cache: { ..., max_entry_bytes: 1024, on_miss: [ <produces Body::Stream exceeding 1 KiB> ] }`
- **WHEN** the route executes on an empty repository and `Body::into_bytes(1024)` returns
  `StreamLimitExceeded`
- **THEN** the cache step propagates `Err(CamelError)` (the consumed stream cannot be
  re-served; oversized streams are a hard error, not a silent passthrough)

#### Scenario: cache on_miss Stopped propagates without write-back

- **GIVEN** a route with `cache: { ..., on_miss: [ <filter that returns Stopped> ] }`
- **WHEN** the route executes on an empty repository and `on_miss` returns `Stopped`
- **THEN** the cache step propagates `PipelineOutcome::Stopped` downstream and `set` is
  NEVER called on the repository

#### Scenario: cache on_miss Err propagates without write-back

- **GIVEN** a route with `cache: { ..., on_miss: [ <step that returns Err> ] }`
- **WHEN** the route executes on an empty repository and `on_miss` returns `Err`
- **THEN** the cache step propagates `Err` downstream and `set` is NEVER called

#### Scenario: cache repository get Err propagates as Err

- **GIVEN** a route with `cache: { repository: custom, ... }` where the `"custom"`
  repository's `get` returns `Err`
- **WHEN** the route executes
- **THEN** the cache step propagates `Err` (Contract C1 — backend failure never silently
  becomes a miss)

#### Scenario: cache repository set Err propagates as Err

- **GIVEN** a route with `cache: { ... }` on an empty repository whose `set` returns `Err`
- **WHEN** the route executes, `on_miss` runs successfully, and the write-back `set` fails
- **THEN** the cache step propagates `Err` (the write-back failure is not silently swallowed)

#### Scenario: cache_peek_stale serves post-expiry entry

- **GIVEN** a repository with `set("k", entry, Some(1ms))`, 10ms elapsed, and a route step
  `cache_peek_stale: { repository: memory, key: "k" }`
- **WHEN** the route executes
- **THEN** the exchange body is the post-expiry cached entry

#### Scenario: cache_peek_stale HIT sets peek properties

- **GIVEN** a repository holding a post-expiry entry under `"k"` and a route step
  `cache_peek_stale: { repository: memory, key: "k" }`
- **WHEN** the route executes
- **THEN** the exchange properties `CamelCachePeekHit=true` and `CamelCachePeekStale=true`
  are set and the body is the stale entry

#### Scenario: cache_peek_stale on absence Stops the branch

- **GIVEN** an empty repository and a route step
  `cache_peek_stale: { repository: memory, key: "absent" }`
- **WHEN** the route executes
- **THEN** the step sets `PipelineOutcome::Stopped` for the current branch (does NOT
  pass through with an unchanged body), `CamelCachePeekHit=false` and
  `CamelCachePeekStale=false` are set on the exchange, and one `debug`-level log record
  naming the step and repository is emitted

#### Scenario: cache_peek_stale on_miss continue passes through on absence

- **GIVEN** an empty repository and a route step
  `cache_peek_stale: { repository: memory, key: "absent", on_miss: continue }` followed
  by a `log` step
- **WHEN** the route executes
- **THEN** the pipeline reaches the `log` step with the body unchanged,
  `CamelCachePeekHit=false` and `CamelCachePeekStale=false` are set, and no
  `PipelineOutcome::Stopped` is returned

#### Scenario: cache_peek_stale on_miss invalid value fails compile

- **GIVEN** a route step `cache_peek_stale: { repository: memory, key: "k", on_miss: skip }`
- **WHEN** the route compiles
- **THEN** compilation fails with an error naming the invalid `on_miss` value

#### Scenario: cache_peek_stale key expression None Stops with debug log

- **GIVEN** a route step `cache_peek_stale: { repository: memory, key: <expression that
  resolves to None> }`
- **WHEN** the route executes
- **THEN** the step sets `PipelineOutcome::Stopped` for the current branch and one
  `debug`-level log record naming the step and repository is emitted

#### Scenario: cache_invalidate removes the key

- **GIVEN** a repository holding `"k"` and a route step
  `cache_invalidate: { repository: memory, key: "k" }` followed by `cache: { ..., key: "k" }`
- **WHEN** the route executes
- **THEN** the second step misses (`on_miss` runs) because the first step removed the entry

### Requirement: Stale-on-error composition with CircuitBreaker

The system SHALL allow users to compose stale-on-error resilience by combining the
route-level `circuit_breaker` configuration (with its `fallback:` sub-pipeline) and the
`cache_peek_stale` step. No feature of the cache SHALL bake stale-on-error into the
`CacheRepository` trait or its backends. The composition SHALL be demonstrable
end-to-end from YAML. A fallback that stops (peek MISS with the default `on_miss: stop`
policy) SHALL surface as a clean outcome, not an error.

#### Scenario: circuitBreaker fallback serves cached stale entry on upstream failure

- **GIVEN** a route of the shape `from: ...` with route-level
  `circuit_breaker: { failure_threshold: 1, open_duration_ms: 60000, fallback: [
  cache_peek_stale: { repository: persistent, key: "tile-xyz" } ] }`, where the route
  body performs the upstream fetch, and a `"persistent"` repository holding a stale
  (past-expiry) entry under `"tile-xyz"`
- **WHEN** the upstream fetch fails enough times that the circuit opens and a further
  exchange arrives
- **THEN** the fallback runs, `cache_peek_stale` returns the post-expiry entry, and the
  exchange body is the stale cached value (instead of an error propagating)

#### Scenario: fallback miss yields a clean outcome

- **GIVEN** the same route shape with an open circuit, but no entry (fresh or stale)
  under `"tile-xyz"`
- **WHEN** the fallback `cache_peek_stale` misses and stops per the default
  `on_miss: stop` policy
- **THEN** the route surfaces `Ok(exchange)` with the Exchange state intact — no
  `CircuitOpen` and no error escapes the circuit breaker fallback path, because the
  composed fallback pipeline translates Stop to `Ok` at its own pipeline boundary
  (ADR-0024/0025)

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

#### Scenario: cache step hit and miss increment OTel counters

- **GIVEN** a route with a `cache:` step bound to repository `"memory"` that already holds
  a fresh entry under key `"k"`, and a test OTel exporter wired
- **WHEN** the route executes once with key `"k"` (hit), then once with key `"absent"` (miss)
- **THEN** the test exporter observes one increment of `camel.cache.hits{repository=memory}`
  and one increment of `camel.cache.misses{repository=memory}`, emitted by the CacheSegment
  (not by the repository trait method)

### Requirement: Cache write-back skips on Stopped and Failed on_miss outcomes

The cache Segment SHALL write back a body ONLY when the `on_miss` sub-pipeline
reports `PipelineOutcome::Completed(exchange)`. When the on_miss reports
`Stopped(exchange)` or `Failed(error)`, the cache SHALL propagate that outcome
as-is and SHALL NOT write any entry to the repository. This prevents poisoning
the cache with an inbound body that a failed on_miss did not legitimately
produce (rc-20yn). This requirement is the cache-side expression of the
segment-outcome-composition zero-success invariant.

#### Scenario: cache skips write-back when on_miss returns Failed

- **GIVEN** a `cache:` Segment with key `k`, a seeded stale entry under `k`, and
  an `on_miss` sub-pipeline that returns `Failed(CamelError)`
- **WHEN** the cache runs on a MISS (the entry's in-band expiry has elapsed)
- **THEN** no `repository.set` call is made for `k`, the Segment returns
  `Failed(error)`, and `cache_peek_stale(k)` afterwards returns the previously
  seeded stale entry (NOT the inbound body, NOT empty)

#### Scenario: cache skips write-back when on_miss returns Stopped

- **GIVEN** a `cache:` Segment with key `k` and an `on_miss` sub-pipeline that
  returns `Stopped(exchange)` (e.g. an inner Stop EIP)
- **WHEN** the cache runs on a MISS
- **THEN** no `repository.set` call is made for `k` and the Segment returns
  `Stopped(exchange)` with the exchange state intact

### Requirement: Stale body survives through do_try catch + cache write-back

When a `cache_peek_stale` step runs inside a `do_try` catch clause that shares a
key with an outer `cache:` step, the stale body retrieved by `cache_peek_stale`
SHALL survive through the do_try `Completed` outcome and any outer cache
write-back boundary. The response SHALL carry the stale body, not an empty body
(rc-65yi).

#### Scenario: stale-serve route returns the stale body, not empty 200

- **GIVEN** a route `cache:{key:k, on_miss:[do_try:{ steps:[recipient_list
  url→broken], catch:[cache_peek_stale:{key:k}] }]}` and a seeded stale body
  under `k`
- **WHEN** the recipient_list fails (broken host) and the catch runs
- **THEN** the response carries the stale body (HTTP 200 with the stale body
  content), NOT an empty 200 and NOT the inbound body

