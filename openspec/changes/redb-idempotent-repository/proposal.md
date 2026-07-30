# Proposal: redb-idempotent-repository

## Why

`MemoryIdempotentRepository` (the current default, ADR-0023) is volatile: a process restart loses every idempotent key, so duplicate messages arriving after restart are reprocessed. For at-most-once delivery across restarts, the dedup state must survive to disk.

This change adds `RedbIdempotentRepository`, a persistent backend implementing the existing `IdempotentRepository` trait (camel-api). redb is already a workspace dependency (`redb = "4"`) and is the storage engine behind `RedbRuntimeEventJournal` (ADR-0018). It is pure-Rust, ACID, and needs no external infrastructure, which keeps single-binary deployment intact. Multi-replica topologies still require a shared store (Redis/SQL-backed); redb is embedded/local only.

bd issue: rc-ymk (discovered-from rc-blw, the EIP-parity epic).

## What Changes

Included:

- `RedbIdempotentRepository` struct holding a `redb::Database` handle, in `camel-core/src/idempotent/`, registered under name `"redb"`.
- `IdempotentRepository` impl: `contains`/`add`/`remove`/`clear`, all `Result`-returning (Contract C1). Every operation wraps blocking redb I/O in `tokio::task::spawn_blocking` and maps errors to `CamelError::Io`, mirroring `RedbRuntimeEventJournal`.
- `add` uses a single write transaction: `insert` returns the prior value, so check-and-insert is atomic and `Ok(true)`/`Ok(false)` is derived from `prior.is_none()`. redb serializes writers (single-writer MVCC), so no per-instance mutex is needed.
- A `RedbIdempotentConfig` (`path`, `durability`) in `camel-config`, plus opt-in wiring in `context_ext.rs` that builds and registers the repo as `"redb"` when configured.

Excluded (deferred, to be filed as follow-up bd):

- TTL / time-based eviction. The trait has no TTL surface; the value jump is durability alone.
- Sharing the journal's `.redb` file. A separate file (`.camel/idempotent.redb`) is the v1 default to decouple durability and compaction.
- Distributed coordination. Embedded/local only.

## Acceptance criteria

- `RedbIdempotentRepository` implements `IdempotentRepository`; `contains`/`add`/`remove`/`clear` work against a persistent redb file.
- A key written before a handle is dropped is present when the same file is reopened (persistence across restart).
- `add` is atomic: concurrent `add` of the same key yields exactly one `Ok(true)`.
- Transient backend failure surfaces as `Err` (Contract C1); never as "not a duplicate".
- `MemoryIdempotentRepository` remains the default `"memory"` repo; redb is opt-in via config.
- Configurable through `CamelConfig` (path + durability), wired the same way as the runtime journal.
- Unit + integration tests cover the trait contract and restart persistence.

## Risk budget

Acceptable: per-operation `spawn_blocking` overhead (materially slower than DashMap; correct and necessary for sync redb I/O). Acceptable: `Immediate` durability default costs an fsync per added key on the message hot path — documented as a trade-off; high-throughput routes choose `Eventual` and accept at-least-once degradation on OS/power crash.

Out of bounds: any change to the `IdempotentRepository` trait (C1 is fixed). Out of bounds: distributed dedup.
