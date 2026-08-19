# Design: cache-stats-async-port

## Approach

Make the port honest about I/O. `CacheRepository` already exposes six async methods
under `#[async_trait]`; `stats` is the lone synchronous method, and that signature is
the sole reason `RedbCacheRepository::stats()` runs its payload-sum scan
(`total_bytes()`: `begin_read` + per-entry `serde_json` deserialize) on the tokio
worker (bd rc-22wj). Change the port to `async fn stats(&self) -> CacheStats` with
default body `CacheStats::default()` — infallible, no `Result`, unchanged. The redb
adapter then moves the existing scan, unchanged, into
`tokio::task::spawn_blocking`, mapping scan or join failure to `bytes: None`. The
memory adapter keeps its cheap snapshot through a private synchronous
`stats_snapshot()` helper shared by `stats().await` and `Debug::fmt` (Debug cannot
await). The single production caller, `CacheStatsService::run()`
(camel-processor/src/cache_eip.rs), is already inside `Box::pin(async move)` — it
gains `.await`.

Semantics freeze: `bytes` remains the payload-byte sum. It does NOT switch to redb
`stored_bytes()` — that reports serialized key + envelope bytes, is O(B-tree pages)
(`btree_stats()` walks every page; redb 4.1.0 tree_store/btree.rs:1044), and varies
with serialization versions. The `cache_stats` JSON snapshot keeps its exact shape.
This is a worker-safety fix, not a semantics change.

## Affected crates

- camel-api: trait method `stats` becomes async (default impl `CacheStats::default()`);
  unit tests and the `NoIter` test implementor await.
- camel-core: redb `stats()` loads atomic counters, then runs the existing payload-sum
  scan in `spawn_blocking` (failure maps to `bytes: None`); memory gains private
  `stats_snapshot()` used by `stats().await` and `Debug::fmt`; redb/memory test call
  sites await.
- camel-processor: `CacheStatsService` awaits `stats()`;
  `MockCacheRepository::stats` becomes async and reads the `stats_override` mutex.
- camel-config: test call sites await (tests/cache_repo_config.rs).
- Docs: ADR-0056 amendment (the interface-stability section sanctions default-method
  extension; a signature change requires an explicit amendment), CONTEXT-MAP.md,
  camel-api/CONTEXT.md, camel-core/CONTEXT.md, camel-processor/CONTEXT.md.

## Architecture boundaries

camel-api owns the port (hexagonal in-edge unchanged); camel-core owns both adapters;
the data-plane step (`CacheStatsService`) stays in camel-processor. No DSL, schema,
route-lint, or canonical-command surface changes — `cache_stats` compiles exactly as
before. The change runs under the existing
hexagonal_architecture_boundaries_test with no new cross-layer imports.

Single-phase change: one coherent slice, no milestone grouping.

## Alternatives considered

- Additive `stats_async` twin (e_opus B2): rejected — permanent caller ambiguity and
  it leaves an unsafe synchronous method on the port (e_gpt ruling).
- redb native `stored_bytes()` (e_opus C1): rejected — not O(tree-height) as claimed
  (page walk), and it silently redefines `bytes` from payload sum to serialized blob
  size.
- Cached `AtomicU64` maintained on writes: rejected — adds deserialize/delta work to
  every `set`/`invalidate` to serve an admin-frequency read; drift and underflow risk.
- `block_in_place`: rejected — parks a worker anyway and requires the multi-thread
  runtime; the production caller is already async.
- Do nothing: rejected — pre-1.0 (0.29.0) is the cheap window for the correct port
  shape; after API freeze this fix becomes semver-expensive.
