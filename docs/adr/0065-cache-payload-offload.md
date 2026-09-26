# ADR-0065: Cache Payload Offload

**Date:** 2026-08-24
**Status:** Accepted (amended 2026-09-26: payload location tiering, bd rc-6b88t)
Cross-references: ADR-0023, ADR-0033, ADR-0056, ADR-0063

## Decision

### Decision 1: decorator over the backend, index in backend, blob on disk

`OffloadRepository` (`crates/camel-core/src/cache/offload.rs:101`)
wraps any `Arc<dyn CacheRepository>`. It is a decorator, not a backend. The
wrapped backend stores a small index entry with an emptied `bytes` field and
a `payload_path`. The payload bytes live outside the backend, written
through a pluggable `PayloadStore` (the 2026-08-24 decision knew only the
disk tier; the amendment below adds the redis tier).

The payload travels opaquely through the trait. The decorator intercepts
`set`, `get`, and `peek_stale`, and delegates every other method. One
insertion point serves both persistent backends, per the service-seam
reasoning of ADR-0063. `CacheEntry` gains `payload_path: Option<String>`
(`crates/camel-api/src/cache.rs:23`). The field serializes with
`#[serde(default)]`, so JSON stored by older binaries still deserializes.
Context wiring registers the decorator under the bare backend's name, so
route steps select it unchanged.

### Decision 2: file-first write order with unique per-attempt tmp names

`set` writes the blob before it stores the index entry. The write opens a
unique per-attempt tmp name with `create_new`, calls `sync_all`, then
`rename`s onto the final name within the same directory. Same-directory
rename is atomic on POSIX. The parent-directory fsync is best effort, warns
once, and is ignored on failure. A crash before rename leaves a `.tmp`
orphan. It never leaves an index row that points at missing bytes.

### Decision 3: self-die filenames and the death epoch

Blob names are
`{blake3-128hex(key)}.{death_epoch}.{blake3-128hex(payload || content_type-discriminant)}.blob`
(`blob_filename`, `crates/camel-core/src/cache/offload.rs:437`). The
content fingerprint hashes the payload bytes followed by the one-byte
discriminant of the `ContentType` enum, which separates the domains
(`content_fingerprint`, `crates/camel-core/src/cache/offload.rs:429`).

The `death_epoch` is `effective_expires_at + stale_retention +
payload_sweep_interval` as unix seconds. The sweep-interval grace keeps each
file alive at least as long as any inner sweeper or server-side deadline
keeps the index row. Residual tick lag is a documented MISS+WARN degradation.

Entries written without a TTL get `expires_at = now + payload_max_ttl`
fabricated on the stored entry, so index and file share one death timeline.
The default cap is 720h (30 days).

Two writers that store identical content under the same key produce the same
filename, which is coherent. Any difference in content or death time yields a
distinct filename. The surviving index row references its own blob. No
cross-writer pairing of one writer's bytes with another writer's metadata can
occur. A 128-bit collision would yield a complete-but-stale entry or a miss,
never a torn entry. Orphaned blobs reclaim themselves at their encoded epoch.

### Decision 4: inline fallback on blob-write failure

If the blob write fails (ENOSPC, EIO), the decorator stores the unstripped
entry inline, warns, and returns `Ok(())`. The cache EIP fails the pipeline
on a `set` error, so the decorator must not add a new route-failure mode.
Errors from the wrapped backend still propagate.

### Decision 5: MISS+WARN for a dead file, Err for failing storage (Contract C1)

The read path holds zero expiry logic. The in-band expiry check stays in the
wrapped backend. When an entry carries `payload_path`, the decorator
sanitizes the name (`sanitize_blob_name`,
`crates/camel-core/src/cache/offload.rs:457`): the path must resolve to
a direct child of `payload_dir`. Separators and `..` are rejected, so a
corrupt or foreign row cannot trigger an arbitrary file read.

A missing blob (sweep lag, NFS skew, crash window) returns `Ok(None)` with a
WARN. An I/O failure on an existing blob (EIO, EACCES) returns `Err`, per
Contract C1: a failing disk is a storage failure, not a miss.

### Decision 6: standalone sweeper with ENOENT as success

