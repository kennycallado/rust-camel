# ADR-0065: Cache Payload Offload

**Date:** 2026-08-24
**Status:** Accepted
Cross-references: ADR-0023, ADR-0033, ADR-0056, ADR-0063

## Decision

### Decision 1: decorator over the backend, index in backend, blob on disk

`DiskOffloadRepository` (`crates/camel-core/src/cache/disk_offload.rs:62`)
wraps any `Arc<dyn CacheRepository>`. It is a decorator, not a backend. The
wrapped backend stores a small index entry with an emptied `bytes` field and
a `payload_path`. The payload bytes live in one blob file under `payload_dir`.

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
(`blob_filename`, `crates/camel-core/src/cache/disk_offload.rs:479`). The
content fingerprint hashes the payload bytes followed by the one-byte
discriminant of the `ContentType` enum, which separates the domains
(`content_fingerprint`, `crates/camel-core/src/cache/disk_offload.rs:471`).

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
`crates/camel-core/src/cache/disk_offload.rs:499`): the path must resolve to
a direct child of `payload_dir`. Separators and `..` are rejected, so a
corrupt or foreign row cannot trigger an arbitrary file read.

A missing blob (sweep lag, NFS skew, crash window) returns `Ok(None)` with a
WARN. An I/O failure on an existing blob (EIO, EACCES) returns `Err`, per
Contract C1: a failing disk is a storage failure, not a miss.

### Decision 6: standalone sweeper with ENOENT as success

A standalone tokio task sweeps the payload directory
(`spawn_sweeper`, `crates/camel-core/src/cache/disk_offload.rs:619`). It
unlinks blobs whose encoded death epoch has passed and reclaims stale `.tmp`
files (`sweep_payload_dir`,
`crates/camel-core/src/cache/disk_offload.rs:560`). Unlink of an absent file
counts as success, so concurrent replicas sweep without coordination. The
task stops on the context shutdown token, and `Drop` aborts it
(`crates/camel-core/src/cache/disk_offload.rs:432`). No sweeper exists under
inline mode.

### Decision 7: fail-closed config matrix

Four `cache_repo` fields govern offload: `payload` (`"inline"` default,
`"disk"`), `payload_dir`, `payload_sweep_interval` (default 1h), and
`payload_max_ttl` (default 720h). Validation rejects `payload = "disk"` on
the memory backend, `payload = "disk"` without a non-empty `payload_dir`,
and any payload field set under inline mode or the memory backend
(`crates/camel-config/src/config.rs:1806`,
`crates/camel-config/src/config.rs:1899`). Malformed or zero intervals fail
with an error that names the field. `payload_dir` has no default: the
operator states where the blobs live. `${env:}` strict interpolation applies.

## Rejected alternatives

### Per-backend offload options

Rejected: the same logic would be triplicated across backends that already
diverge on scan and delete.

### Compact binary codec (bincode, base64)

Rejected as a substitute: a codec fixes the JSON x4 bloat of a `Vec<u8>` but
keeps the full dataset in backend RAM. It also does not unlock `replicas >
1` on the redb single-writer lock. Kept as a separate follow-up.

### Trait widening (`keys(prefix)`) and directory-per-prefix layout

Rejected: self-die filenames make eager file deletion unnecessary. Purge is
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

Offloaded entries are unreadable by consumers that do not share
`payload_dir`. Context build emits one startup WARN that names the resolved
directory (`crates/camel-config/src/context_ext.rs:296`).

### Multi-replica on RWX volumes

With the redis backend, the index is shared and the blobs live on one RWX
volume. Concurrent writers to the same key are last-index-wins. The
surviving row references its own blob, and the loser reclaims at its death
epoch.

## Load-bearing citations

| File:line | Element |
|---|---|
| `crates/camel-api/src/cache.rs:18` | `CacheEntry` |
| `crates/camel-api/src/cache.rs:23` | `payload_path: Option<String>`, `#[serde(default)]` |
| `crates/camel-core/src/cache/disk_offload.rs:62` | `DiskOffloadRepository` decorator |
| `crates/camel-core/src/cache/disk_offload.rs:479` | `fn blob_filename` self-die name format |
| `crates/camel-core/src/cache/disk_offload.rs:471` | `fn content_fingerprint` domain-separated blake3-128 |
| `crates/camel-core/src/cache/disk_offload.rs:490` | `fn parse_death_epoch` |
| `crates/camel-core/src/cache/disk_offload.rs:499` | `fn sanitize_blob_name` direct-child guard |
| `crates/camel-core/src/cache/disk_offload.rs:560` | `sweep_payload_dir`: death-epoch unlink, tmp GC |
| `crates/camel-core/src/cache/disk_offload.rs:619` | `spawn_sweeper` standalone task |
| `crates/camel-core/src/cache/disk_offload.rs:432` | `impl Drop` aborts the sweeper |
| `crates/camel-config/src/config.rs:709-730` | the four payload config fields |
| `crates/camel-config/src/config.rs:1806` | memory-backend payload rejection |
| `crates/camel-config/src/config.rs:1899` | disk-mode fail-closed matrix |
| `crates/camel-config/src/context_ext.rs:296-330` | wrap-on-disk wiring and portability WARN |
| `crates/camel-test/tests/cache_payload_offload.rs` | live redis offload integration suite |
