# Design: redb-idempotent-repository

## Approach

Add `RedbIdempotentRepository`, a persistent `IdempotentRepository` backend backed by an embedded redb file. The struct holds a diagnostic `name`, the on-disk `path`, a `redb::Database` handle, and a `JournalDurability` mode. It stores idempotent keys in one redb table keyed by `&str` with a unit value.

The implementation follows `RedbRuntimeEventJournal` (ADR-0018) mechanically, because both wrap the same engine under the same async runtime:

- `Database::open` runs in `spawn_blocking`; the parent directory is created first (redb does not `mkdir -p`).
- Each trait method opens a transaction, does its work, and commits — all inside `spawn_blocking`.
- redb errors map to `CamelError::Io` with a `redb <op>` prefix, satisfying Contract C1 (ADR-0023): transient failures propagate as `Err`, never as "not a duplicate".

Operation semantics:

- `contains` — read transaction; `open_table` + `get(key)`; result is `value.is_some()`.
- `add` — single write transaction; `set_durability` per the journal's `&mut tx` pattern; `table.insert(key, ())` returns the prior value. `Ok(prior.is_none())` — newly added vs already present. This is atomic within one transaction, so it is strictly stronger than the memory repo's mutex-guarded check-then-act. redb serializes writers (single-writer MVCC), so no per-instance `Mutex` is added.
- `remove` — write transaction; `table.remove(key)`; succeeds whether or not the key existed.
- `clear` — write transaction; collect keys into a `Vec` (the iterator and `remove` cannot share the borrow), then `remove` each. `delete_table` is rejected because a concurrent reader could observe a missing table and surface a spurious `Err` under Contract C1.

A small gotcha the impl must handle: `redb::Database` is not `Debug`, but the trait requires `Debug`. `Debug` is implemented by hand, printing `name` and `path`.

## Affected crates

- `camel-core`: new module `idempotent/redb_repository.rs` and re-export from `idempotent/mod.rs`. New type `RedbIdempotentRepository` plus a `TableDefinition<&str, ()>`. Unit + integration tests.
- `camel-config`: new `RedbIdempotentConfig { path, durability }` (serde, `deny_unknown_fields`) mirroring `JournalConfig`; a mirror `IdempotentDurability` enum with a `From` to `camel_core::JournalDurability`, reusing the same `Immediate`/`Eventual` mapping.
- `camel-config` (`context_ext.rs`): opt-in wiring — when `config.idempotent_repo` is `Some`, build `RedbIdempotentRepository` and `builder.register_idempotent_repository("redb", repo)`.

No change to `camel-api` (the trait is fixed by ADR-0023).

## Architecture boundaries

The repository is data-plane state, on the same side as `MemoryIdempotentRepository` and `RedbRuntimeEventJournal`. It does not touch the control plane (RuntimeBus, route lifecycle), the DSL, components, or languages. Registration reuses the existing `NamedRegistry<IdempotentRepository>` wiring (ADR-0023) that already powers the default `"memory"` repo and the compile-time name resolution used by the Idempotent Consumer step. Keeping redb as an opt-in `"redb"` name leaves the `"memory"` default untouched, so this change is strictly additive.

ADR references: ADR-0018 (redb persistence + durability pattern), ADR-0023 (trait boundary, Contract C1, key-only), ADR-0028 (separate Claim Check trait — shared `NamedRegistry` wiring, not inherited).

## Alternatives considered

- **TTL/eviction in v1.** Rejected for v1. The trait has no TTL surface, and durability is the value jump. A timestamp value plus a background cleanup task would add lifecycle and wall-clock coupling for an invisible-to-routes knob. Deferred to a follow-up bd. Unlike the memory repo, the redb repo has no `max_entries` cap; it trades bounded memory for unbounded disk, which is the correct trade for a persistent store.
- **Share the journal's `.redb` file.** Rejected. redb permits multiple handles on one file, but sharing couples two subsystems with different durability needs and compaction cadences, and forces the repo to depend on the journal being enabled. A separate file (`.camel/idempotent.redb`) decouples them. The issue marks this as tuning, not architectural.
- **A config map of N named redb repos.** Rejected for v1. A single opt-in `"redb"` repo mirrors the existing `runtime_journal` config shape and keeps the upgrade path clean (move `Option<Config>` to a map later is additive).

## Durability default decision

`Immediate` is the config default, for correctness parity with the journal: an added key is fsynced before `add` returns, so at-most-once holds across OS/power crash. This costs one fsync per added key on the message hot path. High-throughput routes set `Eventual`, which skips fsync and accepts that a crash may lose recently-added keys (at-least-once degradation). The trade-off is documented on the config type and in crate docs.