A standalone tokio task sweeps the payload directory
(`spawn_sweeper`, `crates/camel-core/src/cache/disk_offload.rs:440`). It
unlinks blobs whose encoded death epoch has passed and reclaims stale `.tmp`
files (`sweep_payload_dir`,
`crates/camel-core/src/cache/disk_offload.rs:375`). Unlink of an absent file
counts as success, so concurrent replicas sweep without coordination. The
task stops on the context shutdown token, and `Drop` aborts it
(`crates/camel-core/src/cache/disk_offload.rs:295`; the sweeper is owned by
`DiskPayloadStore`, not by the decorator). No sweeper exists under inline
mode, and none under the redis tier (the amendment below: native `EXAT`
replaces the sweeper there).

### Decision 7: fail-closed config matrix

Four `cache_repo` fields govern offload: `payload` (`"inline"` default,
`"disk"`, `"redis"`), `payload_dir`, `payload_sweep_interval` (default 1h),
and `payload_max_ttl` (default 720h). Validation rejects `payload = "disk"`
on the memory backend, `payload = "disk"` without a non-empty `payload_dir`,
and any payload field set under inline mode or the memory backend
(`crates/camel-config/src/config.rs:2123`,
`crates/camel-config/src/config.rs:2222`). The redis-tier rows, added by the
amendment below (bd rc-6b88t), require `backend = "redis"` for
`payload = "redis"` and reject `payload_dir` on that tier. Malformed or zero
intervals fail with an error that names the field. `payload_dir` has no
default: the operator states where the blobs live. `${env:}` strict
interpolation applies.

## Rejected alternatives

### Per-backend offload options

Rejected: the same logic would be triplicated across backends that already
diverge on scan and delete.

### Compact binary codec (bincode, base64)

Rejected as a substitute: a codec fixes the JSON x4 bloat of a `Vec<u8>` but
keeps the full dataset in backend RAM. It also does not unlock `replicas >
1` on the redb single-writer lock. Kept as a separate follow-up.

### Trait widening (`keys(prefix)`) and directory-per-prefix layout

Rejected: self-die filenames make eager file deletion unnecessary (superseded
for hot keys by the amendment below — bd rc-uteoa). Purge is
index purge plus asynchronous reclaim.

### `payload_min_size` threshold

Rejected: every payload in the target workload is about 50 KB. Additive
later if a small-entry consumer appears.

## Context

### Problem

The cache backs the tile proxy for emergency-services WMS, WMTS, and radar
delivery (7 sources, about 8k tiles per source). It is a resilience asset:
when an upstream fails, the stale tile is served. The hard requirement is
having the tile, not latency. No backend satisfies `replicas > 1` with a
full dataset and bounded RAM:

- redb is embedded and takes a single-writer file lock. It needs a
  read-write-once volume, and no RWX or NFS sharing. Each replica re-warms
  its own copy.
- redis is shared, but the whole dataset lives in RAM. `CacheEntry`
  serializes as JSON, so a `Vec<u8>` payload becomes an integer array. A
  50 KB tile occupies about 200 KB of redis RAM.
- memory is volatile.

A small, shared, durable index plus payload files on cheap storage resolves
the tension.

### Forces

- **Operator-owned placement.** Routes, EIPs, and backend choice stay
  untouched. The operator only decides where the blob lives.
- **Fail-closed culture.** The config matrix validates at startup
  (ADR-0033).
- **Unknown-outcome honesty.** Contract C1 (ADR-0023) governs the read
  paths: a dead file is a miss, a failing disk is an error.
- **No new failure mode.** A full disk must degrade the cache, not break
  the route.

## Consequences

### Rollback requires a cache clear

A rollback to a binary without offload reads an offloaded index row and
serves empty bytes. Clear or re-seed the cache across a rollback.

### NFS caveats

A local volume is preferred. On NFS, fsync durability is mount-dependent,
and rename plus `create_new` semantics can leave short-lived `.tmp` files
under load. The sweeper reclaims them.

### Stats report index-side accounting only

`stats` delegates to the wrapped backend. Offloaded entries contribute an
emptied `bytes` field: redb sums entry bytes, so each offloaded entry
contributes 0, and redis reports `None` for the sum. Blob bytes never
appear.

### Portability

Disk-tier entries are unreadable by consumers that do not share
`payload_dir`. Context build emits one startup WARN that names the resolved
directory (`crates/camel-config/src/context_ext.rs:388`). The redis tier has
no such WARN: it is shared by construction (amendment below).

