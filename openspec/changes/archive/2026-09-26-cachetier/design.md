# Design: cachetier

## Approach

Make the ADR-0065 offload decorator payload-store-pluggable, then add a
redis store that rides the index's own connection and keyspace.

1. **camel-core — pluggable store.** New module
   `crates/camel-core/src/cache/offload.rs` holds a `PayloadStore` trait
   and the decorator, renamed `OffloadRepository`:

   ```rust
   #[async_trait]
   pub trait PayloadStore: Send + Sync {
       async fn put(&self, name: &str, bytes: &[u8],
                    death_epoch: SystemTime) -> Result<(), CamelError>;
       async fn read(&self, name: &str) -> Result<Option<Vec<u8>>, CamelError>;
       async fn unlink(&self, name: &str) -> Result<(), CamelError>;
       async fn clear(&self) {} // eager bulk reclaim (disk); redis payload keys die via the index clear()'s namespace SCAN
   }
   ```

   The decorator keeps everything ADR-0065 fixed: blob naming
   (`{key-hash}.{death-epoch}.{content-fingerprint}.blob`), death-epoch
   math (`expires_at + stale_retention + sweep_interval`), fabricated TTL
   from `payload_max_ttl`, bytes-empty index row with `payload_path`,
   MISS+WARN on a vanished payload, `Err` on payload-read failure (C1),
   inline fallback on payload-write failure, eager predecessor reclaim on
   overwrite (bd rc-uteoa). `disk_offload.rs` shrinks to
   `DiskPayloadStore` (tmp+fsync+rename writes, reads, unlinks, and the
   standalone sweeper with its `Drop` abort; it owns the shutdown token).
   The decorator no longer spawns or aborts anything.

2. **camel-redis-repo — `RedisPayloadStore`.** New
   `src/payload_store.rs` implements `PayloadStore` over the existing
   `RepoCommandExecutor` seam via `execute_retry_safe`. Keys live inside
   the repository namespace: `{prefix}:{repo}:payload:{blob-name}` — so
   `clear()`'s prefix-scoped SCAN+UNLINK reclaims payloads eagerly and
   the charset/namespace guards stay uniform. `put` = `SET key bytes EXAT
   death_epoch_secs` (checked math; on overflow `put` returns `Err` so
   the ADR-0065 inline fallback applies — a payload blob is never stored
   without a deadline); `read` = `GET` (nil → `None` → MISS+WARN);
   `unlink` = `UNLINK`. TTL discipline: native EXAT replaces the disk
   sweeper — every payload entry is bounded. A new
   `RedisCacheRepository::connect_with_payload_store` returns the index
   and the store over ONE multiplexed connection (ADR-0063: one
   connection per repository); `with_executor` test seam extended to
   build both from a `FakeRepoExecutor`.

3. **camel-config — knob + fail-closed matrix.** `PayloadMode` gains
   `Redis` (`payload = "disk" | "redis" | "inline"`). New validation
   rows: `payload = "redis"` requires `backend = "redis"` (error names
   the backend); `payload = "redis"` with `payload_dir` set is rejected
   with a redis-tier message naming `payload_dir`; the memory backend
   keeps rejecting every offload mode. The duration knobs
   (`payload_sweep_interval`, `payload_max_ttl`) stay tunable for BOTH
   offload tiers — the decorator consumes them for death-epoch/EXAT math
   on every tier — with the existing parse and ≥1s checks; they remain
   rejected for inline mode. Env override `CAMEL_CACHE_REPO_PAYLOAD`
   already passes values verbatim — "redis" flows with no new code.

4. **Wiring.** `wrap_disk_offload` generalizes to `wrap_payload_offload`
   taking an optional `RedisPayloadStore` (built only on the redis
   branch via `connect_with_payload_store`). redb branch keeps disk-only
   wrapping. Decorator registers under the bare backend's name, as
   today. No startup WARN for the redis tier: redis is shared by
   construction, unlike the disk-dir portability WARN.

## Affected crates

- `camel-core` — offload.rs (decorator + trait), disk_offload.rs (store)
- `camel-redis-repo` (service) — payload_store.rs, cache_repo.rs connect seam
- `camel-config` — PayloadMode, validation, context_ext wiring
- `camel-api` — one additive `#[derive(Clone)]` on `ComponentMetrics` (the
  shared-connection constructor hands one metrics facade to both the index
  and the payload store; no new types or fields)
- `camel-test` — live tiering suite (extends cache_payload_offload.rs)
- docs: ADR-0065 amendment, CONTEXT-MAP.md, CONTEXT.md (camel-core,
  camel-redis-repo, camel-config), configuration docs

## Architecture boundaries

Runtime core (camel-core) defines the trait; the service crate
(camel-redis-repo) implements it — dependency direction preserved
(core never sees redis). camel-api is untouched (`payload_path` already
exists). camel-component-redis untouched (zone lease). Config stays the
single construction site (fail-closed before any connection opens).

## Alternatives considered

- **Second `RedisOffloadRepository` decorator** duplicating
  interception/reclaim logic: rejected — two copies of ADR-0065 policy
  drift apart; the mission names the offload pluggable.
- **Separate payload connection**: rejected — doubles connections,
  duplicates failover/retry; ADR-0063 says one per repository.
- **Payload keyspace outside `{prefix}:{repo}`**: rejected — `clear()`
  could not reclaim payloads and the namespace guard would need a second
  token shape.
- **Hash value type**: rejected — one blob per name; a hash adds fields
  nothing reads.
