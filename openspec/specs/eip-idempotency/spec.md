# eip-idempotency Specification

## Purpose
Persistent idempotent repositories. A redb-backed repository persists keys
across restart and stays opt-in under the name `"redb"`, memory remains the
default, and a Redis backend is available in the repository service crate.
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

### Requirement: Idempotent consumer step reservation semantics

The `idempotent_consumer` step SHALL treat an absent or empty message-id as a missing key and SHALL resolve it according to an `on_missing_key` setting with values `fail` and `process`, defaulting to `fail`, on both the synchronous and asynchronous arms of the step. Under `fail`, the step SHALL fail the exchange with an error naming the step and the repository, and SHALL NOT run the child sub-pipeline. Under `process`, the step SHALL log a warning and forward the exchange to the child sub-pipeline without repository interaction. An empty string key SHALL be treated identically to an absent key, and the synchronous arm SHALL NOT register an empty key in the repository.

#### Scenario: sync arm, absent key, fail is the default

- **GIVEN** a route with an `idempotent_consumer` step that omits `on_missing_key` and whose synchronous key extractor returns nothing
- **WHEN** such an exchange is processed
- **THEN** the outcome is `Failed` with an error naming `idempotent_consumer` and the repository, and the child sub-pipeline does not run

#### Scenario: async arm, absent key, fail is the default

- **GIVEN** a route with an `idempotent_consumer` step that omits `on_missing_key` and whose asynchronous key expression evaluates to null
- **WHEN** such an exchange is processed
- **THEN** the outcome is `Failed` with an error naming `idempotent_consumer` and the repository, and the child sub-pipeline does not run

#### Scenario: missing-key process forwards with a warning on both arms

- **GIVEN** an `idempotent_consumer` step configured with `on_missing_key: process`, tested once with a synchronous extractor returning nothing and once with an asynchronous expression evaluating to null
- **WHEN** each exchange is processed
- **THEN** a warning is logged in both cases, the child sub-pipeline runs in both cases, and the repository is never consulted for those exchanges

#### Scenario: empty key is treated as absent on the sync arm

- **GIVEN** an `idempotent_consumer` step whose synchronous key extractor returns `Some("")` for a message
- **WHEN** that message is processed under the default `on_missing_key: fail`
- **THEN** the step behaves exactly as if the key were absent (the exchange fails under `fail`), and no empty-string key is ever registered in or read from the repository

#### Scenario: empty key is treated as absent on the async arm

- **GIVEN** an `idempotent_consumer` step whose asynchronous key expression evaluates to an empty string
- **WHEN** that message is processed under `on_missing_key: process`
- **THEN** the child sub-pipeline runs and the repository is not consulted, identically to an absent key

### Requirement: DSL surface for on_missing_key

The `idempotent_consumer` DSL step SHALL accept an optional `on_missing_key` field with the string values `fail` and `process`; an absent field SHALL default to `fail`; an invalid value SHALL be rejected at parse time with an error naming the field; an unknown sibling field SHALL be rejected by `deny_unknown_fields`.

#### Scenario: valid values parse

- **GIVEN** two routes whose `idempotent_consumer` steps set `on_missing_key: "fail"` and `on_missing_key: "process"`
- **WHEN** each route is parsed
- **THEN** parsing succeeds and the compiled step carries the corresponding `OnMissingKey` value

#### Scenario: invalid value is rejected at parse time

- **GIVEN** a route whose `idempotent_consumer` step sets `on_missing_key: "skip"`
- **WHEN** the route is parsed
- **THEN** parsing fails with an error naming `on_missing_key` and the accepted values

#### Scenario: default is fail

- **GIVEN** a route whose `idempotent_consumer` step omits `on_missing_key`
- **WHEN** the route is compiled
- **THEN** the compiled step carries `on_missing_key = fail`

#### Scenario: unknown field is rejected

- **GIVEN** a route whose `idempotent_consumer` step sets an unknown field `on_duplicate: true`
- **WHEN** the route is parsed
- **THEN** parsing fails with an unknown-field error

### Requirement: Eager reservation rolls back by default

The system SHALL default `remove_on_failure` to `true` when `eager` is enabled, so a failed child sub-pipeline rolls back the eager reservation and a route-level retry re-executes the child. An explicit `remove_on_failure: false` SHALL remain an opt-in at-most-once contract and SHALL raise a load-time warning stating that semantic.

#### Scenario: eager rollback default

- **GIVEN** an `idempotent_consumer` step with `eager: true` and no `remove_on_failure` field, on a route with retries
- **WHEN** the child sub-pipeline fails and the same key is delivered again
- **THEN** the reservation is removed, the retry runs the child sub-pipeline again, and the route does not report success with zero work

#### Scenario: explicit at-most-once opt-in warns at load time

- **GIVEN** an `idempotent_consumer` step with `eager: true` and explicit `remove_on_failure: false`
- **WHEN** the route is compiled
- **THEN** a load-time warning naming the step and the at-most-once contract is emitted, and on a failed child the reservation is kept so the retry skips the child as an observable duplicate

### Requirement: Failed rollback fails the exchange and recovery is single-owner

