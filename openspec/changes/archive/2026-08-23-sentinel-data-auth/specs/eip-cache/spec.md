# eip-cache delta — sentinel-data-auth

## MODIFIED Requirements

### Requirement: cache_repo Camel.toml configuration

The system SHALL accept a `cache_repo: Option<CacheRepoConfig>` field on `CamelConfig`,
mirroring the existing `idempotent_repo: Option<IdempotentRepoConfig>` field. The
`CacheRepoConfig` SHALL carry a `backend: "memory" | "redb" | "redis"` discriminator (default
`"memory"`) and backend-specific sub-fields. When `backend = "redb"`, the context builder
SHALL register a `RedbCacheRepository` under the name `"persistent"` in addition to the
default `"memory"`. When `backend = "redis"`, the context builder SHALL register a
`RedisCacheRepository` under the name `"redis"` in addition to the default `"memory"`. When
`backend = "memory"` or `cache_repo` is unset, only `"memory"` is
registered. The configuration SHALL be expressible via the profile section
`[default.cache_repo]` (mirrors `[default.idempotent_repo]`), SHALL carry `path`,
`stale_retention`, and an optional `max_entries` cap (default 1_000_000 entries) for the
redb backend, SHALL carry `max_capacity` for the memory backend, and SHALL fail validation
when `backend = "redb"` and `path` is empty.
For the redb backend it SHALL also carry `cache_size` — a **required** byte-size string
such as `"384MB"` or `"512MiB"` (plain integers mean bytes; decimal suffixes are powers
of 10^3, binary suffixes powers of 2^10; overflowing values are rejected) that bounds the
redb page cache — and optional `sweep_interval`, a humantime string such as `"30m"`
(default `"1h"`, values of zero rejected). Malformed `cache_size`, `sweep_interval`, or
`stale_retention` values SHALL fail validation with an error naming the offending field —
silent fallback to a default is forbidden. A redb-backend config without `cache_size`
SHALL fail validation with an error naming the field and suggesting example values.
The EFFIS anchor case configures persistence with:
`[default.cache_repo] backend = "redb"`, `path = "data/cache.redb"`,
`stale_retention = "168h"`, `cache_size = "256MiB"`.
The redis backend SHALL carry `url` (standalone endpoint), OR the pair
`sentinel_nodes` + `master_name` (sentinel topology), an optional `key_prefix`
(default `"camel:cache"`, restricted to the charset `[A-Za-z0-9:_-]`), and the same
`stale_retention` field as redb. In sentinel mode the redis backend SHALL
additionally carry data-node credentials `password` and `username` (both
optional `Option<String>`; they authenticate the client against the
master/replicas, NOT against the sentinels — sentinel credentials remain
`sentinel_username`/`sentinel_password`) and an optional `db` of type `Option<u16>`
(validated 0..=16383, default 0) selecting the logical database on the
data connection. Validation SHALL reject, with an error naming the
dotted field (`cache_repo.<field>`) and the violated rule: `backend = "redis"` with
both `url` and `sentinel_nodes`; with neither `url` nor `sentinel_nodes`; with an
empty `sentinel_nodes` list or any empty node entry; with `sentinel_nodes` and an
empty `master_name`; with `master_name`, `sentinel_username`, or `sentinel_password`
set but no `sentinel_nodes`; with `password`, `username`, or `db` set but no
`sentinel_nodes` (in `url` mode the password and database selection ride
the URI — userinfo password and `?db=N`; username in the URI is out of
scope); with a `db` outside 0..=16383; with a `url` scheme other than `redis://` or
`rediss://`; with any cluster topology fields (cluster mode is not supported for
repository backends); with a `key_prefix` that is empty or contains characters
outside `[A-Za-z0-9:_-]` (glob metacharacters would break prefix-scoped `clear`);
with a `stale_retention` that fails duration parsing; and, when the idempotent repo
is also `backend = "redis"` on the same effective endpoint and database, with an
effective prefix identical to the idempotent repository's (the effective
endpoint identity SHALL include the sentinel-mode `db`, so db 2 vs db 3 on
the same sentinel topology and prefix do not collide). Fields that do not
apply to the configured `backend` SHALL be rejected at validation with an
error naming `cache_repo.<field>` (fail-closed): `backend = "redis"` rejects
`path`, `cache_size`, `sweep_interval`, `max_entries`, and `max_capacity`;
`backend = "memory"` and `"redb"` reject `url`, `sentinel_nodes`,
`master_name`, `sentinel_username`, `sentinel_password`, `key_prefix`,
`password`, `username`, and `db`.
Foreign fields SHALL NOT be required. The `CacheRepoConfig`
`Debug` output SHALL redact credentials: URL userinfo, sentinel
username/password, and the data-node `password` and `username` SHALL NOT
appear in formatted output.

