# Design: cache-payload-offload

## Approach

A decorator, not a backend. `DiskOffloadRepository` wraps any
`Arc<dyn CacheRepository>` (memory excluded by config validation) and owns a
`payload_dir`. The payload travels opaquely through the trait today, so the
decorator intercepts `set`/`get`/`peek_stale` and delegates everything else —
one insertion point, zero per-repo duplication (per ADR-0063's service-seam
reasoning).

**Write path (`set`)** — file-first, index-second:
1. Compute `effective_expires_at = now + ttl` (the EIP passes `expires_at: None`
   and both impls overwrite it internally — the decorator derives it from `ttl`
   itself; `ttl = None` means `now + payload_max_ttl`, fabricated on the stored
   entry so index and file share one death timeline).
2. Write to a unique per-attempt tmp name (create-new; no shared
tmp path), fsync, rename onto
`<blake3-128hex(key)>.<death_epoch>.<blake3-128hex(payload ∥ content_type)>.blob`
(blake3 prod dep on camel-core; fingerprint domain-separated by the content_type
enum discriminant — no tempfile/uuid prod deps needed) where
   `death_epoch = effective_expires_at + stale_retention + payload_sweep_interval`
   (grace keeps files alive at least as long as any inner sweeper/EXAT keeps the
   index row; residual tick-lag is a documented MISS+WARN degradation). Write is
   tmp+fsync+rename within the dir (same-dir rename = atomic on POSIX, no EXDEV;
   best-effort parent-dir fsync, warn-once, ignored on failure).
3. Store the index entry in `inner`: `bytes` emptied, `payload_path` set.
   If the blob write fails (ENOSPC/EIO): degrade to inline — store the unstripped
   entry, WARN, return `Ok(())`. `cache_eip.rs` `write_back` fails the pipeline
   on `set` `Err`, so the decorator must not introduce a new failure mode; inner
   errors (including the `"cache: max_entries"` contract) still propagate.
   Concurrent same-key writers: unique tmp names isolate writers during
   rename; same final filename only for identical content (coherent); any
   difference yields a distinct filename so the surviving index row references
   its own blob — no cross-writer bytes/metadata pairing (modulo negligible
   128-bit collisions: complete-but-stale entry or miss, never torn), orphans
   reclaimed at their epochs.

**Read path (`get`/`peek_stale`)** — zero expiry logic in the decorator; the
single in-band check stays in `inner` exactly as today. If the entry carries
`payload_path`, sanitize it (must resolve to a direct child of `payload_dir` —
reject separators/`..`; corrupt or foreign rows cannot trigger arbitrary file
reads), load the file, re-inject bytes. File missing (sweep/NFS lag/skew) →
`Ok(None)` + WARN; I/O failure on an existing blob (EIO/EACCES) → `Err` per
Contract C1 — a failing disk is a storage failure, not a miss.
Entries without `payload_path` (legacy rows, inline fallback) pass through
unchanged — serde-compat for old JSON is free via `#[serde(default)]`.

**Delete paths** — `invalidate`/`invalidate_prefix` delegate only (the returned
count is index-scoped; blobs are reclaimed asynchronously at their death epoch).
`clear` best-effort unlinks the payload dir contents (never `Err` on unlink),
then delegates. `stats`/`name` delegate unchanged (`bytes` = inner accounting;
offloaded entries contribute an emptied `bytes` field — redb sums entry
`bytes` lengths so each contributes 0, redis reports `None`; blob bytes never
appear).

**Sweeper** — standalone tokio task mirroring redb's sweep loop
(`tokio::time::interval` + context `CancellationToken`, Drop-abort handle):
scans the dir, unlinks blobs whose filename epoch has passed and `*.tmp` files
older than `payload_sweep_interval` (crash leftovers). `ENOENT` = success
(multi-replica races on RWX). No sweeper under inline.

**Wiring** — `camel-config` `context_ext.rs` builds the backend repo then wraps
it before `register_cache_repository` when `payload = "disk"`; emits the startup
portability WARN naming `payload_dir`.

## Affected crates

- `camel-api`: `CacheEntry.payload_path: Option<String>` additive field.
- `camel-core`: `cache/disk_offload.rs` (decorator + sweeper); `blake3` dep
  (already in workspace via `camel-api`).
- `camel-config`: 4 fields, fail-closed matrix rows, wiring, portability WARN.
- `camel-processor`: struct-literal updates only (no behavior change).
- `camel-test`: integration suite (round-trip, fallback, MISS-on-dead-file,
  sweeper, sanitization, no-TTL cap).
- docs: ADR (offload mode + death-epoch design), schema.md rows, operational
  notes (NFS, rollback = clear/seed, portability).

## Architecture boundaries

Data plane: the decorator is a local-filesystem repository concern in
`camel-core` — it touches no external service and respects the
runtime/contract split (`camel-api` gains only a data-model field). Control
plane: mode selection lives in `camel-config` validation, fail-closed per
field per backend, mirroring the existing matrix (ADR-0063 cross-backend field
discipline). EIP faces (`cache`, `cache_peek_stale`) are untouched; expiry/SWR
semantics are unchanged. Live integration tests follow ADR-0054 (no
`#[ignore]`, per-test containers).

## Alternatives considered

- **Per-repo offload options** — rejected: triplicates the logic across three
  backends that already diverge (SCAN/UNLINK vs txn collect-then-delete).
- **Compact binary codec (bincode/base64) instead of offload** — rejected as a
  substitute: fixes the x4 JSON bloat but keeps the full dataset in RAM; does
  not unlock `replicas > 1` on redb's RWO lock. Kept as a separate follow-up.
- **Trait widening (`keys(prefix)`) / directory-per-prefix layout** — rejected:
  self-die filenames make eager file deletion unnecessary; purge = index purge +
  asynchronous reclaim.
- **`payload_min_size` threshold** — YAGNI: all payloads in the target workload
  are ~50 KB+; additive later if a small-entry consumer appears.

## Decisions locked by expert review (e_opus + e_glm)

Ratified GREEN-LIGHT-WITH-RESHAPE / RATIFY-WITH-AMENDMENTS: file-first order,
inline fallback on blob-write failure, MISS+WARN on file-dead, fabricated
expiry for no-TTL, sweep grace in the death epoch, memory+disk rejection,
`payload_dir` required without default, ENOENT=success, tmp GC, read-side path
sanitization, stats/name delegation (`bytes` = inner value, index bytes under
offload), I/O errors on existing blobs surface as Err (C1), 128-bit content
fingerprint in blob filenames (no cross-writer pairing), unique per-attempt
tmp with create-new semantics, bincode out of scope.
