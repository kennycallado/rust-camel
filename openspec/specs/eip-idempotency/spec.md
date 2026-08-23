# eip-idempotency Specification

## Purpose
TBD - created by archiving change redb-idempotent-repository. Update Purpose after archive.
## Requirements
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

The system SHALL register a redb-backed idempotent repository under the name `"redb"` only when configuration requests it, and SHALL keep `MemoryIdempotentRepository` as the default `"memory"` repository otherwise. The redb repository SHALL be configurable through `CamelConfig` via an `idempotent_repo: Option<IdempotentRepoConfig>` field that mirrors the existing `runtime_journal: Option<JournalConfig>` field. `IdempotentRepoConfig` SHALL be a `#[serde(deny_unknown_fields)]` struct (not a serde-tagged enum) carrying `backend: String` with `#[serde(default = "default_idempotent_backend")]` defaulting to `"redb"`, so existing TOML that sets only `path` and `durability` keeps parsing unchanged. Fields: `backend`; redb: `path` (file path) and `durability` (mode); redis: `url`, `sentinel_nodes`, `master_name`, `sentinel_username`, `sentinel_password`, and `key_prefix` (default `camel:idem`, charset `[A-Za-z0-9:_-]`). Redis fields are validated with the same fail-closed matrix as `cache_repo` (exactly one topology, non-empty values, no orphan sentinel fields, `redis://`/`rediss://` schemes only, cluster fields rejected), with errors naming `idempotent_repo.<field>`. The struct's `Debug` output SHALL redact URL userinfo and sentinel credentials.

#### Scenario: redb registered when configured, memory still default

- **GIVEN** a `CamelConfig` whose `idempotent_repo` field is set to a path and durability
- **WHEN** the context is built from that config
- **THEN** a repository is resolvable by name `"redb"` and a repository is still resolvable by name `"memory"`

#### Scenario: redb absent when not configured, memory remains default

- **GIVEN** a `CamelConfig` with no `idempotent_repo` field
- **WHEN** the context is built from that config
- **THEN** no repository is resolvable by name `"redb"` and a repository is resolvable by name `"memory"`

#### Scenario: durability defaults to immediate

- **GIVEN** an `IdempotentRepoConfig` with `backend = "redb"` parsed from configuration that omits the durability field
- **WHEN** the resulting `RedbIdempotentRepository` adds a key
- **THEN** the write is fsynced (immediate durability) before `add` returns `Ok`

#### Scenario: eventual durability skips fsync

- **GIVEN** an `IdempotentRepoConfig` with `backend = "redb"` and durability set to `eventual`
- **WHEN** the resulting `RedbIdempotentRepository` adds a key
- **THEN** the write commits without forcing fsync (eventual durability)

#### Scenario: parent directory is created before opening the database

- **GIVEN** a configured path whose parent directory does not yet exist
- **WHEN** the `RedbIdempotentRepository` is constructed
- **THEN** the parent directory is created and the repository opens successfully

#### Scenario: empty idempotent repo path is rejected at config validation

- **GIVEN** a `CamelConfig` whose `idempotent_repo.backend = "redb"` and `idempotent_repo.path` is empty
- **WHEN** the config is validated
- **THEN** validation returns an error naming the offending field

#### Scenario: existing redb TOML parses unchanged

- **GIVEN** a `[default.idempotent_repo]` section that sets only `path` and `durability`,
  written before the `backend` discriminator existed
- **WHEN** the section is parsed into `IdempotentRepoConfig`
- **THEN** parsing succeeds with `backend = "redb"`

### Requirement: Redis idempotent repository backend

The system SHALL provide a `RedisIdempotentRepository` in the `camel-redis-repo` repository service crate that implements `camel_api::IdempotentRepository` over a multiplexed Redis connection owned by the repository. `add` SHALL issue `SET key 1 NX` atomically and SHALL return `Ok(true)` when the key was set, `Ok(false)` when it already existed. The repository SHALL NOT re-issue `SET NX` after a transport error; the unknown outcome SHALL surface as `Err(CamelError::Io(..))` and the connection MAY be refreshed for subsequent calls. `contains` SHALL issue `EXISTS`. `remove` SHALL issue `UNLINK`. `clear` SHALL delete only keys under the repository prefix via `SCAN` + `UNLINK` batching and SHALL never issue `FLUSHDB` or `FLUSHALL`. Keys SHALL be namespaced under `{key_prefix}:{repo-name}:` with default prefix `camel:idem`. The repository name SHALL be validated at construction with the same rule as `key_prefix`: non-empty, charset `[A-Za-z0-9:_-]`, no glob metacharacters — the name is part of every SCAN pattern and an unsafe name would break `clear` scoping. Any backend or transport failure SHALL surface as `Err(CamelError::Io(..))`, never as `Ok(false)` and never as a silent absence (Contract C1). During a sentinel failover, a `SET NX` whose outcome is unknown SHALL return `Err`. No TTL SHALL be applied to idempotent keys (out of scope, parity with ADR-0023 and the redb backend). The repository SHALL register under the name `"redis"` only when `[default.idempotent_repo] backend = "redis"` is configured, reusing the same connection lifecycle and validation rules as the cache backend (`url` XOR `sentinel_nodes` + `master_name`; cluster fields rejected), not the same connection — each repository owns its own connection. The idempotent redis configuration SHALL carry the same data-node fields as the cache backend: optional `password` and `username` (authenticate the master/replicas in sentinel mode; rejected in `url` mode — where password and db ride the URI and username-in-URI is out of scope — and on the `redb` backend) and optional `db` of type `Option<u16>` (validated 0..=16383, default 0, rejected in `url` mode and on the `redb` backend), validated with the same fail-closed matrix as `cache_repo` under the `idempotent_repo.<field>` names, with the `Debug` output redacting the data-node `password` and `username`, and with the sentinel-mode `db` participating in the effective-endpoint identity of the cross-repo prefix-collision rule.

