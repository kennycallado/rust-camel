# ADR-0056: Cache Repository Port

**Date:** 2026-08-11
**Status:** Accepted
**References:** ADR-0001, ADR-0023, ADR-0028, ADR-0033, ADR-0046, ADR-0049, ADR-0055

## Decision

### Decision 1: memory-default despite the anchor

The default cache repository is `"memory"`, registered during
`CamelContextBuilder::build()` with `max_capacity = 10_000`. This matches the
Idempotent (ADR-0023) and Claim Check (ADR-0028) precedent: every context gets a
zero-dependency in-process backend out of the box.

The motivating anchor (EFFIS/GIBS geo-layer tiles) needs a persistent backend
(redb). The user opts in via `[default.cache_repo]` in `Camel.toml`:

```toml
[default.cache_repo]
backend = "redb"
path = "data/cache.redb"
stale_retention = "168h"
```

This registers a second repository named `"persistent"` alongside the default
`"memory"`. The DSL step selects the repository by name. The memory default is
unchanged — the anchor case is an opt-in override, not a reason to change the
default.

File: `crates/camel-core/src/context_builder.rs:233-235`.

### Decision 2: in-band `expires_at` everywhere

Every `CacheEntry` carries an `expires_at: Option<SystemTime>` field. The
`CacheRepository::set` method accepts `ttl: Option<Duration>`, computes
`expires_at = SystemTime::now() + ttl`, and stores it in the entry. The
`CacheRepository::get` method checks expiry in-band: if `expires_at <= now`,
the entry is treated as absent (`Ok(None)`).

This design has three consequences:

1. **No native TTL.** Backends (moka, redb, Redis) do not use their own
   time-based eviction. The cache tier is a size-bounded key-value store;
   expiry is a correctness layer above it. This is the prerequisite for
   `peek_stale` (Decision 5).

2. **`SystemTime`, not `Instant`.** `SystemTime` is clock-aware and
   serializable. `Instant` is monotonic but opaque — it cannot be stored in
   redb or compared across process restarts. The clock-skew caveat applies:
   if the system clock jumps backward, entries may live longer than their
   intended TTL. This is acceptable for a cache (not a security boundary).

3. **Serializable.** `CacheEntry` derives `Serialize`/`Deserialize` so
   persistent backends (redb, Redis) can store and restore entries without
   a per-backend serialization layer.

File: `crates/camel-api/src/cache.rs:18-25` (CacheEntry struct),
`crates/camel-api/src/cache.rs:77-82` (set computes expires_at),
`crates/camel-core/src/cache/memory.rs:95-115` (get checks expiry in-band).

### Decision 3: moka size-eviction only (no Expiry, no time_to_live)

The `MemoryCacheRepository` configures moka with `max_capacity` only — no
`expire_after` or `time_to_live`. This is what makes `peek_stale` work on the
memory tier: moka never evicts by time, so an expired entry stays in the cache
until it is evicted by size pressure or explicitly invalidated.

`get()` checks `expires_at` in-band and returns `Ok(None)` for expired entries.
`peek_stale()` returns the raw entry from moka without checking expiry, so
callers can read stale data when the upstream is unavailable.

File: `crates/camel-core/src/cache/memory.rs:50-54` (moka builder — no
time-based eviction), `crates/camel-core/src/cache/memory.rs:95-115` (get
checks expiry), `crates/camel-core/src/cache/memory.rs:128-135` (peek_stale
skips expiry check).

### Decision 4: mandatory `max_capacity` on memory default

`MemoryCacheRepository::new()` requires a `max_capacity` parameter. The default
registration in `CamelContextBuilder::build()` uses `10_000`. This follows
ADR-0033 (security defaults, D-A5: bounded resource consumption) and the
AggregatorConfig precedent: an unbounded default memory cache is a DoS risk.

The `[default.cache_repo]` config can override the capacity via
`max_capacity = 5000` when `backend = "memory"`. If omitted, the default
`10_000` stands.

File: `crates/camel-core/src/context_builder.rs:235` (default 10_000),
`crates/camel-core/src/cache/memory.rs:44` (constructor requires max_capacity).

### Decision 5: retention window != TTL

Persistent backends (redb, Redis) reclaim entries at `expires_at + retention`,
not at `expires_at`. The retention window is a configurable duration (default
`168h` / 7 days) that extends the entry's life in storage after its logical
expiry.

This gives `peek_stale` post-expiry reach on persistent tiers: an entry that
expired 6 hours ago is still readable via `peek_stale` if the retention window
is 168 hours. The entry is invisible to normal `get()` (which checks
`expires_at`), but the bytes remain in storage for the stale-read fallback.

The memory tier does not need a retention window — moka keeps entries until
size-eviction removes them, which is effectively an unbounded retention.

