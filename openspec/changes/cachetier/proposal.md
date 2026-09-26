# Proposal: cachetier

## Why

ADR-0065 payload offload is disk-ONLY: `DiskOffloadRepository` wraps any
index backend, and payload bytes always land in `payload_dir` on local
disk. Teams on shared or no-local-volume deployments (demo-team NFS
report, bd rc-6b88t) must point `payload_dir` at NFS, which ADR-0065
documents as the worst-case deployment. When the index is redis, the
natural payload home is redis itself — shared by construction, no local
volume needed. Today no such tier exists.

## What Changes

Add a payload location tier to `[cache_repo]`: `payload = "disk"` (today's
behavior, default posture unchanged) or `payload = "redis"` — payloads
stored as redis entries alongside the redis index, with TTL discipline
(EXAT at the blob death epoch) and key-delete reclaim.

- `PayloadMode` gains `Redis`; `None` (inline, no offload) is unchanged.
- The ADR-0065 decorator becomes store-pluggable: a `PayloadStore` trait
  in camel-core with the disk store (existing blob lifecycle, unchanged)
  and a new `RedisPayloadStore` in camel-redis-repo riding the same
  multiplexed connection and keyspace discipline as the index.
- Fail-closed validation: `payload = "redis"` requires `backend = "redis"`;
  `payload_dir` set with the redis tier is rejected; the existing disk
  matrix (dir required, memory backend rejected) is untouched.
- Reclaim semantics per tier: disk keeps the death-epoch sweeper; redis
  uses eager predecessor UNLINK on overwrite plus native EXAT expiry.
- Delta spec: ADD "Payload location tier selection" requirement to
  `cache-repo-configuration`.
- ADR-0065 amendment records the tiering decision and the NFS motivation.

Excluded: inline-by-size threshold (optional in bd rc-6b88t, not
requested by mission 282), any camel-api surface change (`payload_path`
already exists), any camel-component-redis change (zone lease boundary).

Affected crates: camel-core, camel-redis-repo (service), camel-config,
camel-test (live tests).

## Acceptance criteria

- `[cache_repo] payload = "redis"` validates only with
  `backend = "redis"`; disk and inline modes behave exactly as before.
- Redis tier payload roundtrip works against TestContainers redis:
  index rows stay bytes-empty with `payload_path`, payload bytes live
  under the repository's redis namespace, `get` re-injects them.
- Payload is NOT stored in the index entry on the redis tier.
- `payload = "redis"` with memory or redb backend, or with `payload_dir`
  set, fails validation with an error naming the offending field.
- Reclamation documented per tier (sweeper vs UNLINK+EXAT) in ADR-0065.
- `openspec/specs/cache-repo-configuration` delta validates.

## Risk budget

Acceptable: additive config knob with fail-closed rejection; refactor of
the disk decorator internals behind one constructor call site, guarded by
the existing disk-offload test suite staying green. Out of bounds:
behavior change for `payload = "disk"` or unset payload; new camel-api
types; touching camel-component-redis; unbounded payload growth (EXAT
must bound every redis payload entry).
