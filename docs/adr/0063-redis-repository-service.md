# ADR-0063: Redis Repository Service

**Date:** 2026-08-22
**Status:** Accepted
Amends: ADR-0023, ADR-0056
Cross-references: ADR-0028, ADR-0032, ADR-0033, ADR-0051, ADR-0054

## Decision

### Decision 1: repository service crate at `crates/services/camel-redis-repo`

The Redis backends ship as one new crate, `crates/services/camel-redis-repo`. The
crate implements `IdempotentRepository` and `CacheRepository` from `camel-api`.
It does not implement `Component`, own a URI scheme, or create Endpoints.

The Services family is the correct home. `camel-auth` already proves the family
holds named, context-scoped infrastructure that is not a `Lifecycle`
implementation. The repository backends share that shape: contract in
`camel-api`, implementation outside `camel-core`, named lookup in
`CamelContext`, and route steps that consume the lookup.

Repository reads and writes run during Exchange processing. Named registration
happens at context build time through `camel-config`. The crate differs from a
URI Component in contract and user-facing role, so the Components directory
charter and its component-specific lints must not apply.

### Decision 2: both repositories in one change

`RedisIdempotentRepository` and `RedisCacheRepository` ship together. They share
connection acquisition, key namespacing, error mapping, and the executor seam.
Splitting them would double the wiring and test overhead for zero isolation
benefit. The traits stay independent, so shipping together creates no coupling.

### Decision 3: connection seam through the component, commands as `redis::Cmd`

The service reuses the connection machinery of `camel-component-redis` instead
of building a second client. The component widens its surface additively:

- `MultiplexedExecutor::get_conn` moves from `pub(crate)` to `pub`
  (`crates/components/camel-redis/src/executor.rs:268`).
- A new `pub async fn refresh(&self)` reconnects without `&mut self`
  (`crates/components/camel-redis/src/executor.rs:313`).
- `topology_from_config` moves from `pub(crate)` to `pub`
  (`crates/components/camel-redis/src/topology.rs`).

The service issues repository commands (`SET NX`, `SET ... EXAT`, `EXISTS`,
`GET`, `UNLINK`, `SCAN`) as `redis::Cmd` values through its own narrow
`trait RepoCommandExecutor` (`crates/services/camel-redis-repo/src/executor.rs:46`).
The component's `RedisCommand` dispatch path stays untouched. No existing
component signature changes.

### Decision 4: one executor per repository, no registry

Each repository owns one `MultiplexedExecutor` built from its own configuration.
Two repositories that target the same server hold two multiplexed connections.
That costs one extra socket. A deduplication registry would need identity
equality over topology, credentials, TLS, and database index. It would also need
lifetime and eviction rules. Two possible repositories do not justify that
machinery. A registry, if ever wanted, is an additive future change with its own
spec.

### Decision 5: Sentinel always compiled in the service crate

The service enables `camel-component-redis/sentinel` unconditionally. Cargo
feature unification compiles the component's Sentinel branch into every binary
that links both the service crate and `camel-config`. The component's own
`sentinel` feature stays default-off and fail-closed. That gate still governs
builds whose dependency graph does not link the service crate. The asymmetry is
intentional and documented in the `redis-failover` spec delta.

### Decision 6: explicit config selection, no auto-detection

Sentinel routing is selected by explicit fields: `sentinel_nodes` plus
`master_name`. `sentinel_nodes` and `cluster_nodes` are mutually exclusive and
validated at startup. `master_name` missing under `sentinel_nodes` is rejected
at startup.

Runtime probing for "is this host a Sentinel" would infer topology from
unconfirmed signals. That inference contradicts ADR-0033 (fail closed, validate
at startup) and ADR-0032 (`master_name` is operator config, never inferred).
The probe also cannot produce a reliable positive signal. A Sentinel answers
`SENTINEL get-master-addr-by-name` only for a known master name. Port heuristics
are convention, not contract.

### Decision 7: `SET NX` is never re-issued (Contract C1)

Idempotent `add` issues exactly one `SET key 1 NX` per trait call. A lost
response leaves the outcome unknown: the first insert may have succeeded on the
server. A retry could observe the key it set itself and return `Ok(false)` for a
first-seen message. On any transport error, `add` returns
`Err(CamelError::Io)` immediately. The Idempotent Consumer treats `Err` as an
unknown outcome and retries or dead-letters, per ADR-0023 Contract C1.

The repository may call `refresh()` after the failure so later calls use a
healthy connection. Command re-issue for `add` is what is forbidden. Retry-safe
operations (`GET`, `EXISTS`, `UNLINK`, `SCAN`, plain `SET`) may refresh and
re-issue at most once, because a re-issued command cannot corrupt state.

### Decision 8: one atomic `SET ... EXAT` for cache writes