### Multi-replica on RWX volumes

With the redis backend, the index is shared and the blobs live on one RWX
volume. Concurrent writers to the same key are last-index-wins. The
surviving row references its own blob. A losing writer's blob is a stray
that only the sweeper reclaims, at its death epoch: the next overwrite
reads the current row and cannot discover it (see the amendment below).

### Amendment (bd rc-uteoa): eager predecessor reclaim on overwrite

Field report bd rc-uteoa (camel-cache demo team) showed that the claim
"self-die filenames make eager file deletion unnecessary" (Rejected
alternatives, trait-widening bullet) held only under cold-key assumptions. A
hot-overwritten key on NFS accumulates one grace-window blob per write. The
sweeper `readdir` cost then grows with the directory.

Decision: `set()` reclaims the predecessor blob eagerly, inside the same
call. The reclaim is row-guided. Before the write, `set()` reads the current
index row and records its `payload_path`. After the decorated backend
accepts the overwrite, `set()` unlinks that one file. No directory scan
runs: an NFS `readdir` costs one metadata round-trip per directory entry,
and a scan would hurt most when the directory is largest. The row-guided
unlink costs one index GET plus one unlink, whatever the directory size.

The bound holds for keys written by one writer at a time: after the
overwrite, one blob remains when the unlink succeeds or the predecessor is
already gone (ENOENT). A failed unlink leaves the predecessor on disk until
its death epoch; the sweeper then reclaims it. Under concurrent same-key
writes, a losing writer's blob is a stray beyond the row's reach. Only the
sweeper reclaims it, at its death epoch.

Ordering and guards:

- The unlink runs only after the inner `set` returns `Ok`. On an inner
  error the row may still reference the predecessor, so the reclaim is
  skipped.
- A failed pre-swap row read (`get` returns `Err`) WARNs and skips the
  reclaim. The write proceeds unchanged. The reclaim never adds a failure
  mode.
- The unlink also runs when the blob write degrades to inline storage. The
  new row no longer references the predecessor. The equal-name guard below
  does not apply on this path: the failed write left no fresh file owning
  that name.
- The target name must pass `sanitize_blob_name` and carry a parseable
  death epoch. It must also start with the current key's blake3-128
  filename prefix: a corrupt row naming another key's blob never causes
  that blob's deletion. Foreign files are never unlinked. This tightens
  the sweeper's discipline.
- A same-second identical rewrite produces the same file name. On the
  successful-blob path, the guard `old_name != dest_name` keeps the fresh
  blob alive.
- The unlink is best-effort. A failure WARNs and never fails the write. The
  filename-encoded death epoch and the sweeper stay the backstop for every
  stray: crash-window orphans, losing concurrent writers' blobs, and
  unlink failures.

The reader race contract is unchanged (Decision 5). A reader that holds a
pre-swap row while the writer reclaims hydrates to `Ok(None)` with WARN.
Eager reclaim only widens that documented window for the overwritten key.
It never turns a read into an `Err`.

Cost: each `set()` issues one extra index GET, including first writes where
the GET returns a miss. On redis this is one sub-millisecond round-trip.
Durability: the eager unlink is not directory-fsynced. A crash in that
window leaves a stray that the sweeper collects at its epoch.

### Retention and TTL changes apply to future writes only

Every blob's death epoch is computed at `set()` time with the retention
values in force at that moment, and the sweeper deletes by the filename
epoch alone — runtime config never takes part in deletion. Changing
`stale_retention`, `payload_max_ttl`, or `payload_sweep_interval` therefore
never applies retroactively:

- **Lowering `stale_retention`** gives no immediate disk relief. Existing
  blobs die at their baked epoch, so the orphan window after a purge grows
  temporarily. Relief comes only from `clear()` or from waiting out the old
  cycle.
- **Raising `stale_retention`** opens a redb-only transient window where a
  row outlives its blob: the row is reclaimed at
  `expires_at + new_retention` (redb recomputes with the runtime value),
  while the blob dies at its baked epoch. Reads in between are a MISS with
  WARN. The window is nominally bounded by
  `new_retention − old_retention − payload_sweep_interval` (the grace baked
  into the blob), plus up to one redb sweep tick before the row goes. It
  closes by itself when the redb sweep removes the row — cold keys heal
  without churn. Pair a large raise with `clear()` when stale-serve continuity
  matters during the transition. Redis has no such window: the key expires
  at the immutable EXAT set at write time, which is the same death line the
  blob already encodes.