#### Scenario: redb registered when backend = redb

- **GIVEN** a `CamelConfig` whose `cache_repo` field has `backend = "redb"`, a path, a
  retention, a cap, and a cache size
- **WHEN** the context is built from that config
- **THEN** a cache repository is resolvable by name `"persistent"` and a repository is
  still resolvable by name `"memory"`

#### Scenario: redb absent when backend = memory or cache_repo unset

- **GIVEN** a `CamelConfig` with `cache_repo` unset, OR with `backend = "memory"`
- **WHEN** the context is built from that config
- **THEN** no cache repository is resolvable by name `"persistent"` and a repository is
  resolvable by name `"memory"`

#### Scenario: empty redb path rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"` and `cache_repo.path` is empty
- **WHEN** the config is validated
- **THEN** validation returns an error naming the offending field

#### Scenario: memory max_capacity supplied via config

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` and
  `cache_repo.max_capacity = 5000`
- **WHEN** the context is built from that config
- **THEN** the `"memory"` cache repository is constructed with `max_capacity = 5000`

#### Scenario: cache_size and sweep_interval reach the redb backend

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redb"`, a path,
  `cache_size = "512MiB"`, and `sweep_interval = "30m"`
- **WHEN** the context is built from that config
- **THEN** the `"persistent"` repository is constructed with a recorded cache size of
  536870912 bytes and a sweep interval of 30 minutes

#### Scenario: missing cache_size on redb backend rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"` with a path and a
  retention but no `cache_size`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.cache_size` and suggesting
  example values, and no repository is constructed

#### Scenario: malformed cache_size rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.cache_size = "thirty"` (unparseable)
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.cache_size`

#### Scenario: overflowing cache_size rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.cache_size = "18446744073709551616B"`
  (2^64 bytes — exceeds `usize` on every supported architecture, with a valid suffix)
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.cache_size`

#### Scenario: malformed sweep_interval rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.sweep_interval = "1x"` (unparseable)
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.sweep_interval`

#### Scenario: zero sweep_interval rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.sweep_interval = "0s"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.sweep_interval` and stating
  that the interval must be positive

#### Scenario: malformed stale_retention rejected at validation

- **GIVEN** a `CamelConfig` whose `cache_repo.stale_retention = "forever-ish"` (unparseable)
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.stale_retention` — the 7-day
  silent fallback no longer applies to malformed values

#### Scenario: sweep_interval defaults to one hour when unset

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redb"`, a path, a
  retention, and a `cache_size`, with no `sweep_interval`
- **WHEN** the context is built from that config
- **THEN** the sweep task runs at a one-hour interval, matching the previously shipped
  hardcoded cadence

#### Scenario: redis registered when backend = redis

- **GIVEN** a `CamelConfig` whose `cache_repo` field has `backend = "redis"` and a
  reachable `url`
- **WHEN** the context is built from that config
- **THEN** a cache repository is resolvable by name `"redis"` and a repository is still
  resolvable by name `"memory"`

#### Scenario: sentinel-selected redis backend registered

