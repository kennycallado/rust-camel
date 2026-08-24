## ADDED Requirements

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

## MODIFIED Requirements

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
