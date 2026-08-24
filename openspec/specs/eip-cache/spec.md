# eip-cache Specification

## Purpose
TBD - created by archiving change add-cache-repository. Update Purpose after archive.
## Requirements
### Requirement: CacheRepository port with in-band expiry

The system SHALL provide an object-safe `CacheRepository` trait in `camel-api`,
implemented with `#[async_trait]`, whose implementations are `Send + Sync` and whose
fallible operations return `Result`, and stores
`CacheEntry { bytes: Vec<u8>,
payload_path: Option<String>, content_type: ContentType,
expires_at: Option<SystemTime> }`, where `payload_path` is a
`#[serde(default)]` additive field (absent in legacy stored entries deserializes
as `None` — the bytes live inline in `bytes`; offload re-injection semantics are
specified in the *Disk payload offload decorator* requirement). The trait SHALL expose `get`, `set`, `peek_stale`,
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

### Requirement: cache_repo Camel.toml configuration

The system SHALL accept a `cache_repo: Option<CacheRepoConfig>` field on `CamelConfig`,
mirroring the existing `idempotent_repo: Option<IdempotentRepoConfig>` field. The
`CacheRepoConfig` SHALL carry a `backend: "memory" | "redb" | "redis"` discriminator (default
`"memory"`) and backend-specific sub-fields. When `backend = "redb"`, the context builder
SHALL register a `RedbCacheRepository` under the name `"persistent"` in addition to the
default `"memory"`. When `backend = "redis"`, the context builder SHALL register a
`RedisCacheRepository` under the name `"redis"` in addition to the default `"memory"`. When
`backend = "memory"` or `cache_repo` is unset, only `"memory"` is
registered. The configuration SHALL be expressible via the profile section
`[default.cache_repo]` (mirrors `[default.idempotent_repo]`), SHALL carry `path`,
`stale_retention`, and an optional `max_entries` cap (default 1_000_000 entries) for the
redb backend, SHALL carry `max_capacity` for the memory backend, and SHALL fail validation
when `backend = "redb"` and `path` is empty.
For the redb backend it SHALL also carry `cache_size` — a **required** byte-size string
such as `"384MB"` or `"512MiB"` (plain integers mean bytes; decimal suffixes are powers
of 10^3, binary suffixes powers of 2^10; overflowing values are rejected) that bounds the
redb page cache — and optional `sweep_interval`, a humantime string such as `"30m"`
(default `"1h"`, values of zero rejected). Malformed `cache_size`, `sweep_interval`, or
`stale_retention` values SHALL fail validation with an error naming the offending field —
silent fallback to a default is forbidden. A redb-backend config without `cache_size`
SHALL fail validation with an error naming the field and suggesting example values.
The EFFIS anchor case configures persistence with:
`[default.cache_repo] backend = "redb"`, `path = "data/cache.redb"`,
`stale_retention = "168h"`, `cache_size = "256MiB"`.
The redis backend SHALL carry `url` (standalone endpoint), OR the pair
`sentinel_nodes` + `master_name` (sentinel topology), an optional `key_prefix`
(default `"camel:cache"`, restricted to the charset `[A-Za-z0-9:_-]`), and the same
`stale_retention` field as redb. In sentinel mode the redis backend SHALL
additionally carry data-node credentials `password` and `username` (both
optional `Option<String>`; they authenticate the client against the
master/replicas, NOT against the sentinels — sentinel credentials remain
`sentinel_username`/`sentinel_password`) and an optional `db` of type `Option<u16>`
(validated 0..=16383, default 0) selecting the logical database on the
data connection. Validation SHALL reject, with an error naming the
dotted field (`cache_repo.<field>`) and the violated rule: `backend = "redis"` with
both `url` and `sentinel_nodes`; with neither `url` nor `sentinel_nodes`; with an
empty `sentinel_nodes` list or any empty node entry; with `sentinel_nodes` and an
empty `master_name`; with `master_name`, `sentinel_username`, or `sentinel_password`
set but no `sentinel_nodes`; with `password`, `username`, or `db` set but no
`sentinel_nodes` (in `url` mode the password and database selection ride
the URI — userinfo password and `?db=N`; username in the URI is out of
scope); with a `db` outside 0..=16383; with a `url` scheme other than `redis://` or
`rediss://`; with any cluster topology fields (cluster mode is not supported for
repository backends); with a `key_prefix` that is empty or contains characters
outside `[A-Za-z0-9:_-]` (glob metacharacters would break prefix-scoped `clear`);
with a `stale_retention` that fails duration parsing; and, when the idempotent repo
is also `backend = "redis"` on the same effective endpoint and database, with an
effective prefix identical to the idempotent repository's (the effective
endpoint identity SHALL include the sentinel-mode `db`, so db 2 vs db 3 on
the same sentinel topology and prefix do not collide). Fields that do not
apply to the configured `backend` SHALL be rejected at validation with an
error naming `cache_repo.<field>` (fail-closed): `backend = "redis"` rejects
`path`, `cache_size`, `sweep_interval`, `max_entries`, and `max_capacity`;
`backend = "memory"` and `"redb"` reject `url`, `sentinel_nodes`,
`master_name`, `sentinel_username`, `sentinel_password`, `key_prefix`,
`password`, `username`, and `db`.
The cache_repo section SHALL additionally carry payload offload fields: `payload` (an `"inline"` | `"disk"` discriminator, default `"inline"` — today's behavior), and when `payload = "disk"`: `payload_dir` (a **required** path string with no default — the operator consciously chooses a shared or node-local location; `${env:}` strict interpolation applies), plus optional `payload_sweep_interval` (humantime, default `"1h"`, values of zero rejected) and `payload_max_ttl` (humantime, default `"720h"`, values of zero rejected). Malformed values SHALL fail validation with an error naming the offending field. `payload = "disk"` SHALL be valid only over `backend = "redis"` or `backend = "redb"`: `backend = "memory"` rejects `payload = "disk"` (a volatile index cannot own persistent blobs), and both `backend = "memory"` and `payload = "inline"` reject `payload_dir`, `payload_sweep_interval`, and `payload_max_ttl` (fail-closed, naming `cache_repo.<field>`). When `payload = "disk"` the context builder SHALL wrap the backend repository in the disk-offload decorator before registration (see the *Disk payload offload decorator* requirement) and emit a startup WARN naming `payload_dir` (offloaded entries are unreadable by consumers that do not share the directory).
Foreign fields SHALL NOT be required. The `CacheRepoConfig`
`Debug` output SHALL redact credentials: URL userinfo, sentinel
username/password, and the data-node `password` and `username` SHALL NOT
appear in formatted output.

#### Scenario: redb registered when backend = redb

- **GIVEN** a `CamelConfig` whose `cache_repo` field has `backend = "redb"`, a path, a
  retention, a cap, and a cache size
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

#### Scenario: cache_size and sweep_interval reach the redb backend

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redb"`, a path,
  `cache_size = "512MiB"`, and `sweep_interval = "30m"`
- **WHEN** the context is built from that config
- **THEN** the `"persistent"` repository is constructed with a recorded cache size of
  536870912 bytes and a sweep interval of 30 minutes

#### Scenario: missing cache_size on redb backend rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"` with a path and a
  retention but no `cache_size`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.cache_size` and suggesting
  example values, and no repository is constructed

#### Scenario: malformed cache_size rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.cache_size = "thirty"` (unparseable)
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.cache_size`

#### Scenario: overflowing cache_size rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.cache_size = "18446744073709551616B"`
  (2^64 bytes — exceeds `usize` on every supported architecture, with a valid suffix)
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.cache_size`

#### Scenario: malformed sweep_interval rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.sweep_interval = "1x"` (unparseable)
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.sweep_interval`

#### Scenario: zero sweep_interval rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.sweep_interval = "0s"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.sweep_interval` and stating
  that the interval must be positive

#### Scenario: malformed stale_retention rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.stale_retention = "forever-ish"` (unparseable)
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.stale_retention` — the 7-day
  silent fallback no longer applies to malformed values

#### Scenario: sweep_interval defaults to one hour when unset

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redb"`, a path, a
  retention, and a `cache_size`, with no `sweep_interval`
- **WHEN** the context is built from that config
- **THEN** the sweep task runs at a one-hour interval, matching the previously shipped
  hardcoded cadence

#### Scenario: redis registered when backend = redis

- **GIVEN** a `CamelConfig` whose `cache_repo` field has `backend = "redis"` and a
  reachable `url`
- **WHEN** the context is built from that config
- **THEN** a cache repository is resolvable by name `"redis"` and a repository is still
  resolvable by name `"memory"`

#### Scenario: sentinel-selected redis backend registered

- **GIVEN** a `CamelConfig` whose `cache_repo` field has `backend = "redis"`,
  `sentinel_nodes = ["s-a:26379", "s-b:26379"]`, and `master_name = "orders"`
- **WHEN** the context is built from that config against a live sentinel topology
- **THEN** a cache repository is resolvable by name `"redis"`

#### Scenario: sentinel without master_name rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"` and
  `sentinel_nodes` set but no `master_name`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `master_name`

#### Scenario: url and sentinel_nodes together rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`, a `url`, and
  `sentinel_nodes`
- **WHEN** the config is validated
- **THEN** validation returns an error stating the two are mutually exclusive

#### Scenario: no topology rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"` with neither
  `url` nor `sentinel_nodes`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo` and stating that a
  topology (`url` or `sentinel_nodes`) is required

#### Scenario: orphan sentinel master_name rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`, a `url`, and
  a `master_name` but no `sentinel_nodes`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.master_name` and stating
  it requires `sentinel_nodes`

#### Scenario: invalid url scheme rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"` and
  `url = "http://cache.internal:6379"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.url` and stating only
  `redis://` and `rediss://` are accepted

#### Scenario: glob metacharacters in key_prefix rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"` and
  `key_prefix = "camel:*"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.key_prefix` and stating
  the allowed charset

#### Scenario: empty sentinel_nodes list or entry rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"` and
  `sentinel_nodes = []`, or `sentinel_nodes = ["s-a:26379", ""]`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.sentinel_nodes` and stating
  entries must be non-empty

#### Scenario: empty master_name with sentinel_nodes rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`,
  `sentinel_nodes = ["s-a:26379"]`, and `master_name = ""`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.master_name`

#### Scenario: orphan sentinel credentials rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`, a `url`, and
  `sentinel_password = "x"` but no `sentinel_nodes`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.sentinel_password` and
  stating it requires `sentinel_nodes`

#### Scenario: shared-database prefix collision rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` and `idempotent_repo` are both
  `backend = "redis"` on the same `url` and database, both with
  `key_prefix = "camel:shared"`
- **WHEN** the config is validated
- **THEN** validation returns an error stating the effective prefixes must be
  distinct

#### Scenario: credentials redacted from Debug output

- **GIVEN** a `CacheRepoConfig` with `url = "redis://user:secret@host:6379"` and
  `sentinel_password = "hunter2"`
- **WHEN** the struct is formatted with `{:?}`
- **THEN** the output contains neither `secret` nor `hunter2`

#### Scenario: data credentials reach the endpoint in sentinel mode

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`,
  `sentinel_nodes`, `master_name`, `password = "master-secret"`,
  `username = "svc"`, and `db = 2`
- **WHEN** the redis endpoint is constructed from that config
- **THEN** the endpoint carries the data-node username and password and
  database 2 for the master/replica connection, distinct from any sentinel
  credentials

#### Scenario: data credentials rejected in url mode

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`,
  `url = "redis://h:6379"`, and `password = "x"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.password` (credentials
  ride the URI — userinfo password and `?db=N`; username in the URI is
  out of scope)

#### Scenario: db out of range rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`,
  `sentinel_nodes`, `master_name`, and `db = 20000`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.db`

#### Scenario: sentinel db participates in the prefix-collision identity

- **GIVEN** two redis repo configs on the same sentinel topology and master
  with identical effective prefixes but `db = 2` and `db = 3`
- **WHEN** the config is validated
- **THEN** validation succeeds (different logical databases do not collide);
  the same config with both `db = 2` fails with the prefix-collision error

#### Scenario: data credentials redacted from Debug output

- **GIVEN** a `CacheRepoConfig` in sentinel mode with
  `password = "master-secret"` and `username = "svc-user"`
- **WHEN** the struct is formatted with `{:?}`
- **THEN** the output contains neither `master-secret` nor `svc-user`

#### Scenario: disk payload over redis registers the offload-wrapped repository

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redis"` (url mode),
  `payload = "disk"`, and `payload_dir` set to a temp dir
- **WHEN** the context is built and a `set`/`get` round-trip runs against the
  `"redis"` cache repository
- **THEN** the blob file materializes in `payload_dir`, `get` returns the
  original bytes, and the startup logs a portability WARN naming `payload_dir`

#### Scenario: disk payload over redb registers the offload-wrapped repository

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"`, `path`,
  `cache_size` set, `payload = "disk"`, and `payload_dir` set to a temp dir
- **WHEN** the context is built and a `set`/`get` round-trip runs against the
  `"persistent"` cache repository
- **THEN** the blob file materializes in `payload_dir` and `get` returns the
  original bytes

#### Scenario: disk payload without payload_dir rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redis"` and
  `payload = "disk"` with `payload_dir` unset
- **WHEN** the config is validated
- **THEN** validation fails with an error naming `cache_repo.payload_dir`

#### Scenario: memory backend rejects disk payload

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` and
  `payload = "disk"`
- **WHEN** the config is validated
- **THEN** validation fails with an error naming `cache_repo.payload`

#### Scenario: payload offload fields rejected under inline payload

- **GIVEN** a `CamelConfig` whose `cache_repo.payload` is unset (or
  `"inline"`) and any of `payload_dir`, `payload_sweep_interval`,
  `payload_max_ttl` is set
- **WHEN** the config is validated
- **THEN** validation fails with an error naming the offending
  `cache_repo.<field>`

#### Scenario: payload offload fields rejected under memory backend

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` and any of
  `payload_dir`, `payload_sweep_interval`, `payload_max_ttl` is set
- **WHEN** the config is validated
- **THEN** validation fails with an error naming the offending
  `cache_repo.<field>`

#### Scenario: malformed payload value rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.payload = "spool"`
- **WHEN** the config is validated
- **THEN** validation fails with an error naming `cache_repo.payload`

#### Scenario: malformed payload_sweep_interval rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.payload = "disk"` and
  `payload_sweep_interval = "thirty"` (unparseable)
- **WHEN** the config is validated
- **THEN** validation fails with an error naming
  `cache_repo.payload_sweep_interval`

#### Scenario: zero payload_sweep_interval rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.payload = "disk"` and
  `payload_sweep_interval = "0s"`
- **WHEN** the config is validated
- **THEN** validation fails with an error naming
  `cache_repo.payload_sweep_interval`

#### Scenario: malformed payload_max_ttl rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.payload = "disk"` and
  `payload_max_ttl = "forever"` (unparseable)
- **WHEN** the config is validated
- **THEN** validation fails with an error naming `cache_repo.payload_max_ttl`

#### Scenario: zero payload_max_ttl rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.payload = "disk"` and
  `payload_max_ttl = "0s"`
- **WHEN** the config is validated
- **THEN** validation fails with an error naming `cache_repo.payload_max_ttl`

#### Scenario: payload defaults reach the decorator when unset

- **GIVEN** a `CamelConfig` whose `cache_repo.payload = "disk"` with
  `payload_sweep_interval` and `payload_max_ttl` unset
- **WHEN** the context is built
- **THEN** the offload decorator runs with sweep interval 1h and max ttl 720h

#### Scenario: payload_dir resolves env placeholders

- **GIVEN** a `CamelConfig` whose `cache_repo.payload_dir = "${env:CACHE_PAYLOAD_DIR}"`
  and the `CACHE_PAYLOAD_DIR` environment variable set to a temp dir
- **WHEN** the config is interpolated and validated
- **THEN** the decorator uses the resolved path and validation passes

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

### Requirement: Redb cache container memory guardrail

At redb cache repository open, the system SHALL detect the container memory limit from
cgroup v2 (`/sys/fs/cgroup/memory.max`) with cgroup v1
(`/sys/fs/cgroup/memory/memory.limit_in_bytes`) as fallback, treating `"max"`, v1
sentinel values above 16 TiB, and unparseable content as no-limit (falling through
without error). When the effective cache size (the configured `cache_size`; there is no
unset case, since `cache_size` is required) exceeds the detected limit, the system SHALL
emit a single startup warning naming both the effective cache size and the container
limit. When no limit is detectable or the cache size fits within the limit, the system
SHALL stay silent. The guardrail SHALL be diagnostic only: it SHALL never fail
repository construction. Tests SHALL exercise the check through parameterized cgroup
file paths (temporary files) and captured tracing output — never a real cgroup.

#### Scenario: warning when cache size exceeds cgroup limit

- **GIVEN** cgroup files reporting a memory limit of 768MiB and a configured
  `cache_size` of 1GiB
- **WHEN** the limit check runs against those paths
- **THEN** the check reports that 1073741824 exceeds 805306368, and the startup warning
  is emitted with both numbers in the captured tracing output

#### Scenario: cgroup v2 unlimited reports no limit

- **GIVEN** a cgroup v2 `memory.max` file containing `max`
- **WHEN** the limit is read from that path
- **THEN** no limit is reported from v2 and detection falls through to the v1 path

#### Scenario: cgroup v1 sentinel treated as unlimited

- **GIVEN** a cgroup v1 `memory.limit_in_bytes` file containing `9223372036854771712`
- **WHEN** the limit is read from that path
- **THEN** the value is treated as unlimited (no limit reported)

#### Scenario: malformed cgroup content falls through without error

- **GIVEN** a cgroup v2 `memory.max` file containing `not-a-number`
- **WHEN** the limit is read from that path
- **THEN** no limit is reported from v2, no error is raised, and detection falls through
  to the v1 path

#### Scenario: silent when cache size fits

- **GIVEN** a detected memory limit of 768MiB and a configured `cache_size = "256MiB"`
- **WHEN** the limit check runs
- **THEN** no warning is emitted

#### Scenario: missing cgroup files are not an error

- **GIVEN** neither cgroup path exists
- **WHEN** the limit check runs
- **THEN** repository construction proceeds normally with no warning and no error

### Requirement: Redis cache repository backend

The system SHALL provide a `RedisCacheRepository` in the `camel-redis-repo` repository service crate
that implements `camel_api::CacheRepository` over a multiplexed Redis connection owned by
the repository. Values SHALL be `serde_json`-serialized `CacheEntry` blobs, identical in
format to the redb backend. Expiration SHALL stay in-band per ADR-0056: Redis TTL SHALL be
used only for reclamation, applied as a single `SET … EXAT (expires_at + retention)`
command (one atomic write, no separate `EXPIREAT`) when the entry carries `expires_at:
Some(t)`, and no Redis TTL SHALL be set when `expires_at` is `None`.
`get` SHALL return `None` for entries whose in-band `expires_at` has passed and `Err`
on backend failure, never a silent miss (Contract C1). `peek_stale` SHALL return the
entry regardless of in-band expiry. Keys SHALL be namespaced under
`{key_prefix}:{repo-name}:` with default prefix `camel:cache`. The repository name SHALL
be validated at construction with the same rule as `key_prefix`: non-empty, charset
`[A-Za-z0-9:_-]`, no glob metacharacters — the name is part of every SCAN pattern and an
unsafe name would break `clear` scoping. `clear` SHALL delete only
keys under the repository prefix via `SCAN` + `UNLINK` batching and SHALL never issue
`FLUSHDB` or `FLUSHALL`. `invalidate_prefix(prefix)` SHALL override the trait default
(the default fails closed with a no-key-iteration error) and SHALL SCAN+UNLINK keys
matching `{key_prefix}:{repo-name}:{prefix}*`, returning the removed count; the resolved
step prefix SHALL be validated against the same glob-metacharacter charset before being
embedded in the SCAN pattern (untrusted-data trust boundary, ADR-0032 — the step prefix
is a simple-language expression resolved from exchange data). `async fn stats` SHALL
report in-process hit/miss counters with `entries` and
`evictions` always zero (Redis-side eviction and entry counts are not observable
through the repository path; non-tracking backend semantics per the base spec). Sentinel
topologies SHALL resolve the master once at construction (eager connection, fail fast on
an unreachable topology; the component offloads the blocking sentinel resolve internally),
use the single connection owned by the repository, and re-resolve only after a connection
error.

#### Scenario: set and get round-trip through Redis

- **GIVEN** a `RedisCacheRepository` connected to a Redis server
- **WHEN** `set("k", entry, None)` then `get("k")` are awaited
- **THEN** `get` returns an entry equal to the stored one

#### Scenario: get surfaces backend failure as Err, never as silent miss

- **GIVEN** a `RedisCacheRepository` whose underlying executor returns a transient
  connection error for reads
- **WHEN** `get("k")` is awaited
- **THEN** the result is `Err(CamelError::Io(..))` and not `Ok(None)`

#### Scenario: in-band expiry enforced on get, peek_stale still reads

- **GIVEN** a `RedisCacheRepository` holding an entry with `expires_at` in the past,
  still inside the retention window
- **WHEN** `get("k")` and then `peek_stale("k")` are awaited
- **THEN** `get` returns `Ok(None)` and `peek_stale` returns the stale entry

#### Scenario: EXAT applied only when the entry carries expires_at

- **GIVEN** a `RedisCacheRepository` constructed with a deterministic clock
  (`now`) and a fixed `stale_retention`, whose executor records issued commands
- **WHEN** `set("k", entry, Some(ttl))` is awaited, then
  `set("k", entry, None)` is awaited
- **THEN** the first write is a single `SET … EXAT (now + ttl + stale_retention)`
  command and the second is a single `SET` with no expiry option

#### Scenario: set retries once after a lost response (last-writer-wins)

- **GIVEN** a `RedisCacheRepository` whose executor returns one transient error on the
  first `SET`, then succeeds
- **WHEN** `set("k", entry, ttl)` is awaited
- **THEN** the repository refreshes the connection, re-issues the identical `SET`,
  and returns `Ok(())`

#### Scenario: clear deletes only the cache repository prefix

- **GIVEN** a Redis server holding keys `camel:cache:default:a` (this repository) and
  `camel:idem:default:b` (an idempotent repository on the same server)
- **WHEN** the cache repository's `clear()` is awaited
- **THEN** `camel:cache:default:a` no longer exists and `camel:idem:default:b` still
  exists

#### Scenario: invalidate_prefix purges one logical namespace and guards the step prefix

- **GIVEN** a `RedisCacheRepository` holding keys `camel:cache:default:ns:a` and
  `camel:cache:default:other:b`
- **WHEN** `invalidate_prefix("ns:")` is awaited, and separately
  `invalidate_prefix("ns*")` is awaited on another instance
- **THEN** the first call removes only `camel:cache:default:ns:a` and returns `1`,
  leaving `camel:cache:default:other:b` intact; the second call returns
  `Err(CamelError::Config)` before any SCAN is issued (glob metacharacter in the
  resolved step prefix)

#### Scenario: cache repository name with glob metacharacters rejected at construction

#### Scenario: foreign backend field rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo` sets a foreign field for its
  backend (`backend = "redis"` with `path`, or `backend = "memory"`/`"redb"`
  with `url`)
- **WHEN** the config is validated
- **THEN** validation fails with an error naming the foreign field
  (`cache_repo.path` or `cache_repo.url`)

- **GIVEN** a `RedisCacheRepository` construction attempt with repository name
  `"my*cache"` (glob metacharacter in the name, which becomes part of every SCAN
  pattern)
- **WHEN** the constructor runs
- **THEN** it returns `Err(CamelError::Config)` naming the repository name and the
  allowed charset

#### Scenario: stats reports one hit and one miss with zero entries and evictions

- **GIVEN** a `RedisCacheRepository` holding key `"k"` and not holding key `"x"`
- **WHEN** `get("k")` then `get("x")` are awaited, then `stats()` is awaited
- **THEN** the snapshot reports `hits = 1`, `misses = 1`, `entries = 0`, and
  `evictions = 0`

### Requirement: Disk payload offload decorator

The system SHALL provide a `DiskOffloadRepository` in `camel-core` that decorates
a registered `CacheRepository` (memory excluded — see the cache_repo
configuration matrix) and stores entry payloads as blob files under the
configured `payload_dir`, while the decorated backend holds a tiny index entry.
Offload SHALL be transparent to the EIP faces: expiry and stale-while-revalidate
semantics stay owned by the decorated backend (the decorator SHALL NOT
re-evaluate expiry; the single in-band check remains the backend's).

`set` SHALL: (1) derive the effective expiry from the supplied `ttl`
(`now + ttl`; when `ttl` is `None` it SHALL fabricate
`expires_at = now + payload_max_ttl` on the stored entry — no-TTL entries behave
as payload_max_ttl-TTL entries under disk offload); (2) write the blob file
FIRST within `payload_dir` to a UNIQUE temporary name per write attempt
(opened with create-new semantics — never a shared destination-derived
tmp name; on a name collision, retry with a fresh nonce), fsync, then rename
onto the destination filename
`<blake3-128-hex(key)>.<death_epoch_secs>.<blake3-128-hex(payload ∥
content_type)>.blob` where
`death_epoch = effective_expires_at + stale_retention + payload_sweep_interval`
(the grace keeps the blob alive at least as long as any backend sweeper or EXAT
keeps the index row; the trailing fingerprint domain-separates the content:
`blake3-128(payload || u8-discriminant(content_type))` — the one-byte enum
discriminant makes the encoding unambiguous) (3) then store the index entry with `bytes` emptied and
`payload_path` set. When the blob write fails (e.g. ENOSPC, EIO), `set` SHALL
degrade to inline storage — store the unstripped entry, log a WARN, and return
`Ok(())`: cache writes SHALL NOT introduce a new route-failure mode. Errors from
the decorated backend (including the `"cache: max_entries"` capacity contract)
SHALL propagate unchanged; only the decorator's own file-write errors are
contained. Concurrent same-key `set` calls (multi-replica) are safe without locks:
two writers produce the SAME filename only when key, death epoch, payload
bytes, and content_type all hash alike — identical content, in which case the
files are identical and any surviving index row is coherent. Any differing
component yields a distinct filename (up to a negligible 128-bit collision, a
manifestation of which is a complete-but-stale entry or a clean MISS — never
torn bytes), so the surviving index row references its OWN complete blob and
the unreferenced blob is an orphan reclaimed at its own epoch (last index
write wins). Reads observe a complete, coherent entry or a clean MISS — never
cross-writer pairing of one replica's bytes with another's metadata, never torn
bytes.

`get`/`peek_stale` SHALL re-inject: entries carrying `payload_path` SHALL have
the blob loaded and the bytes restored before returning. `payload_path` values
SHALL be sanitized on read — only a direct child of `payload_dir` is acceptable
(separators, `..`, and absolute paths are corrupt rows). A missing blob file
(early sweep, NFS lag, clock skew, foreign reader) or a corrupt index row
(sanitization failure) SHALL return `Ok(None)` with a WARN — NEVER `Err`
(stale-serve resilience takes precedence). An I/O failure on a blob that
EXISTS (EIO, EACCES, EPERM) SHALL surface as `Err` per Contract C1 — a failing
disk is a storage failure, not a miss. Backend failures still surface as
Contract C1 Err as always. Entries without `payload_path` SHALL pass through unchanged,
including entries stored by older inline versions (serde `default` compatibility).

`invalidate` and `invalidate_prefix` SHALL delegate to the decorated backend
only; the returned count SHALL be index-scoped (blobs are reclaimed
asynchronously at their filename-encoded death epoch — after invalidation or
backend-side eviction the blob is an orphan awaiting its epoch). `clear` SHALL
best-effort unlink the `payload_dir` contents (unlink failures SHALL NOT turn
`clear` into `Err`) and then delegate. `stats` and `name` SHALL delegate
unchanged (`CacheStats.bytes` reports the decorated backend's value unchanged;
offloaded entries store an emptied `bytes` field, and redb's accounting sums
entry `bytes` lengths — so each offloaded entry contributes 0, and redis
reports `None`; blob bytes never appear in stats).

#### Scenario: set stores the blob on disk and a bytes-empty index entry

- **GIVEN** a repository decorated with `payload_dir` D holding a 50 KiB entry
  for key `"k"` (ttl 1h, stale_retention 168h, payload_sweep_interval 1h)
- **WHEN** `set("k", entry, Some(1h))` completes and `get("k")` runs
- **THEN** D contains a file named
  `<blake3-128-hex("k")>.<death_epoch>.<blake3-128-hex(payload ∥
  content_type)>.blob`
  with the entry bytes, the decorated backend's index row for `"k"` has empty
  `bytes` and `payload_path` set to that filename, and `get("k")` returns the
  original bytes and content type

#### Scenario: concurrent same-key writers never cross-pair content

- **GIVEN** two `set` calls for the same key with DIFFERENT payloads (or
  content types) completing within the same death-epoch second
- **WHEN** both writes settle and `get("k")` runs
- **THEN** the returned entry's bytes and content type both come from the SAME
  write (the index row references its own fingerprinted blob), the other blob
  remains as an orphan, and it is reclaimed at its own death epoch

#### Scenario: blob write failure degrades to inline storage

- **GIVEN** a decorated repository whose `payload_dir` cannot be written
  (read-only filesystem)
- **WHEN** `set("k", entry, Some(1h))` runs
- **THEN** `set` returns `Ok(())`, the index row stores the FULL entry bytes
  with `payload_path = None`, a WARN is logged, and `get("k")` returns the bytes

#### Scenario: index-alive file-dead read is a MISS with WARN, never Err

- **GIVEN** a set entry whose blob file is removed underneath the index
  (simulating early sweep or NFS lag)
- **WHEN** `get("k")` and `peek_stale("k")` run
- **THEN** both return `Ok(None)` and a WARN is logged — neither returns `Err`

#### Scenario: blob read failure on an existing file surfaces as Err

- **GIVEN** a set entry whose blob file exists but its permissions deny read
- **WHEN** `get("k")` runs
- **THEN** the call returns `Err` (Contract C1: a storage read failure is
  never swallowed as a miss)
#### Scenario: peek_stale re-injects bytes past expiry

- **GIVEN** a set entry with ttl 10ms and 10ms elapsed
- **WHEN** `peek_stale("k")` runs
- **THEN** the post-expiry entry returns with the original bytes re-injected
  from the blob file

#### Scenario: no-TTL entry is capped at payload_max_ttl

- **GIVEN** `payload_max_ttl = 24h` and `set("k", entry, None)`
- **WHEN** the index row and blob filename are inspected
- **THEN** the stored entry carries `expires_at = now + 24h` and the blob's
  death epoch equals that time plus `stale_retention` plus
  `payload_sweep_interval`

#### Scenario: payload_path traversal is rejected without file access

- **GIVEN** an index row (corrupt or foreign) whose `payload_path` is
  `"../../etc/passwd"` or an absolute path
- **WHEN** `get("k")` runs
- **THEN** the call returns `Ok(None)` with a WARN and no file outside
  `payload_dir` is opened

#### Scenario: legacy inline entries pass through

- **GIVEN** an index row stored before this change (JSON without
  `payload_path`, `bytes` populated)
- **WHEN** `get("k")` runs
- **THEN** the entry returns with its stored bytes unchanged

#### Scenario: invalidate delegates and the blob dies at its epoch

- **GIVEN** a set entry under disk offload
- **WHEN** `invalidate("k")` runs, then `get("k")`, then time passes the
  blob's death epoch and the sweeper runs
- **THEN** `get("k")` returns `Ok(None)` immediately, the blob file remains
  until its epoch, and the sweeper unlinks it afterwards

#### Scenario: clear unlinks the payload dir then delegates

- **GIVEN** two set entries under disk offload on a writable local `payload_dir`
- **WHEN** `clear()` runs and the unlinks succeed
- **THEN** the backend's `stats().entries` is 0 and `payload_dir` contains no
  blob files (an unlink failure is swallowed with a WARN — `clear` still
  returns `Ok(())`)

#### Scenario: stats and name delegate to the inner backend

- **GIVEN** a decorated redb-backed repository
- **WHEN** `stats()` and `name()` run
- **THEN** both return the inner backend's values (`name()` = `"persistent"`;
  `CacheStats.bytes` reports the inner backend's value)

### Requirement: Payload sweeper for offloaded blobs

The system SHALL run a standalone sweeper task per offload-decorated repository,
spawned at wiring time on the context's shutdown token (mirroring the redb
sweep loop): it wakes every `payload_sweep_interval`, scans `payload_dir`, and
unlinks (a) blob files whose filename-encoded death epoch has elapsed and
(b) `*.tmp` files older than `payload_sweep_interval` (crash leftovers between
tmp-write and rename). Unlink `ENOENT` SHALL be treated as success
(multi-replica races on shared volumes). The sweeper SHALL stop when the
context shuts down; dropping the repository SHALL abort its handle. No sweeper
SHALL be spawned under inline mode.

#### Scenario: sweeper unlinks dead blobs and tolerates ENOENT

- **GIVEN** a `payload_dir` containing one blob whose death epoch has elapsed
  and one live blob
- **WHEN** the sweeper runs (and a concurrent unlink already removed the dead
  blob)
- **THEN** the dead blob is gone, the live blob remains, and the ENOENT from
  the racing unlink is treated as success (no error surfaced)

#### Scenario: tmp crash leftovers are GC'd by age

- **GIVEN** a `*.tmp` file in `payload_dir` older than
  `payload_sweep_interval`, and one younger
- **WHEN** the sweeper runs
- **THEN** the stale tmp file is unlinked and the young one remains

#### Scenario: sweeper stops on context shutdown

- **GIVEN** a running payload sweeper
- **WHEN** the context shutdown token fires
- **THEN** the sweeper task exits

#### Scenario: no sweeper under inline mode

- **GIVEN** a context built with `payload = "inline"` (or unset)
- **WHEN** the context is built
- **THEN** no payload sweeper task is spawned