- **`payload_max_ttl`** changes affect only the expiry fabricated for
  future `ttl = None` writes; existing blobs keep their baked epochs.
- **`payload_sweep_interval`** changes affect only the grace of future
  writes plus the runtime sweep cadence and `.tmp` GC age.
- **Mixed-config replicas** sharing one `payload_dir` are safe: the
  sweeper is filename-driven, so different retention settings only shift
  windows, never correctness.

The sweeper logs one INFO line per pass with live and reclaimed blob
counts and bytes, so operators can watch the volume during any of these
transitions.

## Amendment (bd rc-6b88t): payload location tiering

A demo-team report on NFS-backed `payload_dir` deployments restated the
rc-uteoa finding at the storage layer. The disk tier couples payload
durability to mount semantics: fsync durability is mount-dependent, rename
plus `create_new` leaves short-lived `.tmp` orphans under load, the sweeper
`readdir` cost grows with the directory, and `payload_dir` portability
needs the startup WARN plus an RWX volume shared by every replica. The
index already has a shared, self-expiring home in redis (ADR-0063); the
payloads can ride the same keyspace.

Decision: the decorator no longer owns payload storage. A `PayloadStore`
trait (`crates/camel-core/src/cache/offload.rs:55`) carries `put`, `read`,
and `unlink`, with `clear` defaulting to a no-op
(`crates/camel-core/src/cache/offload.rs:90`). The decorator is renamed
`OffloadRepository` (`crates/camel-core/src/cache/offload.rs:101`); its
interception, blob naming, death-epoch math, eager predecessor reclaim,
and read contract are unchanged. `disk_offload.rs` shrinks to
`DiskPayloadStore` (`crates/camel-core/src/cache/disk_offload.rs:54`):
tmp+fsync+rename writes, reads, unlinks, the eager payload-dir `clear`,
and the standalone sweeper with its `Drop` abort. The decorator no longer
spawns or aborts anything. The redis repository service crate implements
the trait as `RedisPayloadStore`
(`crates/services/camel-redis-repo/src/payload_store.rs:30`); the
dependency direction is preserved, since core never sees redis.

Redis tier contract:

- Payload keys live inside the repository namespace:
  `{prefix}:{repo}:payload:{blob-name}` (`payload_key`,
  `crates/services/camel-redis-repo/src/payload_store.rs:59`). The
  `clear()` prefix-scoped `SCAN` plus `UNLINK` therefore reclaims payloads
  eagerly, and the namespace and charset guards apply without a second
  token shape.
- The `payload:` segment is reserved on this backend (rc-6b88t fix wave).
  `RedisCacheRepository::set_entry`
  (`crates/services/camel-redis-repo/src/cache_repo.rs:238`) rejects user
  keys starting with it as `CamelError::Config`, and
  `RedisCacheRepository::invalidate_prefix`
  (`crates/services/camel-redis-repo/src/cache_repo.rs:521`) skips scanned
  `{prefix}:{repo}:payload:` keys from its UNLINK batches and returned
  count. Without the reservation, a user prefix like `p` globs every
  payload key ("payload:" itself starts with "p") and the sweep would
  delete payload blobs of keys outside the requested prefix while
  inflating the reported count. Payload reclaim keeps its three
  legitimate channels: the native `EXAT`, the eager predecessor overwrite
  reclaim, and `clear()`'s full-namespace SCAN.
- `put` issues `SET key bytes EXAT death_epoch_secs`. The epoch conversion
  is checked, and an unusable death epoch (overflow, pre-epoch) fails
  `put` before any command is issued. The inline fallback of Decision 4
  then applies: a payload blob is never stored without its deadline.
- `read` issues `GET`. A nil reply is `None`, which the decorator reports
  as MISS+WARN (Decision 5). A transport failure is `Err` (Contract C1).
- Reclamation is native. Every payload entry carries its `EXAT` death
  epoch, which replaces the disk sweeper: no sweep loop exists on this
  tier. Overwrite reclaim uses the same row-guided `UNLINK` as the disk
  tier (rc-uteoa amendment), and `clear()` scavenges through the SCAN
  above.
