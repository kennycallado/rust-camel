## MODIFIED Requirements

### Requirement: RedbCacheRepository opt-in persistent backend

The system SHALL provide a `RedbCacheRepository` in `camel-core` that implements
`CacheRepository` by persisting `CacheEntry` values (with their in-band `expires_at`) to a
redb file on disk, surviving process restart. Every trait operation SHALL wrap blocking
redb I/O in `tokio::task::spawn_blocking` and SHALL map redb errors to `CamelError::Io`,
satisfying Contract C1. A background sweep task SHALL remove entries whose
`expires_at + stale_retention` has elapsed; the task SHALL bind to the context's
`CancellationToken` so it stops cleanly on shutdown. The constructor SHALL take a
**required** cache size in bytes (`usize`) and SHALL open the database through
`redb::Builder` with `set_cache_size(bytes)` — redb's own default cache size (currently
1GiB) SHALL NOT be reachable through any code path. The repository SHALL record the
configured cache size in a field observable to in-crate tests as the propagation seam.

#### Scenario: entries survive handle drop and reopen

- **GIVEN** a `RedbCacheRepository` opened on file `X` with `set("k", entry, Some(1h))`
- **WHEN** the handle is dropped and a new `RedbCacheRepository` is opened on the same file `X`
- **THEN** `get("k")` on the new handle returns `Ok(Some(entry))`

#### Scenario: peek_stale returns post-expiry entry on redb

- **GIVEN** a `RedbCacheRepository` with `set("k", entry, Some(1ms))` and 10ms elapsed but
  within `stale_retention`
- **WHEN** `peek_stale("k")` is called
- **THEN** it returns `Ok(Some(entry))` (sweep has not yet reclaimed it)

#### Scenario: sweep removes entries past stale_retention

- **GIVEN** a `RedbCacheRepository` whose sweep interval has fired and whose entry `"k"` is
  past `expires_at + stale_retention`
- **WHEN** `peek_stale("k")` is called after sweep
- **THEN** it returns `Ok(None)`

#### Scenario: sweep stops on context shutdown

- **GIVEN** a `RedbCacheRepository` whose sweep task is running and bound to a
  `CancellationToken`
- **WHEN** the token is cancelled
- **THEN** the sweep task exits within a bounded grace period and no sweep task lingers

#### Scenario: redb errors surface as Contract C1 Err

- **GIVEN** a `RedbCacheRepository` whose backing file has been removed beneath it
- **WHEN** `get("k")` is called
- **THEN** the result is `Err(CamelError::Io(..))` and is NOT `Ok(None)`

#### Scenario: configured cache size is observable on the repository

- **GIVEN** a `RedbCacheRepository` constructed with `cache_size = 536870912`
- **WHEN** an in-crate test reads the repository's recorded cache size field
- **THEN** it equals 536870912 (the propagation seam proving the value reached the
  repository that owns the `Builder::set_cache_size` call)

#### Scenario: explicit cache size supports the full round-trip

- **GIVEN** a `RedbCacheRepository` constructed with an explicit cache size
- **WHEN** `set("k", entry, Some(1h))` then `get("k")` are called
- **THEN** the round-trip succeeds on the database opened through the builder

### Requirement: cache_repo Camel.toml configuration

The system SHALL accept a `cache_repo: Option<CacheRepoConfig>` field on `CamelConfig`,
mirroring the existing `idempotent_repo: Option<RedbIdempotentConfig>` field. The
`CacheRepoConfig` SHALL carry a `backend: "memory" | "redb"` discriminator (default
`"memory"`) and backend-specific sub-fields. When `backend = "redb"`, the context builder
SHALL register a `RedbCacheRepository` under the name `"persistent"` in addition to the
default `"memory"`. When `backend = "memory"` or `cache_repo` is unset, only `"memory"` is
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

## ADDED Requirements

### Requirement: Redb cache container memory guardrail

At redb cache repository open, the system SHALL detect the container memory limit from
cgroup v2 (`/sys/fs/cgroup/memory.max`) with cgroup v1
(`/sys/fs/cgroup/memory/memory.limit_in_bytes`) as fallback, treating `"max"`, v1
sentinel values above 16 TiB, and unparseable content as no-limit (falling through
without error). When the effective cache size (the configured `cache_size`; there is no
unset case, since `cache_size` is required) exceeds the detected limit, the system SHALL
emit a single startup warning naming both the effective cache size and the container
limit. When no limit is detectable or the cache size fits within the limit, the system
SHALL stay silent. The guardrail SHALL be diagnostic only: it SHALL never fail
repository construction. Tests SHALL exercise the check through parameterized cgroup
file paths (temporary files) and captured tracing output — never a real cgroup.

#### Scenario: warning when cache size exceeds cgroup limit

- **GIVEN** cgroup files reporting a memory limit of 768MiB and a configured
  `cache_size` of 1GiB
- **WHEN** the limit check runs against those paths
- **THEN** the check reports that 1073741824 exceeds 805306368, and the startup warning
  is emitted with both numbers in the captured tracing output

#### Scenario: cgroup v2 unlimited reports no limit

- **GIVEN** a cgroup v2 `memory.max` file containing `max`
- **WHEN** the limit is read from that path
- **THEN** no limit is reported from v2 and detection falls through to the v1 path

#### Scenario: cgroup v1 sentinel treated as unlimited

- **GIVEN** a cgroup v1 `memory.limit_in_bytes` file containing `9223372036854771712`
- **WHEN** the limit is read from that path
- **THEN** the value is treated as unlimited (no limit reported)

#### Scenario: malformed cgroup content falls through without error

- **GIVEN** a cgroup v2 `memory.max` file containing `not-a-number`
- **WHEN** the limit is read from that path
- **THEN** no limit is reported from v2, no error is raised, and detection falls through
  to the v1 path

#### Scenario: silent when cache size fits

- **GIVEN** a detected memory limit of 768MiB and a configured `cache_size = "256MiB"`
- **WHEN** the limit check runs
- **THEN** no warning is emitted

#### Scenario: missing cgroup files are not an error

- **GIVEN** neither cgroup path exists
- **WHEN** the limit check runs
- **THEN** repository construction proceeds normally with no warning and no error
