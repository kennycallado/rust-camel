## ADDED Requirements

### Requirement: repository service crate compiles sentinel unconditionally and selects it by explicit config

The system SHALL provide Redis-backed repository implementations (repository service crate
`camel-redis-repo`) in which sentinel support is enabled unconditionally: the crate's
manifest enables both the `camel-component-redis/sentinel` crate feature (the controlling
branch for the reused `topology_from_config` code) and the `redis/sentinel` driver
feature, so no fail-closed "sentinel feature disabled" rejection exists on the repository
path. Cargo feature unification consequence: any application graph that links
`camel-redis-repo` compiles the component WITH its sentinel support. The repository
service crate SHALL select sentinel topology only through explicit configuration
fields (`sentinel_nodes` plus `master_name`) and SHALL NOT probe endpoints at runtime to
infer topology and SHALL NOT require a `redis-sentinel://` URI scheme. The crate SHALL
reuse the component's public connection seams: `RedisTopology`,
`topology_from_config` (widened from `pub(crate)` to `pub`),
`MultiplexedExecutor`, `MultiplexedExecutor::get_conn` (widened from
`pub(crate)` to `pub`) as the `&self` connection-acquisition primitive, and a
new `pub async fn MultiplexedExecutor::refresh(&self)` reconnect primitive
(an `&self` analogue of the existing `&mut self` `reconnect`). The crate
SHALL issue repository-level commands (`SCAN`, `UNLINK`, `SET NX`,
`SET … EXAT`, `EXISTS`, `GET`, `DEL`) as `redis::Cmd` values through its
internal `RepoCommandExecutor` seam; the production executor pipes each
command over the `MultiplexedConnection` returned by `get_conn`, and the
crate SHALL NOT use the
component's `RedisCommand` dispatch (that path is unchanged and does not
model these commands). This requirement does not modify the component's own `sentinel`
feature gate: `camel-redis` keeps `sentinel` default-off and keeps its fail-closed
rejection of `redis-sentinel://` URIs and `[components.redis.sentinel]` blocks when the
feature is disabled, for builds that do not link `camel-redis-repo`. The asymmetry is
intentional: the component gates a compile-time capability on an opt-in feature; the
service crate compiles the capability always and gates the behavior on explicit config.
The repository service crate SHALL resolve the sentinel master once at construction
(eager connection, fail fast on an unreachable topology; the component offloads the
blocking sentinel resolve internally, topology.rs:270-275) and SHALL re-resolve only
after a connection error; it SHALL NOT resolve per repository operation.

#### Scenario: sentinel selected by config fields, no URI scheme

- **GIVEN** repository configuration with `sentinel_nodes = ["s-a:26379", "s-b:26379"]`
  and `master_name = "orders"`, against a live sentinel topology
- **WHEN** the repository connection is constructed
- **THEN** the connection targets the master resolved through the sentinels

#### Scenario: master resolved once, not per operation

- **GIVEN** a repository over a sentinel topology and `FakeStaticTopology` (the
  service-crate fake implementing the component's `RedisTopology` trait) counting
  `resolve()` calls
- **WHEN** two `get` operations are awaited on a healthy connection
- **THEN** `resolve()` was called exactly once

#### Scenario: command response timeout maps to transient Io

- **GIVEN** an executor against a peer that accepts commands but never replies
- **WHEN** a command is executed
- **THEN** the call fails within the response timeout with `Err(CamelError::Io)` that
  classifies transient, and the next call triggers re-resolve

#### Scenario: component feature gate unchanged for graphs without the service crate

- **GIVEN** the `camel-redis` component built without its `sentinel` feature, in an
  application that does not link `camel-redis-repo`
- **WHEN** a `redis-sentinel://` component endpoint is configured
- **THEN** startup rejects the endpoint with the existing fail-closed error

#### Scenario: cluster topology rejected for repositories

- **GIVEN** repository configuration that requests a cluster topology
- **WHEN** the configuration is validated
- **THEN** validation returns an error stating cluster mode is unsupported for
  repository backends

### Requirement: Redis repository live coverage runs in camel-test, never ignored

The system SHALL cover the Redis repository backends with a non-ignored integration
suite in `crates/camel-test/tests/redis_repositories_test.rs` under the existing
`integration-tests` feature, using testcontainers to self-provision Redis and a Sentinel
topology (the `redis_sentinel_test.rs` pattern, per ADR-0054 which forbids
external-service `#[ignore]` tests). The suite SHALL exercise cache and idempotent
trait behavior against real Redis (round-trips, prefix-scoped `clear`, registration via
`CamelConfig`) and the sentinel path (construction through sentinels). The
`.github/workflows/ci.yml` workflow SHALL run the suite alongside the existing
`redis_test` target. `camel-redis-repo` SHALL be an optional dependency of `camel-test`
under `integration-tests` (leaf direction only); the service crate SHALL NOT depend on
`camel-test`.

#### Scenario: integration suite runs in CI without ignore markers

- **GIVEN** the `redis_repositories_test` suite in `camel-test` with the
  `integration-tests` feature enabled
- **WHEN** `cargo test -p camel-test --features integration-tests --test
  redis_repositories_test` runs in CI
- **THEN** the suite provisions its containers, exercises cache and idempotent
  behavior, and contains no `#[ignore]` attributes