File: `crates/camel-core/src/cache/memory.rs:128-135` (peek_stale on memory —
no retention window needed).

### Decision 6: no `sweep()` on the trait

`CacheRepository` does not expose a `sweep()` method. Reclamation of expired
entries is per-backend:

- **Memory (moka):** no sweep needed. Entries are evicted only by size
  pressure. Expired entries consume space until evicted, which is acceptable
  for an in-process cache bounded by `max_capacity`.
- **Redb:** a background sweep task runs at a configurable interval (default
  `60s`) and deletes entries whose `expires_at + retention < now`.

> Amendment (2026-08-18): the 60s default documented above never shipped — the wiring hardcoded 1h. `sweep_interval` now makes the interval configurable via `[default.cache_repo]`; the default stays 1h because an O(N) sweep over a large persistent cache costs more than delayed reclamation.

- **Redis:** `EXPIRE` / `PEXPIRE` handles reclamation natively (the entry is
  deleted at the Redis server level when the TTL expires).

Adding `sweep()` to the trait would force every backend to implement a method
that is a no-op for memory and Redis. This is contract dishonesty — the trait
should not promise a capability that most backends do not need. The idempotent
and claim check traits set the same precedent: no sweep method.

File: `crates/camel-api/src/cache.rs:68-120` (trait — no sweep method).

## Rejected alternatives

### `cache://` component (ADR-0001 + ADR-0046)

A `cache://` URI scheme would make caching a component endpoint, following the
Apache Camel pattern. Rejected because:

- A component endpoint implies a Consumer or Producer lifecycle (start, stop,
  health). Caching is a pipeline step, not an endpoint — it does not consume
  from or produce to an external system.
- The `cache://` scheme would need to be a pseudo-component (no real endpoint),
  adding complexity to the component registry and URI resolution.
- ADR-0046 establishes Apache Camel as inspiration, not conformance authority.
  The pipeline-step approach is more idiomatic for rust-camel's Tower-native
  architecture (ADR-0001).

### `Body` as stored type (not Serialize, Stream un-cacheable)

Storing `Body` directly in the cache would avoid the `Vec<u8>` + `ContentType`
split. Rejected because:

- `Body` is not `Serialize` — it contains `StreamBody` (an
  `Arc<Mutex<Option<BoxStream>>>`) which cannot be serialized for persistent
  backends.
- `Stream` variants are inherently un-cacheable: a stream can be consumed only
  once. Caching a stream would require materializing it first, which is the
  caller's responsibility (via `StreamCacheService`).
- `CacheEntry` with `Vec<u8>` + `ContentType` is serializable, backend-agnostic,
  and reconstructible into `Body` by the `CacheService` step.

### `on_no_poll` reuse (passthrough, no write-back)

An alternative design would let the cache step pass through the upstream
response on cache miss without writing it back (passthrough mode). Rejected
because:

- The cache step's contract is "check cache first; on miss, fetch and store."
  A passthrough mode that skips the store is a different pattern (memoize with
  expiry, not cache).
- If the caller wants passthrough behavior, they can omit the cache step
  entirely. Adding a mode that does half the job adds API surface without
  compositional benefit.

### Native backend TTL eviction (breaks peek_stale)

Using moka's `time_to_live` or Redis's `EXPIRE` for TTL enforcement would let
the backend handle expiry natively. Rejected because:

- Native TTL eviction removes the entry from storage at expiry time. This
  makes `peek_stale` impossible — there is nothing to peek.
- In-band expiry (Decision 2) gives the trait control over the
  expired-but-readable window, which is the foundation of the stale-read
  fallback pattern.

### Extending ClaimCheckRepository with TTL (breaks payload-ownership contract)

Adding TTL-aware `set`/`get` to `ClaimCheckRepository` would let it serve as
a cache. Rejected because:

- `ClaimCheckRepository` owns payloads (full `Message` with headers). A cache
  stores materialized bytes, not messages. The ownership contract is different:
  Claim Check returns the payload and removes it (`get_and_remove`); Cache
  returns a copy and keeps it.
- The two traits serve different EIPs (Claim Check EIP vs Cache EIP). Merging
  them would couple two independent patterns under one trait, violating the
  single-responsibility precedent set by ADR-0023 and ADR-0028.

## Context

### Problem

Before this ADR, rust-camel had no pluggable cache backend. The Cache EIP
(cache / cache_invalidate / cache_peek_stale) needed a storage abstraction
that supports:

- TTL-based expiry with stale-read fallback.
- Multiple backends (memory, redb, Redis) selectable by name.
- In-band expiry for correctness across backends.
- Size-bounded memory usage (ADR-0033 D-A5).

