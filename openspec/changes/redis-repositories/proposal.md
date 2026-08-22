# Proposal: redis-repositories

## Why

The Cache EIP (ADR-0056) and the Idempotent Consumer EIP (ADR-0023) ship
memory and redb backends only. Both ADRs name Redis as an anticipated
persistent backend, but no Redis backend exists. A shared Redis gives both
EIPs cross-process state: many route instances, or many processes, can share
one cache and one dedup set. Redis Sentinel support already exists in the
`camel-redis` component (spec `redis-failover`), so a failover-capable
repository does not need new topology code.

## What Changes

- New repository service crate `crates/services/camel-redis-repo` implementing
  `CacheRepository` and `IdempotentRepository`, each over its own
  multiplexed Redis connection built from its own configuration.
- The service crate reuses the component's `RedisTopology`, `topology_from_config`,
  and `MultiplexedExecutor` seams and issues repository-level commands
  (`SCAN`, `UNLINK`, `SET NX`, `SET … EXAT`, `EXISTS`, `GET`, `DEL`) as
  `redis::Cmd` values through an internal executor seam; the production
  executor pipes each command over the `MultiplexedConnection` returned
  by `get_conn`. The `camel-redis`
  changes are additive visibility widenings: `topology_from_config`
  `pub(crate)` to `pub`, `MultiplexedExecutor::get_conn` `pub(crate)` to
  `pub`, and a new `pub async fn MultiplexedExecutor::refresh(&self)`
  reconnect primitive (an `&self` analogue of the existing `&mut self`
  `reconnect`). No behavior change; the `RedisCommand` dispatch path is
  untouched.
- `camel-config` (the composition root) accepts `backend = "redis"` for
  `cache_repo` and generalizes `idempotent_repo` to a backend-discriminator
  struct (default `"redb"` keeps existing TOML parsing), mirroring
  `CacheRepoConfig`. It registers the Redis repositories under the name
  `"redis"`.
- Sentinel is always compiled into the service crate (the crate enables
  `camel-component-redis/sentinel` and `redis/sentinel` unconditionally;
  feature unification means graphs linking the crate compile the component
  with sentinel support). Sentinel is selected by explicit
  `sentinel_nodes` + `master_name` fields. No runtime auto-detection. The
  component's fail-closed feature gate stays unchanged for builds that do
  not link the service crate.
- Cluster topology is rejected at config validation for repository backends
  in this change (cluster `SCAN` does not span slots; the component's
  cluster support is not production-ready either).
- Live coverage runs in `camel-test` with testcontainers (never
  `#[ignore]`, per ADR-0054): `tests/redis_repositories_test.rs` plus a CI
  target.
- Documentation lands in the same change: ADR-0063 (amends ADR-0023 and
  ADR-0056 placement decisions), crate `CONTEXT.md`, Services charter and
  CONTEXT-MAP updates, mdBook configuration/EIP pages, and a checked-in
  example.
- Idempotent key TTL stays out of scope, for parity with ADR-0023 and the
  redb backend.

Affected crates: `camel-redis-repo` (new, publishable service crate),
`camel-component-redis` (additive connection-seam widening: `get_conn`,
`refresh`, `topology_from_config`), `camel-config` (config + validation +
redaction + registration), `camel-test` (integration test + optional
dependency), `.github/workflows/ci.yml` (test target).

## Acceptance criteria

- `RedisCacheRepository` and `RedisIdempotentRepository` implement their
  traits with in-band expiry (cache), atomic `SET NX` (idempotent `add`),
  `SCAN`+`UNLINK` prefix-scoped `clear`, and Contract C1 error propagation
  (backend failure surfaces as `Err`, never as a silent miss).
- A `SET NX` that fails during a simulated failover returns `Err`, never
  `Ok(true)` or `Ok(false)`. The repository never re-issues `SET NX` after
  a transport error; it refreshes the connection for subsequent calls.
- Cache `set` issues a single `SET … EXAT (expires_at + retention)` command
  when `expires_at` is `Some`, and a plain `SET` when `None`.
- `clear()` on one repository deletes only keys under that repository's
  prefix; keys of the other repository survive.
- `[default.cache_repo] backend = "redis"` and
  `[default.idempotent_repo] backend = "redis"` each register a repository
  resolvable by name `"redis"`, with `"memory"` still the default when
  unset.
- Sentinel config (`sentinel_nodes` without `master_name`, or sentinel plus
  cluster) is rejected at validation with a clear error, along with the
  full fail-closed matrix (no topology, both topologies, empty values,
  orphan sentinel fields, invalid URL scheme, unsafe `key_prefix`
  charset, prefix collision on a shared database).
- Config `Debug` output redacts credentials (URL userinfo, sentinel
  username/password); `lint-secrets` passes.
- `camel-core` source is untouched; the hexagonal boundary test passes.

## Risk budget

Acceptable: small breaking change to the `idempotent_repo` config shape
(default `backend = "redb"` keeps existing TOML working); new public seam
on `camel-redis`.

Out of bounds: cluster mode for repositories; `FLUSHDB`/`FLUSHALL` under
any code path; runtime topology auto-detection; idempotent key TTL;
changes to `camel-core`.
