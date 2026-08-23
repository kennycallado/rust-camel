# eip-idempotency delta — sentinel-data-auth

## MODIFIED Requirements

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
