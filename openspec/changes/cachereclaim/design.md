# Design: cachereclaim

## Approach

Row-guided targeted unlink inside `DiskOffloadRepository::set`
(crates/camel-core/src/cache/disk_offload.rs). The predecessor blob's name
is already persisted in the index row (`payload_path`), so reclaim needs no
directory scan:

1. Before mutation, `inner.get(key)` captures the current row's
   `payload_path` (the predecessor blob name). A miss (first write, expired
   row, inline row) means no reclaim this write. An `Err` from this read
   WARNs, skips the reclaim, and never fails the write — the reclaim is an
   optional optimization.
2. The existing write path runs unchanged: blob write (tmp + fsync +
   rename), then `inner.set` swaps the index row.
3. If `inner.set` returned `Ok`, reclaim the predecessor:
   `remove_file(dir.join(old_name))`, best-effort — failure WARNs and never
   fails the write. The reclaim also runs on the inline-fallback path (the
   new row no longer references the predecessor).
4. Guard set: skip unless `sanitize_blob_name(old_name)` passes AND
   `parse_death_epoch(old_name).is_some()` AND `old_name` starts with the
   current key's blake3-128 filename prefix (`blake3_128hex(key) + "."` —
   a corrupt row naming ANOTHER key's blob, a traversal path, or a foreign
   file such as `readme.txt` is never unlinked, matching and tightening
   sweeper discipline). On the
   successful-blob path, additionally skip when `old_name == dest_name` (a
   same-second identical rewrite produces the same filename; unlinking it
   would delete the fresh blob). The equal-name guard does NOT apply on the
   inline-fallback path — a failed blob write leaves no fresh file owning
   that name, so the predecessor is reclaimed even when the names collide.
   If `inner.set` returned `Err`, skip reclaim entirely — the row may still
   reference the predecessor.

Rejected mechanisms, per the decision criteria (NFS metadata-op cost per
`set()`, reader-race amplitude, code size, ADR-0065 alignment):

- **Prefix-filtered `readdir`** (the order's original sketch): an NFS
  `readdir` costs O(dirsize) metadata round-trips per `set()` — a
  self-inflicted storm exactly when the directory is largest. The
  row-guided unlink costs one index GET (redis: sub-ms) plus one unlink,
  independent of directory size.
- **Per-key version cap K>1**: needs a new `payload_max_versions` config
  knob (camel-config is outside this change's lease), more state, and the
  same scan cost; rejected.
- **Write-through sweep** (unlink already-dead blobs seen in a `readdir`):
  a hot key's predecessor is not dead until `ttl + stale_retention +
  sweep_interval` elapses, so accumulation persists the whole grace window;
  fails the acceptance criterion.

Concurrency and race contract (unchanged C1 semantics, ADR-0065 Decision 5):
a reader holding a pre-swap row whose blob was just reclaimed hydrates to
`Ok(None)` MISS + WARN — the documented "sweep lag, NFS skew, crash
window" degradation, merely a wider window for the overwritten key.
Multi-writer same-key: both writers capture predecessor X; last swap wins;
X is unlinked by either (ENOENT = success); the losing writer's blob is a
transient orphan that only the sweeper reclaims, at its death epoch — the
next overwrite reads the current row and cannot discover it. Readers never
see cross-paired content (the row references its own fingerprinted blob).
The eager unlink is not directory-fsynced; a crash in that window leaves a
stray the sweeper collects at its epoch.

Reclaim is best-effort and backend-observable: it fires only when
`inner.get` observes the predecessor row, so an expired-but-retained row
(backends return miss from `get` after expiry) is reclaimed by the sweeper
at its epoch instead. No metrics are added — the file emits tracing logs
only; the reclaim WARN/INFO follows the existing log discipline.

## Affected crates

- camel-core: `src/cache/disk_offload.rs` (`set()` reclaim step, ~40
  lines) and `src/cache/disk_offload_tests.rs` (new tests).
- docs: `docs/adr/0065-cache-payload-offload.md` (amendment section).

## Architecture boundaries

Runtime-internal decorator change inside camel-core's cache module; no
API, DSL, component, service, or config surface moves. The data-plane
cache contract (Contract C1, ADR-0023/0065) is preserved: missing blob =
miss, failing disk = error. Aligned with ADR-0065 by amendment (self-die
epochs stay the backstop; eager reclaim is a targeted optimization on
top), recorded as a new ADR section rather than a rewrite.

Single-phase change: one coherent slice (spec delta + reclaim
implementation + tests + ADR amendment).
