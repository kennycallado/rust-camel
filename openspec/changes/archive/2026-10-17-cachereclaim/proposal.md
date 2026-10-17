# Proposal: cachereclaim

## Why

External report from the camel-cache demo team (bd rc-uteoa): a cache with a
redis index and disk payload offload on NFS accumulates old blobs for
hot-overwritten keys. Every `set()` writes a new blob named
`{key-hash}.{death_epoch}.{fingerprint}.blob`; the previous blob is reclaimed
only when its own filename-encoded death epoch passes
(`ttl + stale_retention + payload_sweep_interval`). For a key overwritten
every few seconds with a long retention, dozens of grace-window blobs pile
up, and the standalone sweeper's `readdir` cost grows with the directory.

The fix is safe by existing contract: `hydrate()` already treats a missing
blob as `Ok(None)` cache MISS with WARN ("sweep lag, NFS skew, crash
window" — ADR-0065 Decision 5). Eager reclamation can only widen that
documented race window; it can never turn a read into an `Err`.

## What Changes

- `DiskOffloadRepository::set` (crates/camel-core/src/cache/disk_offload.rs)
  gains an eager predecessor reclaim: read the current index row's
  `payload_path` before the swap, and after the decorated backend accepts
  the overwrite, unlink that predecessor blob (targeted unlink guided by the
  row — no directory scan). This bounds on-disk versions per overwritten
  key at 1 immediately in the single-writer case, with no new config knobs.
- Guards: reclaim only names that pass `sanitize_blob_name`, carry a
  parseable death epoch, AND start with the current key's blake3-128
  filename prefix (a corrupt row naming another key's blob never deletes
  that blob); never unlink the fresh blob itself (same-second identical
  rewrite yields the same filename); skip reclaim when the inner `set`
  fails (the row may still reference the predecessor).
- Best-effort: a failed unlink WARNs and never fails the write; the
  self-die filename epoch and the standalone sweeper remain the backstop
  for strays (crash window, multi-writer orphans).
- ADR-0065 gains an amendment section recording the strategy and
  superseding the "self-die filenames make eager file deletion
  unnecessary" rejection rationale.
- Excluded: `invalidate()`/`invalidate_prefix()` stay delegate-only;
  `clear()` and the sweeper are unchanged; camel-config is untouched (no
  new knobs); per-key version caps (K>1) and payload tiering (rc-6b88t)
  are out of scope.

## Acceptance criteria

- Overwriting a key bounds on-disk blob versions for that key at 1 inside
  the same `set()` call when the reclaim unlink succeeds or the
  predecessor is already gone (ENOENT); when the unlink fails, the
  predecessor survives until its death epoch and the sweeper reclaims it.
  This bound holds for keys written by one writer at a time; losing
  writers under concurrent same-key writes leave transient orphans that
  only the sweeper reclaims, at their death epoch (the next overwrite
  reads the current row and cannot discover them).
- A reader holding a pre-swap index row while the writer reclaims hydrates
  to a MISS (`Ok(None)`), never an `Err`.
- A failed predecessor-row read (`get` Err) before the write WARNs, skips
  the reclaim, and never fails the write.
- No behavior change for first writes, non-overwritten keys, `invalidate`,
  `clear`, or sweeper semantics.
- ADR-0065 documents the reclaim strategy and the NFS trade (one extra
  index GET plus one unlink per overwrite instead of an O(dirsize)
  `readdir`).

## Risk budget

Acceptable: one extra index GET per `set()` (redis sub-ms) and a slightly
wider miss window for readers of an overwritten key — both documented in
ADR-0065. Out of bounds: any new route-visible failure mode (a failed
reclaim must degrade to today's behavior), any change to Contract C1
(missing blob = miss; failing disk = error), any config-surface change.
