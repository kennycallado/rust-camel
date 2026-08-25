# camel-redis-repo

Redis-backed implementations of the `camel-api` repository traits:
`RedisIdempotentRepository` and `RedisCacheRepository`. Charter: ADR-0063.

## Scope boundary

This crate is a repository service, not a Component. It owns no URI scheme,
creates no Endpoints, and registers no Consumer or Producer. The Components
directory charter and its component-specific lints do not apply here.

The crate implements two `camel-api` ports. Route steps resolve the
repositories by name from `CamelContext` during Exchange processing.
`camel-config` registers them at context build time when
`[default.cache_repo]` or `[default.idempotent_repo]` selects
`backend = "redis"`.

## Language

Canonical language is English. Prose follows the project STE rules.
Use "repository service" or "repository backend" for this crate. Do not use
the unqualified term "service". The parent glossary
(`crates/services/CONTEXT.md`) owns the term definitions.

## Connection and retry ownership

Each repository owns one `MultiplexedExecutor` built from its own endpoint
configuration. Two repositories that target the same server hold two
multiplexed connections. There is no cross-repository connection registry.

Construction resolves the topology once and connects eagerly, so an
unreachable topology fails fast at context build
(`src/connection.rs`). Sentinel failover surfaces only through an explicit
`refresh` after a failed command. Retry-safe operations (`GET`, `EXISTS`,
`UNLINK`, `SCAN`, plain `SET`) may refresh and re-issue at most once.

Every command carries a 30-second response-timeout backstop; the
connection's driver-level response deadline is set to 35 seconds
(30 s backstop + 5 s margin, ADR-0063 Decision 13 as amended by change
`redis-response-timeout`), so the LOCAL backstop fires first and governs
classification — the driver deadline is defense-in-depth only. Either
deadline surfaces as transient `Io` that triggers re-resolve on
the next call (`src/executor.rs`, `src/connection.rs`, e_opus review I1).
Credentials
are captured at construction: `refresh` re-resolves the topology but not
credentials, so password rotation requires a context rebuild.

Idempotent `add` is the exception. It issues exactly one `SET NX` per trait
call and never re-issues it after a transport error. A lost response leaves
the outcome unknown: the first insert may have succeeded on the server, and a
retry could reject a first-seen message as a duplicate. On any transport
error, `add` returns `Err(CamelError::Io)` immediately (Contract C1,
ADR-0023) and refreshes the connection only for later calls.

All commands travel as `redis::Cmd` values through the narrow
`trait RepoCommandExecutor` seam (`src/executor.rs`). `MultiplexedRepoExecutor`
is the production implementation; it maps every connection failure to
`CamelError::Io`, so transport failures classify as `"io"` for observability.
The crate produces no `ProcessorError`. Setup and validation failures map to
`CamelError::Config`.

## Sentinel feature posture

The crate enables `camel-component-redis/sentinel` unconditionally. Sentinel
support is therefore always compiled, in this crate and in every binary that
links it together with `camel-config` (Cargo feature unification). The
component's own `sentinel` feature stays default-off and fail-closed; that
gate still governs dependency graphs that do not link this crate.

Selection is explicit and fail-closed: `sentinel_nodes` plus `master_name`
routes to the Sentinel topology. `sentinel_nodes` and `cluster_nodes` are
mutually exclusive, a missing `master_name` under `sentinel_nodes` is
rejected at startup, and cluster topologies are rejected outright because the
repositories assume single-key routing. No runtime probe infers the topology
(ADR-0033, ADR-0032).

## Keyspace and clear() safety

Keys are namespaced `{prefix}:{repo}:{key}` by `keyspace::namespaced`
(`src/keyspace.rs`). Every namespace token is validated by
`fn validate_namespace_token` against `[A-Za-z0-9:_-]` before use. Glob
metacharacters are rejected as `CamelError::Config` before any SCAN runs.

`clear()` and `invalidate_prefix` walk prefix-scoped `SCAN` + batched
`UNLINK` under the repository namespace. `FLUSHDB` and `FLUSHALL` are
forbidden: a shared Redis deployment would lose every other tenant's data.
The step prefix that `cache_invalidate { key_prefix }` passes to
`invalidate_prefix` is exchange data, so it passes the same charset guard
before it enters the SCAN pattern (ADR-0032 trust boundary).

## Credential redaction

Sentinel node credentials and connection URLs follow ADR-0051. Redaction
lives in the component's topology code. This crate adds no new
credential-bearing `Debug` output.

## Test seams

- `FakeRepoExecutor` records scripted `redis::Cmd` results and counts
  refreshes, so repository logic is unit-tested without a live Redis
  (`src/executor.rs`).
- `FakeStaticTopology` feeds a fixed address to connection construction.
- Live coverage runs in `crates/camel-test/tests/redis_repositories_test.rs`
  under the `integration-tests` feature (testcontainers, no `#[ignore]`
  attributes, ADR-0054).

## Dependency boundary

Direct dependencies: `camel-api` (ports and `CamelError`),
`camel-component-redis` (connection seam: `MultiplexedExecutor`,
`topology_from_config`), and the `redis` crate as the protocol driver. The
crate sends raw `redis::Cmd` values; no project-owned adapter trait wraps
redis-rs, following the same posture as `crates/components/camel-redis`
(ADR-0020 does not govern this boundary).

## Lifecycle and crash ownership

The repositories implement no `Lifecycle` and no `StepLifecycle`. They hold
no Consumer task and no background task. Reclamation is server-side through
`EXAT`; there is no sweep loop. Dropping the repository owns connection
cleanup. ADR-0028's guidance to implement `StepLifecycle` on a backend client
does not apply: the multiplexed connection carries no timers or queues, so
no connection lifecycle exists to manage (ADR-0063 Decision 12).