The Idempotent (ADR-0023) and Claim Check (ADR-0028) repository traits
established the pattern: a trait in `camel-api`, a memory default in
`camel-core`, and a `NamedRegistry<T>` wiring in `CamelContext`. The cache
repository follows the same pattern with the addition of TTL and stale-read
semantics.

### Forces

- **Consistency with existing repository traits.** The cache trait should
  follow the same structural pattern as Idempotent and Claim Check: trait in
  `camel-api`, memory default in `camel-core`, `NamedRegistry` wiring.
- **Stale-read fallback.** The EFFIS anchor case needs `peek_stale` — read an
  expired entry when the upstream is unavailable. This drives the in-band
  expiry design and the rejection of native TTL eviction.
- **Serializability.** Persistent backends need to store entries on disk or
  send them over the wire. `CacheEntry` must be `Serialize`.
- **Bounded memory.** An unbounded memory cache is a DoS risk (ADR-0033 D-A5).
  `max_capacity` is mandatory.
- **No trait bloat.** The trait should not expose backend-specific operations
  (sweep, compaction). Each backend manages its own reclamation.

## Consequences

### Trait location

`CacheRepository` lives in `camel-api` (`crates/camel-api/src/cache.rs:63`).
Any crate can implement it without depending on `camel-core`. Future backends
(Redis in `camel-component-redis`, SQL in `camel-sql`) can implement the trait
remotely.

### Interface stability

The trait has no `#[non_exhaustive]` attribute — adding methods would break
existing implementations. The 7-method interface (`name`, `get`, `set`,
`peek_stale`, `invalidate`, `clear`, `stats`) is considered stable. If a
future backend needs a `len()` or `keys()` method, a separate trait or default
method with `unimplemented!()` can be added. The anticipated `len()`/`keys()`
extension materialized as the default async method `invalidate_prefix` (chosen
over a separate trait — single registry lookup, no downcast); `CacheStats`
grew `peek_stale_served`/`invalidations`/`bytes` (source-breaking for external
struct literals; migrate with `..Default::default()`).

Amendment (bd rc-22wj, pre-1.0): the `stats` method signature was corrected from
sync `fn stats(&self) -> CacheStats` to `async fn stats(&self) -> CacheStats`
(default body unchanged, still infallible). A synchronous signature made it
structurally impossible for `RedbCacheRepository` to offload its payload-sum
byte scan off the tokio worker. Call sites await; no twin sync/async pair was
introduced. Ruled by escalation review (e_gpt) over the rejected twin-method and
redb `stored_bytes()` alternatives.

### Default memory backend

`MemoryCacheRepository` is registered as `"memory"` with `max_capacity = 10_000`
in `CamelContextBuilder::build()`. The `replace_cache_repository` method allows
overriding the default capacity when `[default.cache_repo] backend = "memory"`
supplies a custom `max_capacity`.

### No autowiring

The Cache EIP step explicitly names which repository to use (default:
`"memory"`). No auto-discovery. This matches the Idempotent Consumer precedent
(ADR-0023 §No autowiring).

### `ContentType` is exhaustive-by-contract

`ContentType` is a closed 4-variant enum (`Bytes`, `Text`, `Json`, `Xml`).
It carries the `exhaustive-by-contract` exception note (ADR-0049) because the
`CacheService` step matches all variants for `ContentType → Body`
reconstruction. Adding a variant would require updating the match in
`CacheService`.

File: `crates/camel-api/src/cache.rs:29-41`.

### `CacheStats` is NOT `#[non_exhaustive]`

`CacheStats` is a plain struct — backends construct it with struct literals.
Adding fields is backward-compatible (existing literals still compile with
`..Default::default()`). No `#[non_exhaustive]` attribute.

File: `crates/camel-api/src/cache.rs:46-62`.

## Load-bearing citations

| File:line | Element |
|---|---|
| `camel-api/src/cache.rs:18-25` | `CacheEntry` struct with `expires_at: Option<SystemTime>` |
| `camel-api/src/cache.rs:29-41` | `ContentType` enum (exhaustive-by-contract) |
| `camel-api/src/cache.rs:46-62` | `CacheStats` struct (not non_exhaustive) |
| `camel-api/src/cache.rs:68-120` | `CacheRepository` trait (no sweep, no non_exhaustive) |
| `camel-api/src/cache.rs:77-82` | `set` computes `expires_at` from `ttl` |
| `camel-core/src/cache/memory.rs:44` | `MemoryCacheRepository::new` requires `max_capacity` |
| `camel-core/src/cache/memory.rs:50-54` | moka builder — size-eviction only, no time-based eviction |
| `camel-core/src/cache/memory.rs:95-115` | `get` checks `expires_at` in-band |
| `camel-core/src/cache/memory.rs:128-135` | `peek_stale` skips expiry check |
| `camel-core/src/context_builder.rs:233-235` | Default `"memory"` registration with `max_capacity = 10_000` |