The step SHALL fail the exchange when an eager rollback fails. The step SHALL return the child's error unchanged as the primary error so that structural `on_exceptions` retry predicates continue to match the child's error variant, and SHALL attach the rollback failure as structured context (an error-level log carrying both errors). The step SHALL record the key in a pending-rollback state shared across clones of the segment. Ownership of a pending key SHALL be taken atomically under the shared lock, and an in-flight marker SHALL span the recovery from ownership take until the key's reservation step resolves: in eager mode until the pre-child reservation resolves, in lazy mode until the post-child bookkeeping resolves — and the marker SHALL be cleared whenever the reservation step resolves, so a settled key never remains in flight. A delivery whose key is in-flight SHALL fail loudly with an error naming the step, the repository, and the recovery-in-progress reason, without repository operations and without running the child. The recovery owner SHALL retry the repository removal: on failure it SHALL return the key to the pending state and fail the exchange with the removal error; on success it SHALL proceed to reserve and run the child exactly once for the key. The step SHALL NOT use redelivery headers to drive this rule.

#### Scenario: failed rollback fails the exchange and preserves the child error type

- **GIVEN** an `idempotent_consumer` step with `eager: true` whose repository removal fails when the child fails
- **WHEN** the child sub-pipeline fails with an `Io` error and the removal also fails
- **THEN** the outcome is `Failed` with a `CamelError::Io` matching the child's error variant, an error-level log carries both the child error and the rollback error, and the key is recorded in the pending-rollback state

#### Scenario: pending removal fails again

- **GIVEN** a pending-rollback key whose repository removal fails again on the next delivery
- **WHEN** a new delivery with that key arrives and the recovery removal fails
- **THEN** the outcome is `Failed` with the removal error, the key returns to the pending state, and the repository contains/add operations are not consulted and the child does not run

#### Scenario: pending rollback is retried before the reserve on re-entry

- **GIVEN** a pending-rollback key after a failed removal
- **WHEN** a new delivery with the same key arrives and the repository removal now succeeds
- **THEN** the step reserves the key and runs the child sub-pipeline exactly once — the delivery is never reported as a duplicate or a success with zero work

#### Scenario: raced delivery during removal, before reservation, fails loudly

- **GIVEN** a pending-rollback key whose recovery owner has taken ownership but has not yet completed the repository removal, and a second clone of the segment sharing the state
- **WHEN** the second clone processes a delivery with the same key while the removal is in progress
- **THEN** the second clone's outcome is `Failed` naming the recovery-in-progress reason, with no repository contains/add/remove performed by that clone and no child run

#### Scenario: raced delivery after removal, before reservation, fails loudly

- **GIVEN** a pending-rollback key whose recovery owner has completed the repository removal successfully but has not yet resolved its reservation, and a second clone of the segment sharing the state
- **WHEN** the second clone processes a delivery with the same key inside that window
- **THEN** the second clone's outcome is `Failed` naming the recovery-in-progress reason, with no repository contains/add performed by that clone and no child run, and the recovery owner then reserves and runs the child exactly once — this window is where premature clearing of the in-flight marker would let the second clone reserve and win the race

#### Scenario: raced delivery during in-flight recovery fails loudly

- **GIVEN** a pending-rollback key whose recovery owner has taken ownership but has not yet completed the removal-to-reservation span, and a second clone of the segment sharing the state
- **WHEN** the second clone processes a delivery with the same key while recovery is in flight
- **THEN** the second clone's outcome is `Failed` naming the recovery-in-progress reason, with no repository contains/add/remove performed by that clone and no child run, the recovery owner completes the removal, reserves, and runs the child exactly once, and no delivery with that key is reported as `Completed` with zero work

### Requirement: Duplicate skips and bookkeeping failures are observable

The step SHALL set the exchange property `camel.idempotent.duplicate` to `true` and SHALL increment the counter `camel.idempotent.duplicates` (label `repository`) whenever the child sub-pipeline is skipped as a duplicate, on both the contains-hit path and the eager race path. A failed non-eager repository add after a successful child SHALL keep the exchange successful, log at error level, and increment the counter `camel.idempotent.add_failures` (label `repository`).

#### Scenario: duplicate skip sets property and counter

- **GIVEN** an `idempotent_consumer` step whose repository already contains the key
- **WHEN** an exchange with that key is processed
- **THEN** the child sub-pipeline is skipped, the exchange carries `camel.idempotent.duplicate = true`, and the counter `camel.idempotent.duplicates` with the repository label is incremented

#### Scenario: eager race duplicate is observable

- **GIVEN** an `idempotent_consumer` step with `eager: true` where a concurrent delivery already reserved the key
- **WHEN** the step's reservation add reports the key already exists
- **THEN** the child sub-pipeline is skipped, the exchange carries `camel.idempotent.duplicate = true`, and the counter `camel.idempotent.duplicates` is incremented

#### Scenario: non-eager add failure keeps success with an error log

- **GIVEN** an `idempotent_consumer` step with `eager: false` whose repository add fails after the child completed
- **WHEN** the child sub-pipeline completes successfully and the post-success add fails
- **THEN** the exchange stays successful, an error-level log is emitted, and the counter `camel.idempotent.add_failures` with the repository label is incremented