- **GIVEN** a `CamelConfig` whose `cache_repo` field has `backend = "redis"`,
  `sentinel_nodes = ["s-a:26379", "s-b:26379"]`, and `master_name = "orders"`
- **WHEN** the context is built from that config against a live sentinel topology
- **THEN** a cache repository is resolvable by name `"redis"`

#### Scenario: sentinel without master_name rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"` and
  `sentinel_nodes` set but no `master_name`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `master_name`

#### Scenario: url and sentinel_nodes together rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`, a `url`, and
  `sentinel_nodes`
- **WHEN** the config is validated
- **THEN** validation returns an error stating the two are mutually exclusive

#### Scenario: no topology rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"` with neither
  `url` nor `sentinel_nodes`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo` and stating that a
  topology (`url` or `sentinel_nodes`) is required

#### Scenario: orphan sentinel master_name rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`, a `url`, and
  a `master_name` but no `sentinel_nodes`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.master_name` and stating
  it requires `sentinel_nodes`

#### Scenario: invalid url scheme rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"` and
  `url = "http://cache.internal:6379"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.url` and stating only
  `redis://` and `rediss://` are accepted

#### Scenario: glob metacharacters in key_prefix rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"` and
  `key_prefix = "camel:*"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.key_prefix` and stating
  the allowed charset

#### Scenario: empty sentinel_nodes list or entry rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"` and
  `sentinel_nodes = []`, or `sentinel_nodes = ["s-a:26379", ""]`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.sentinel_nodes` and stating
  entries must be non-empty

#### Scenario: empty master_name with sentinel_nodes rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`,
  `sentinel_nodes = ["s-a:26379"]`, and `master_name = ""`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.master_name`

#### Scenario: orphan sentinel credentials rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`, a `url`, and
  `sentinel_password = "x"` but no `sentinel_nodes`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.sentinel_password` and
  stating it requires `sentinel_nodes`

#### Scenario: shared-database prefix collision rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` and `idempotent_repo` are both
  `backend = "redis"` on the same `url` and database, both with
  `key_prefix = "camel:shared"`
- **WHEN** the config is validated
- **THEN** validation returns an error stating the effective prefixes must be
  distinct

#### Scenario: credentials redacted from Debug output

- **GIVEN** a `CacheRepoConfig` with `url = "redis://user:secret@host:6379"` and
  `sentinel_password = "hunter2"`
- **WHEN** the struct is formatted with `{:?}`
- **THEN** the output contains neither `secret` nor `hunter2`

#### Scenario: data credentials reach the endpoint in sentinel mode

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`,
  `sentinel_nodes`, `master_name`, `password = "master-secret"`,
  `username = "svc"`, and `db = 2`
- **WHEN** the redis endpoint is constructed from that config
- **THEN** the endpoint carries the data-node username and password and
  database 2 for the master/replica connection, distinct from any sentinel
  credentials

#### Scenario: data credentials rejected in url mode

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`,
  `url = "redis://h:6379"`, and `password = "x"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.password` (credentials
  ride the URI — userinfo password and `?db=N`; username in the URI is
  out of scope)

#### Scenario: db out of range rejected

- **GIVEN** a `CamelConfig` whose `cache_repo` has `backend = "redis"`,
  `sentinel_nodes`, `master_name`, and `db = 20000`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.db`

#### Scenario: sentinel db participates in the prefix-collision identity

- **GIVEN** two redis repo configs on the same sentinel topology and master
  with identical effective prefixes but `db = 2` and `db = 3`
- **WHEN** the config is validated
- **THEN** validation succeeds (different logical databases do not collide);
  the same config with both `db = 2` fails with the prefix-collision error

#### Scenario: data credentials redacted from Debug output

- **GIVEN** a `CacheRepoConfig` in sentinel mode with
  `password = "master-secret"` and `username = "svc-user"`
- **WHEN** the struct is formatted with `{:?}`
- **THEN** the output contains neither `master-secret` nor `svc-user`