Cache `set` writes the serde_json `CacheEntry` with a single `SET`. When
`expires_at` is `Some`, the command carries `EXAT (expires_at + stale_retention)`
(`crates/services/camel-redis-repo/src/cache_repo.rs:111-123`). The server-side
deadline is garbage collection only. It extends past logical expiry so
`peek_stale` stays satisfiable inside the retention window, per ADR-0056
Decision 5. When `expires_at` is `None`, the command sets no Redis deadline: the
entry lives until invalidated.

Plain `SET` is last-writer-wins. A re-issued identical `SET` stores the same
bytes and the same `EXAT`, so a lost response on cache `set` is safe to retry.

### Decision 9: `clear()` and `invalidate_prefix` use prefix-scoped SCAN + UNLINK

Keys are namespaced `{prefix}:{repo}:{key}` by `keyspace::namespaced`
(`crates/services/camel-redis-repo/src/keyspace.rs:9`). `clear()` walks
`SCAN MATCH {prefix}:{repo}:*` and unlinks in batches. `invalidate_prefix`
walks `{prefix}:{repo}:{step_prefix}*` and returns the removed count.

`FLUSHDB` and `FLUSHALL` are forbidden. A shared Redis would lose every other
tenant's data. A prefix-scoped walk with batched `UNLINK` bounds the damage to
one repository namespace.

Every namespace token, including the step prefix from `cache_invalidate
{ key_prefix }`, is validated against `[A-Za-z0-9:_-]` before it enters a SCAN
pattern (`crates/services/camel-redis-repo/src/keyspace.rs:17`). Glob
metacharacters are rejected as `CamelError::Config` before any SCAN runs. The
step prefix is a simple-language expression resolved from exchange data, so this
guard is an ADR-0032 trust-boundary obligation.

### Decision 10: error mapping is `Io` for transient, `Config` for setup

Transient transport failures map to `CamelError::Io`
(`crates/services/camel-redis-repo/src/error.rs:7`). Setup and validation
failures map to `CamelError::Config`. The crate produces no `ProcessorError`.
`classify()` therefore keeps reporting `"io"` for transport failures, and a
failed read stays `Err`, never "absent", per Contract C1.

### Decision 11: cluster rejected; no idempotent TTL

Configuration that requests a cluster topology is rejected at validation with a
`Config` error. The repositories assume single-key routing. Cluster support, if
a need appears, is a separate change.

Idempotent keys carry no TTL. This matches ADR-0023 and the redb backend.
Camel-Java offers optional key expiry, but this project declared idempotent TTL
out of scope. A later TTL needs its own explicitly specced change.

### Decision 12: no `Lifecycle` implementation

The repositories implement no `Lifecycle` and no `StepLifecycle`. Construction
resolves the topology once and connects eagerly, so an unreachable topology
fails fast at context build. Sentinel failover is detected only through the
explicit `refresh` on a later error. No Consumer task and no background task
exist. Reclamation is server-side through `EXAT`. Dropping the repository owns
connection cleanup.

ADR-0028 states that a persistent backend which needs connection lifecycle
management should implement `StepLifecycle` on the backend client. That rule
does not apply here: the multiplexed connection carries no timers, buckets, or
queues, so no lifecycle exists to manage.

### Decision 13: per-command response timeout

Every repository command carries a 30-second response timeout
(`DEFAULT_RESPONSE_TIMEOUT`, `crates/services/camel-redis-repo/src/executor.rs`).
`connection_timeout_secs` guards only the TCP connect. Without an
execution-side bound, a half-open socket or a silent peer could park an
Exchange-processing future indefinitely and defeat refresh-on-error, because
the error never arrives. An elapsed timeout maps to transient
`CamelError::Io`, so retry-safe operations refresh and re-issue once, `add`
returns `Err` without a re-issue (Contract C1), and the next call re-resolves
the topology. The component's connection configuration is untouched: the
timeout wraps `query_async` inside `MultiplexedRepoExecutor::execute` in the
service crate, so component consumers keep their existing behavior. The redis
driver also enforces its own default response timeout on multiplexed
connections; the service-crate timeout is the crate's own contract and does
not depend on driver defaults. Plumbing the driver's response timeout
through the component landed (OpenSpec change `redis-response-timeout`,
bd rc-dq7a). `MultiplexedExecutor` now accepts
`with_response_timeout(Duration)`, applied in `get_conn` on the initial
connect and on every `refresh`/`reconnect` rebuild. On that branch the
driver's parallel 1 s connect default is disabled through
`set_connection_timeout(None)`, so the component's `connection_timeout_secs`
wrapper stays the sole connect bound. The repository service crate
constructs its executor with a 35 s driver response timeout (30 s backstop
plus 5 s margin), so the service crate's own 30 s contract governs end to
end and the driver deadline is defense-in-depth only. Component consumers
that do not call the builder keep the driver default. Their behavior does
not change.

## Rejected alternatives

### Module inside `camel-component-redis`

ADR-0023 and ADR-0056 anticipated Redis backends inside the Redis component,
following Apache Camel packaging. Rejected: the blessed crate keeps repository
APIs, unconditional Sentinel support, and the repository release surface
separate from URI endpoint behavior. It also avoids pushing the component's
optional `sentinel` feature onto component users who want no repository.