- `RedisCacheRepository::connect_with_payload_store`
  (`crates/services/camel-redis-repo/src/cache_repo.rs:157`) builds the
  index and the payload store over ONE multiplexed connection, per
  ADR-0063. `ComponentMetrics` gained `#[derive(Clone)]` so one metrics
  facade serves both halves.

Fail-closed matrix additions (`crates/camel-config/src/config.rs:2222`):

- `payload = "redis"` requires `backend = "redis"`; the error names the
  configured backend.
- `payload = "redis"` with `payload_dir` set is rejected; the message
  names `payload_dir` as inconsistent with the redis tier.
- The memory backend now rejects every offload tier, not only `"disk"`.
- The duration knobs stay required and validated (parse plus the >= 1s
  check) on BOTH offload tiers, because the decorator consumes them for
  the death-epoch and EXAT math on every tier. Inline mode still rejects
  them.

Wiring: `wrap_payload_offload`
(`crates/camel-config/src/context_ext.rs:365`) generalizes the disk-only
wrap. The redis branch builds the store only there, through
`connect_with_payload_store`
(`crates/camel-config/src/context_ext.rs:624`). The redb branch keeps
disk-only wrapping. The decorator still registers under the bare
backend's name. The redis tier emits no startup WARN, because redis is
shared by construction; the disk-dir portability WARN stays disk-only.

The reader contract of Decision 5 is per tier, not per medium: a vanished
payload (expired `EXAT`, evicted entry, disk sweep lag) is a MISS with
WARN on every tier, and a failing store is an `Err`.

## Load-bearing citations

| File:line | Element |
|---|---|
| `crates/camel-api/src/cache.rs:18` | `CacheEntry` |
| `crates/camel-api/src/cache.rs:23` | `payload_path: Option<String>`, `#[serde(default)]` |
| `crates/camel-core/src/cache/offload.rs:55` | `PayloadStore` trait (`put`/`read`/`unlink`, default-noop `clear`) |
| `crates/camel-core/src/cache/offload.rs:101` | `OffloadRepository` decorator |
| `crates/camel-core/src/cache/offload.rs:437` | `fn blob_filename` self-die name format |
| `crates/camel-core/src/cache/offload.rs:429` | `fn content_fingerprint` domain-separated blake3-128 |
| `crates/camel-core/src/cache/offload.rs:448` | `fn parse_death_epoch` |
| `crates/camel-core/src/cache/offload.rs:457` | `fn sanitize_blob_name` direct-child guard |
| `crates/camel-core/src/cache/disk_offload.rs:54` | `DiskPayloadStore` (disk-tier store; owns sweeper + `Drop` abort) |
| `crates/camel-core/src/cache/disk_offload.rs:295` | `impl Drop` aborts the sweeper |
| `crates/camel-core/src/cache/disk_offload.rs:375` | `sweep_payload_dir`: death-epoch unlink, tmp GC |
| `crates/camel-core/src/cache/disk_offload.rs:440` | `spawn_sweeper` standalone task |
| `crates/services/camel-redis-repo/src/payload_store.rs:30` | `RedisPayloadStore` (redis-tier store) |
| `crates/services/camel-redis-repo/src/payload_store.rs:59` | `fn payload_key` `{prefix}:{repo}:payload:` namespacing |
| `crates/services/camel-redis-repo/src/cache_repo.rs:157` | `connect_with_payload_store`: index + store, one connection |
| `crates/services/camel-redis-repo/src/cache_repo.rs:36` | `const RESERVED_PAYLOAD_SEGMENT`: `set` rejection + `invalidate_prefix` payload-key skip |
| `crates/camel-config/src/config.rs:821` | `PayloadMode` (`inline`/`disk`/`redis`) |
| `crates/camel-config/src/config.rs:871-897` | the four payload config fields |
| `crates/camel-config/src/config.rs:2123` | memory-backend payload rejection |
| `crates/camel-config/src/config.rs:2222` | fail-closed matrix, persistent backends (disk + redis rows) |
| `crates/camel-config/src/context_ext.rs:365` | `wrap_payload_offload` wiring (disk WARN at :388, redis branch at :405) |
| `crates/camel-config/src/context_ext.rs:624` | redis-branch store construction |
| `crates/camel-test/tests/cache_payload_offload.rs` | live offload integration suite (disk + redis tiers) |
