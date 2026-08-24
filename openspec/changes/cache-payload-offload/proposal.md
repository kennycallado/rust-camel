# Proposal: cache-payload-offload

## Why

The cache is a resilience asset for the `camel-cache` tile proxy (emergency-services
WMS/WMTS/radar, 7 sources, ~8k tiles per source): when an upstream fails, the stale
tile is served. The hard requirement is *having the tile*, not latency. Today no
backend satisfies `replicas > 1` with a full dataset and bounded RAM:

- **redb**: embedded, single-writer file lock — RWO volumes only, no RWX/NFS
  sharing; each replica re-warms its own copy.
- **redis**: shared, but the whole dataset lives in RAM (`maxmemory` >= working
  set). `CacheEntry` serializes as JSON, so a `Vec<u8>` payload becomes an integer
  array — a 50 KB tile occupies ~200 KB of redis RAM (bloat x4).
- **memory**: volatile, irrelevant here.

The index-in-backend + blob-on-disk pattern resolves the tension: a tiny, shared,
durable index entry (redis or redb) plus payload files on cheap storage (RWX volume
for multi-replica, local volume for single instance). EIPs (`cache`,
`cache_peek_stale`), routes, and backend choice stay untouched — the operator only
decides *where the blob lives*.

## What Changes

- `CacheEntry` gains `#[serde(default)] pub payload_path: Option<String>`
  (serde-compatible with stored entries; `camel-auth`'s local struct is unaffected;
  ~35 struct literals updated mechanically).
- New `DiskOffloadRepository` decorator in `camel-core` over
  `Arc<dyn CacheRepository>`: `set` writes the blob file-first
  (tmp+fsync+rename) then stores a tiny index entry; `get`/`peek_stale` re-inject
  bytes; `invalidate`/`invalidate_prefix` delegate (count is index-scoped);
  `clear` best-effort unlinks the payload dir then delegates; `stats`/`name`
  delegate. Blob filenames self-encode death and content identity:
  `<blake3-128hex(key)>.<death_epoch>.<blake3-128hex(payload ∥ content_type)>.blob`
  written via unique per-attempt tmp files (create-new, then rename)
  where death = `expires_at + stale_retention + payload_sweep_interval` (grace);
  `ttl = None` entries get `expires_at = now + payload_max_ttl` fabricated on the
  stored entry. Index-alive/file-dead reads are a MISS with WARN, never `Err` —
  stale-serve stays protected. Blob-write failure degrades to inline storage
  (set is never a new route-failure mode). `payload_path` values are sanitized on
  read (direct child of the payload dir only).
- Standalone sweeper task (context shutdown token, Drop-abort): unlinks dead
  blobs (ENOENT = success), GCs stale `*.tmp` files.
- Config (`camel-config`): `payload` (`"inline"` default | `"disk"`),
  `payload_dir` (required when disk, no default), `payload_sweep_interval`
  (default 1h), `payload_max_ttl` (default 30d) — fail-closed matrix: disk only
  over `redis`/`redb`; `memory` + disk rejected; payload fields rejected under
  `inline` and under `memory`. `${env:}` strict interpolation applies.
- Docs: `docs/src/configuration/schema.md` rows, ADR for the offload mode,
  operational notes (NFS caveats, rollback = clear/seed cache, portability WARN).

Excluded: binary codec for inline payloads (bincode/base64 — separate change,
both backends are JSON today), `payload_min_size` (YAGNI for ~50 KB+ tiles),
trait widening (`keys(prefix)`), directory-per-prefix layouts.

Bd: rc-e7qb. Affected crates: `camel-api`, `camel-core`, `camel-config`,
`camel-processor` (literals), `camel-test` (integration), docs.

## Acceptance criteria

- `payload = "disk"` over redis or redb: set/get/peek_stale round-trip with the
  blob on disk and a bytes-empty index entry; expiry and SWR semantics identical
  to inline.
- Blob-write failure (ENOSPC/EIO) degrades to inline storage; `set` returns
  `Ok(())`; cache EIP behavior is never worse than today.
- Index-alive/file-dead reads (missing/swept/corrupt path) return `Ok(None)` +
  WARN; I/O failures on existing blobs surface as `Err` (Contract C1).
- Orphan blobs (crash windows, re-set same key, inner eviction) are reclaimed at
  their filename-encoded death epoch; `*.tmp` leftovers GC'd; sweeper stops on
  context shutdown; no sweeper under inline.
- Config matrix rejects: memory+disk, payload fields under inline, disk without
  `payload_dir`, malformed/zero intervals; defaults 1h/30d apply.
- Full gate suite green (fmt, clippy -D warnings, xtask lints, lib tests).

## Risk budget

Acceptable: additive public field on `CacheEntry` (~35 literal migration,
pre-1.0), one-line new dep (`blake3` on `camel-core`, already in workspace).
Out of bounds: any new route-failure mode from cache writes, changes to EIP
semantics or the `CacheRepository` trait shape, stored-format breakage for
existing inline entries (must deserialize unchanged; rollback requires cache
clear — documented, not encoded).