### New `crates/adapters/` family

Rejected: it creates a parent taxonomy, workspace globs, and documentation
context for exactly one crate. A family is created after the family exists. SQL
or Memcached repository backends, if they appear, land in Services under the
same rule.

### Shared connection registry

Rejected per Decision 4. One multiplexed connection per repository keeps
independently configured repositories isolated with a one-sentence ownership
rule and no invariants to police.

### Sentinel auto-detection

Rejected per Decision 6. Probing infers topology at runtime, adds a new failure
mode, and cannot work without the master name it claims to avoid.

### Retrying `SET NX`

Rejected per Decision 7. A retry can convert a first-seen message into a false
duplicate. The unknown outcome must surface as `Err`.

### `FLUSH`-based `clear()`

Rejected per Decision 9. On a shared deployment, `FLUSHDB` is a cross-tenant
data-loss incident.

## Context

### Problem

ADR-0023 and ADR-0056 declared Redis backends future work. Deployments that
share cache and idempotent state across restarts or nodes need them. The Redis
component already owns topology resolution, Sentinel failover, and a multiplexed
executor. A second, independent Redis client inside the service crate would
duplicate that machinery and diverge on failover behavior.

### Forces

- **Reuse with a narrow seam.** The service needs connection acquisition and
  topology resolution, not the component's command dispatch. Widening three
  items is the smallest additive surface that serves both.
- **Hexagonal boundary.** Dependency direction stays
  `camel-api <- camel-redis <- camel-redis-repo <- camel-config`. `camel-core`
  imports nothing new.
- **Fail-closed culture.** Topology selection, charset guards, and cluster
  rejection all validate at startup (ADR-0033).
- **Shared deployments.** Redis instances are commonly shared. `clear()` and
  `invalidate_prefix` must never escape the repository namespace.
- **Unknown-outcome honesty.** Contract C1 governs every repository backend. A
  lost response to `SET NX` must surface as `Err`, and transient failures must
  classify as `"io"`.

## Consequences

### Registration through `camel-config`

`[default.cache_repo] backend = "redis"` and
`[default.idempotent_repo] backend = "redis"` register the repositories at
context build time (`crates/camel-config/src/context_ext.rs`). The DSL steps
select repositories by name. No autowiring exists, matching ADR-0023 and
ADR-0056.

### Component surface grows by three items

`get_conn`, `refresh`, and `topology_from_config` become `pub` in
`camel-component-redis`. Existing consumers are unaffected. The widening is
recorded here so future component audits know the public surface is
intentional.

### Test seams live in the service crate

`FakeRepoExecutor` and `FakeStaticTopology`
(`crates/services/camel-redis-repo/src/executor.rs:164`,
`crates/services/camel-redis-repo/src/executor.rs:291`) drive both repositories
without a live Redis. Live coverage runs in
`crates/camel-test/tests/redis_repositories_test.rs` under the
`integration-tests` feature, per ADR-0054.

### Credentials stay redacted

Sentinel node credentials and connection URLs follow ADR-0051. Redaction
happens in the component's topology code. The service adds no new
credential-bearing `Debug` output.

## Load-bearing citations

| File:line | Element |
|---|---|
| `crates/services/camel-redis-repo/src/lib.rs:20-24` | crate exports: `RedisCacheRepository`, `RedisIdempotentRepository` |
| `crates/services/camel-redis-repo/src/executor.rs:46-51` | `trait RepoCommandExecutor` (`execute`, `refresh`) |
| `crates/services/camel-redis-repo/src/executor.rs:56` | `MultiplexedRepoExecutor` wrapping the component executor |
| `crates/services/camel-redis-repo/src/connection.rs` | one topology resolution per construction, cluster rejected first |
| `crates/services/camel-redis-repo/src/keyspace.rs:9` | `fn namespaced` builds `{prefix}:{repo}:{key}` |
| `crates/services/camel-redis-repo/src/keyspace.rs:17` | `fn validate_namespace_token` enforces `[A-Za-z0-9:_-]` |
| `crates/services/camel-redis-repo/src/cache_repo.rs:111-123` | single `SET` with `EXAT (expires_at + stale_retention)` |
| `crates/services/camel-redis-repo/src/idempotent_repo.rs` | `add` issues one non-retried `SET NX` |
| `crates/services/camel-redis-repo/src/error.rs:7` | `fn to_camel_error` maps Redis failures to `Io` |
| `crates/components/camel-redis/src/executor.rs:268` | `MultiplexedExecutor::get_conn` (widened to `pub`) |
| `crates/components/camel-redis/src/executor.rs:313` | `MultiplexedExecutor::refresh(&self)` reconnect primitive |
| `crates/camel-config/src/context_ext.rs:258-279` | redis cache and idempotent repository registration |
| `crates/camel-test/tests/redis_repositories_test.rs` | live integration suite |
