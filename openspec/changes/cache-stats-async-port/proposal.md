# Proposal: cache-stats-async-port

## Why

bd rc-22wj (discovered-from rc-phvs, cache-admin): `RedbCacheRepository::stats()`
computes `bytes` via `total_bytes()` — a synchronous `begin_read` + full-table
`serde_json` deserialize loop that runs directly on the tokio worker thread. Every
other redb operation in the adapter wraps blocking I/O in `spawn_blocking`;
`stats()` is the single exception because the port signature
`fn stats(&self) -> CacheStats` (camel-api) is synchronous, so `.await` is
structurally impossible. On a large persistent table the scan stalls one worker for
hundreds of milliseconds to seconds (approx. 0.5–5 µs per entry), degrading tail
latency of unrelated routes that share the runtime. The cache-admin holistic review
blessed this knowingly at admin frequency; this change retires the trade-off instead
of living with it.

## What Changes

- `CacheRepository::stats` becomes `async fn stats(&self) -> CacheStats` with a
  default body of `CacheStats::default()`, matching the six already-async port
  methods. Pre-1.0 source break, recorded as an ADR-0056 amendment.
- `RedbCacheRepository::stats()` keeps exact payload-sum semantics but runs the
  existing scan inside `spawn_blocking`; scan or join failure degrades to
  `bytes: None` (stats stays infallible).
- `MemoryCacheRepository` gains a private synchronous `stats_snapshot()` helper;
  async `stats()` and `Debug::fmt` delegate to it (Debug cannot await).
- `CacheStatsService` (camel-processor) awaits `stats()` — the call site is already
  inside an async block.
- The `cache_stats` JSON snapshot shape and the meaning of `bytes` are unchanged.
- Docs: ADR-0056 amendment; CONTEXT-MAP.md; camel-api/CONTEXT.md;
  camel-core/CONTEXT.md; camel-processor/CONTEXT.md.

Explicitly excluded (scope guard, e_gpt ruling): no `stats_async` twin method, no
`stored_bytes()` or metadata-table byte source, no cached byte counter maintained on
write operations, no `entries` counter change, no unrelated refactor.

## Acceptance criteria

- `cargo build --workspace` passes with `stats` async across all four implementors
  (redb, memory, processor test mock, camel-api test `NoIter`).
- The existing regression `stats_reports_bytes_sum` still asserts `Some(8)` for
  3-byte + 5-byte payloads (payload-sum semantics preserved).
- No synchronous redb I/O remains in `stats()` on the async path: the scan runs
  inside `spawn_blocking`.
- The `cache_stats` step emits a JSON snapshot with an unchanged JSON object schema
  and exact key set (`bytes` number-or-null).
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` passes.
- Docs updated per the two-source rule (ADR-0056 amendment + CONTEXT notes).

## Risk budget

Accepted: one pre-1.0 source break (about a dozen test call sites gain `.await`).
Not accepted: semantics changes to `bytes`, new public API surface (twin methods),
hot-path cost added to writes to serve an admin read. Rollback: single revert; no
data migration; no on-disk format change.