#### Scenario: add is atomic insert-if-absent

- **GIVEN** an empty `RedisIdempotentRepository`
- **WHEN** `add("msg-1")` is awaited twice
- **THEN** the first call returns `Ok(true)` and the second returns `Ok(false)`

#### Scenario: transient failure during failover surfaces as Err

- **GIVEN** a `RedisIdempotentRepository` whose underlying executor returns a transient
  connection error for `SET NX` (simulated mid-failover)
- **WHEN** `add("k")` is awaited
- **THEN** the result is `Err(CamelError::Io(..))` and not `Ok(true)` or `Ok(false)`

#### Scenario: add does not retry SET NX after a lost response

- **GIVEN** a `RedisIdempotentRepository` whose executor returns a transient connection
  error on the first `SET NX` (response lost)
- **WHEN** `add("k")` is awaited
- **THEN** the executor issues `SET NX` exactly once (no re-issue), the result is
  `Err(CamelError::Io(..))`, and a subsequent `add` uses a refreshed connection

#### Scenario: contains and remove round-trip

- **GIVEN** a `RedisIdempotentRepository` with `"msg-1"` already added
- **WHEN** `contains("msg-1")`, then `remove("msg-1")`, then `contains("msg-1")` are awaited
- **THEN** the results are `Ok(true)`, `Ok(())`, and `Ok(false)`

#### Scenario: clear deletes only the idempotent repository prefix

- **GIVEN** a Redis server holding keys `camel:idem:default:a` (this repository) and
  `camel:cache:default:b` (a cache repository on the same server)
- **WHEN** the idempotent repository's `clear()` is awaited
- **THEN** `camel:idem:default:a` no longer exists and `camel:cache:default:b` still
  exists

#### Scenario: idempotent repository name with glob metacharacters rejected at construction

- **GIVEN** a `RedisIdempotentRepository` construction attempt with repository name
  `"my*idem"` (glob metacharacter in the name, which becomes part of every SCAN
  pattern)
- **WHEN** the constructor runs
- **THEN** it returns `Err(CamelError::Config)` naming the repository name and the
  allowed charset

#### Scenario: redis registered when configured

- **GIVEN** a `CamelConfig` whose `idempotent_repo` has `backend = "redis"` and a
  reachable `url`
- **WHEN** the context is built from that config
- **THEN** a repository is resolvable by name `"redis"` and a repository is still
  resolvable by name `"memory"`

#### Scenario: idempotent redis validation mirrors the cache matrix

- **GIVEN** a `CamelConfig` whose `idempotent_repo` has `backend = "redis"` with
  `sentinel_nodes = ["s-a:26379"]` and no `master_name`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `idempotent_repo.master_name`

#### Scenario: idempotent credentials redacted from Debug output

- **GIVEN** an `IdempotentRepoConfig` with
  `url = "redis://user:secret@host:6379"` and `sentinel_password = "hunter2"`
- **WHEN** the struct is formatted with `{:?}`
- **THEN** the output contains neither `secret` nor `hunter2`

#### Scenario: idempotent data credentials reach the endpoint in sentinel mode

- **GIVEN** a `CamelConfig` whose `idempotent_repo` has `backend = "redis"`,
  `sentinel_nodes`, `master_name`, `password = "idem-secret"`, and `db = 3`
- **WHEN** the redis endpoint is constructed from that config
- **THEN** the endpoint carries the data-node password and database 3 for the
  master/replica connection

#### Scenario: idempotent data credentials rejected on redb backend

- **GIVEN** a `CamelConfig` whose `idempotent_repo` has `backend = "redb"`,
  a `path`, and `password = "x"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `idempotent_repo.password`

#### Scenario: idempotent data credentials rejected in url mode

- **GIVEN** a `CamelConfig` whose `idempotent_repo` has `backend = "redis"`,
  `url = "redis://h:6379"`, and `db = 1`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `idempotent_repo.db`

#### Scenario: idempotent data credentials redacted from Debug output

- **GIVEN** an `IdempotentRepoConfig` in sentinel mode with
  `password = "idem-secret"` and `username = "idem-user"`
- **WHEN** the struct is formatted with `{:?}`
- **THEN** the output contains neither `idem-secret` nor `idem-user`

