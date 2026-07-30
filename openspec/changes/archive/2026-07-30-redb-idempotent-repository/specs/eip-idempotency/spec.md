## ADDED Requirements

### Requirement: Persistent redb idempotent repository backend

The system SHALL provide a `RedbIdempotentRepository` that implements `camel_api::IdempotentRepository`, persists keys to a redb file on disk, and survives process restart. Every trait operation SHALL wrap blocking redb I/O in `tokio::task::spawn_blocking` and SHALL map redb errors to `CamelError::Io`, satisfying Contract C1 (ADR-0023): a transient backend failure SHALL surface as `Err` and SHALL never be reported as "key absent". The `RedbIdempotentRepository` SHALL NOT impose a `max_entries` cap; unlike `MemoryIdempotentRepository` it trades bounded memory for unbounded disk growth. TTL/eviction is out of scope for this change.

#### Scenario: add returns true for a new key and false for a duplicate

- **GIVEN** a `RedbIdempotentRepository` opened on an empty redb file
- **WHEN** `add("msg-1")` is called, then `add("msg-1")` is called again
- **THEN** the first call returns `Ok(true)` and the second returns `Ok(false)`

#### Scenario: contains reflects added and removed keys

- **GIVEN** a `RedbIdempotentRepository` with `"msg-1"` already added
- **WHEN** `contains("msg-1")` is called, then `remove("msg-1")`, then `contains("msg-1")` again
- **THEN** the first `contains` returns `Ok(true)`, `remove` returns `Ok(())`, and the second `contains` returns `Ok(false)`

#### Scenario: clear removes all keys

- **GIVEN** a `RedbIdempotentRepository` with keys `"a"`, `"b"`, `"c"` already added
- **WHEN** `clear()` is called, then `contains("a")`, `contains("b")`, `contains("c")` are called
- **THEN** `clear()` returns `Ok(())` and every `contains` returns `Ok(false)`

#### Scenario: keys persist across a reopened handle

- **GIVEN** a `RedbIdempotentRepository` opened on file `X` that has added `"msg-1"`
- **WHEN** that handle is dropped and a new `RedbIdempotentRepository` is opened on the same file `X`
- **THEN** `contains("msg-1")` on the new handle returns `Ok(true)`

#### Scenario: concurrent add of the same key yields exactly one success

- **GIVEN** a `RedbIdempotentRepository` opened on file `X` with no key `"k"`
- **WHEN** two `add("k")` calls race concurrently against the same repository
- **THEN** exactly one call returns `Ok(true)` and the other returns `Ok(false)`

#### Scenario: construction failure surfaces as Contract C1 Err, not a silent-absent repo

- **GIVEN** a `RedbIdempotentRepository` construction is attempted on a path whose parent exists as a regular file (so directory creation or `Database::open` must fail)
- **WHEN** construction is attempted
- **THEN** it returns `Err(CamelError::Io(..))` and never yields a repository that would silently report keys as absent

### Requirement: redb idempotent repository is opt-in and configurable

The system SHALL register a redb-backed idempotent repository under the name `"redb"` only when configuration requests it, and SHALL keep `MemoryIdempotentRepository` as the default `"memory"` repository otherwise. The redb repository SHALL be configurable through `CamelConfig` via an `idempotent_repo: Option<RedbIdempotentConfig>` field that mirrors the existing `runtime_journal: Option<JournalConfig>` field, carrying a file path and a durability mode.

#### Scenario: redb registered when configured, memory still default

- **GIVEN** a `CamelConfig` whose `idempotent_repo` field is set to a path and durability
- **WHEN** the context is built from that config
- **THEN** a repository is resolvable by name `"redb"` and a repository is still resolvable by name `"memory"`

#### Scenario: redb absent when not configured, memory remains default

- **GIVEN** a `CamelConfig` with no `idempotent_repo` field
- **WHEN** the context is built from that config
- **THEN** no repository is resolvable by name `"redb"` and a repository is resolvable by name `"memory"`

#### Scenario: durability defaults to immediate

- **GIVEN** a `RedbIdempotentConfig` parsed from configuration that omits the durability field
- **WHEN** the resulting `RedbIdempotentRepository` adds a key
- **THEN** the write is fsynced (immediate durability) before `add` returns `Ok`

#### Scenario: eventual durability skips fsync

- **GIVEN** a `RedbIdempotentConfig` with durability set to `eventual`
- **WHEN** the resulting `RedbIdempotentRepository` adds a key
- **THEN** the write commits without forcing fsync (eventual durability)

#### Scenario: parent directory is created before opening the database

- **GIVEN** a configured path whose parent directory does not yet exist
- **WHEN** the `RedbIdempotentRepository` is constructed
- **THEN** the parent directory is created and the repository opens successfully

#### Scenario: empty idempotent repo path is rejected at config validation

- **GIVEN** a `CamelConfig` whose `idempotent_repo.path` is empty
- **WHEN** the config is validated
- **THEN** validation returns an error naming the offending field
