# redis-failover Specification

## Purpose
TBD - created by archiving change redis-sentinel-failover. Update Purpose after archive.
## Requirements
### Requirement: RedisTopology resolution seam

The system SHALL provide an async, object-safe `RedisTopology` trait in
`camel-component-redis` whose `resolve(kind: ServerKind)` method returns a fresh
`redis::Client` for the requested server kind. The trait SHALL wrap **no**
redis driver command types — it returns only a `redis::Client`. The system SHALL
provide two production implementations: `StandaloneTopology` (returns
`redis::Client::open(fixed_url)` — bit-identical to current behavior for
`redis://` endpoints) and `SentinelTopology` (wraps redis-rs `SentinelClient`
built with `SentinelServerType::Master` and re-resolves the master on **every**
`resolve()` call — it SHALL NOT cache a resolved master URL across reconnects).
A `FakeTopology` test double SHALL be constructible with a programmable address
sequence so failover is deterministic without Docker. Deterministic failover
unit tests SHALL inject BOTH a `FakeTopology` (to assert re-resolution and
target master A then B) and a `FakeExecutor` (to simulate command outcomes)
— a topology seam alone returns real `redis::Client` values a unit test cannot
drive.

#### Scenario: standalone resolve returns a fixed client

- **GIVEN** a `StandaloneTopology` built from `redis://127.0.0.1:6379`
- **WHEN** `resolve(Master)` is called twice
- **THEN** both calls return `Ok(Client)` targeting the same address

#### Scenario: sentinel resolve re-resolves the master on every call

- **GIVEN** a `SentinelTopology` whose sentinel cluster has elected master A,
  then a failover promotes master B
- **WHEN** `resolve(Master)` is called before and after the failover
- **THEN** the first call returns a `Client` for A and the second returns a
  `Client` for B (no cached master URL)

#### Scenario: fake topology simulates deterministic failover

- **GIVEN** a `FakeTopology` programmed to return address A for calls 1..N and
  address B thereafter
- **WHEN** `resolve(Master)` is called N+1 times
- **THEN** calls 1..N resolve to A and call N+1 resolves to B, with no network

### Requirement: redis-sentinel URI scheme and SentinelConfig

The system SHALL accept `redis-sentinel://` and `rediss-sentinel://` endpoint
URIs of the form
`redis-sentinel://sentinel-a:26379,sentinel-b:26379/<master-name>/<db>?command=...`
and a structured `[components.redis.sentinel]` config block with `nodes`
(`Vec<String>`), `master_name` (`String`), and optional `username`/`password`
sentinel credentials. Redis-node credentials (the top-level
`[components.redis]` username/password) SHALL be kept separate from sentinel
credentials. When the crate's `sentinel` feature is disabled, the system SHALL
reject `redis-sentinel:` / `rediss-sentinel:` URIs and any non-empty
`[components.redis.sentinel]` block at startup with a clear error (fail-closed,
ADR-0033) rather than silently falling back to standalone. The system SHALL
reject, at startup, any configuration that sets both `sentinel` and `cluster`
nodes (mutual exclusion per ADR-0033). The `master_name` SHALL come only from
operator-owned config or the URI path — never from exchange data (ADR-0032).

#### Scenario: redis-sentinel URI parses to sentinel topology

- **GIVEN** the URI `redis-sentinel://s-a:26379,s-b:26379/orders/0?command=GET`
- **WHEN** the endpoint is created
- **THEN** the topology is `SentinelTopology` with nodes `[s-a:26379, s-b:26379]`,
  master_name `orders`, db `0`, and command `GET`

#### Scenario: rediss-sentinel enables TLS

- **GIVEN** the URI `rediss-sentinel://s-a:26379/orders/0`
- **WHEN** the endpoint is created
- **THEN** both sentinel and node connections use TLS

#### Scenario: sentinel + cluster configured together is rejected at startup

- **GIVEN** a config with non-empty `sentinel.nodes` and non-empty `cluster_nodes`
- **WHEN** the component starts
- **THEN** startup fails fast with a `Config` error (ADR-0033)

#### Scenario: sentinel scheme is rejected when the feature is disabled

- **GIVEN** the crate compiled WITHOUT the `sentinel` feature
- **WHEN** a `redis-sentinel://` URI or a non-empty `[components.redis.sentinel]`
  block is supplied
- **THEN** startup fails fast with a clear error (fail-closed, ADR-0033); it does
  not silently fall back to standalone

#### Scenario: sentinel and node credentials are applied without crossover

- **GIVEN** a config with sentinel credentials `(sentinel-user, sentinel-pass)`
  and node credentials `(app-user, app-pass)`
- **WHEN** the topology connects to a sentinel and then to the resolved master
- **THEN** the sentinel connection authenticates with `(sentinel-user,
  sentinel-pass)` and the master connection authenticates with `(app-user,
  app-pass)` — the two credential sets are never swapped or merged

#### Scenario: standalone redis endpoint is unchanged

- **GIVEN** a `redis://127.0.0.1:6379?command=GET` endpoint
- **WHEN** the endpoint is created and used
- **THEN** behavior is bit-identical to before this change (StandaloneTopology)

### Requirement: producer reconnects to the elected master after failover

The producer SHALL route connection establishment through the topology seam.
On a qualifying transport failure (connection refused / reset / timeout / EOF /
read-only-role error) of an idempotent command while a `SentinelTopology` is
configured, the producer SHALL invalidate the cached connection, call
`topology.resolve(Master)` to obtain the current master, and reconnect — it
SHALL NOT retry the command against the previous (potentially dead) master. The
bounded reconnect loop SHALL honor `NetworkRetryPolicy` (ADR-0013): after the
budget is exhausted the producer SHALL return `Err`. A non-idempotent command
that fails with an ambiguous transport error SHALL NOT be retried (the producer
returns `Err` immediately).

#### Scenario: idempotent command succeeds on the new master after failover

- **GIVEN** a producer with a `FakeTopology` returning master A then master B,
  and a fake executor whose first `GET` fails transiently against A
- **WHEN** the producer processes the `GET`
- **THEN** it invalidates the A connection, resolves B, reconnects, retries the
  `GET`, and returns `Ok`

#### Scenario: non-idempotent command is not retried on ambiguous failure

- **GIVEN** a producer and an `INCR` command whose first attempt fails with a
  transient transport error
- **WHEN** the producer processes the `INCR`
- **THEN** it returns `Err` without re-resolving or re-executing

#### Scenario: producer returns Err after failover budget exhaustion

- **GIVEN** a producer whose topology always fails to resolve a usable master
- **WHEN** the producer retries an idempotent command up to `max_attempts`
- **THEN** it returns `Err` (does not loop forever)

### Requirement: consumer re-resolves the master on connection loss

The PubSub and Queue (BLPOP/BRPOP) consumers SHALL route connection
establishment through the topology seam. On a qualifying connection loss while a
`SentinelTopology` is configured, the consumer SHALL re-resolve the master via
`topology.resolve(Master)`, reconnect, and resume — bounded by
`NetworkRetryPolicy`. After the budget is exhausted the consumer task SHALL
return `Err` so Route supervision (ADR-0007) restarts it; the consumer SHALL
NOT hide an unbounded inner reconnect loop. This reconnect is bounded transport
recovery, not consumer self-supervision.

#### Scenario: queue consumer recovers after master switch

- **GIVEN** a `BLPOP` consumer with a `FakeTopology` returning A then B
- **WHEN** the connection to A is lost
- **THEN** the consumer resolves B, reconnects, and continues popping; one
  Exchange is emitted per item, bounded by `max_attempts`

#### Scenario: consumer returns Err when failover budget is exhausted

- **GIVEN** a consumer whose topology cannot resolve any usable master within
  the budget
- **WHEN** the reconnect loop exhausts `max_attempts`
- **THEN** the consumer task returns `Err`, enabling Route supervision to fire

#### Scenario: PubSub consumer returns Err on budget exhaustion after stream end

- **GIVEN** a PubSub consumer subscribed to channel `events`, whose topology
  cannot resolve a usable master after the previous connection's stream ended
- **WHEN** the reconnect loop exhausts `max_attempts`
- **THEN** the PubSub consumer task returns `Err` (it does not silently spin on
  the dead connection), enabling Route supervision

#### Scenario: Queue consumer returns Err on budget exhaustion

- **GIVEN** a `BLPOP` consumer whose topology cannot resolve a usable master
- **WHEN** the reconnect loop exhausts `max_attempts`
- **THEN** the Queue consumer task returns `Err`, enabling Route supervision
  (distinct from the PubSub path — both fail independently)

### Requirement: PubSub subscription replay after failover reconnect

After a failover-induced reconnect, the PubSub consumer SHALL replay every
configured `SUBSCRIBE` (channel) and `PSUBSCRIBE` (pattern) on the new
connection before resuming message reads, because subscriptions are per-
connection state. The system documents PubSub transport as **best-effort
delivery**: messages published during the disconnect window MAY be lost (Redis
Pub/Sub is not persistent), and duplicate delivery MAY occur around the failover
reconnection. Routes that require stronger guarantees SHALL deduplicate
downstream (e.g. via message IDs). The system SHALL NOT claim at-most-once
semantics (at-most-once forbids duplicates, which are possible here).

#### Scenario: subscriptions are replayed on the new master

- **GIVEN** a PubSub consumer subscribed to channels `[a, b]` and pattern `ev*`,
  with a `FakeTopology` returning A then B
- **WHEN** the A connection's stream ends
- **THEN** the consumer resolves B, reconnects, re-issues `SUBSCRIBE a`,
  `SUBSCRIBE b`, and `PSUBSCRIBE ev*`, then resumes delivering messages

### Requirement: sentinel-aware health check

`RedisHealthCheck` SHALL resolve the **current** master through the configured
topology and PING it, rather than probing a stale configured address. If the
topology cannot resolve a master (sentinels unreachable, no validated master),
the health check SHALL report `Unhealthy`; it SHALL NOT report `Healthy` by
probing the stale configured node.

#### Scenario: health check pings the current master, not the configured node

- **GIVEN** a `RedisHealthCheck` with a `FakeTopology` that switched from A to B
- **WHEN** `check()` runs
- **THEN** it PINGs B and reports `Healthy` (it does not probe A)

#### Scenario: health check is unhealthy when no master is resolvable

- **GIVEN** a `RedisHealthCheck` whose topology resolves no usable master
- **WHEN** `check()` runs
- **THEN** it reports `Unhealthy`

### Requirement: sentinel failover integration suite via camel-test + testcontainers

The system SHALL provide a self-provisioned sentinel-failover integration test
under `camel-test/tests/` (behind the crate's `integration-tests` feature), that
provisions Redis and Sentinel containers via testcontainers, triggers a
`SENTINEL FAILOVER`, and asserts that the producer, queue consumer, and PubSub
consumer reconnect to the elected master within a bounded deadline. The suite
SHALL run in CI's `full-tests-linux` job. The suite SHALL NOT use `#[ignore]`
(ADR-0054: external-service tests must follow the camel-test + testcontainers
pattern; `requires live` is a migration error rejected by `lint-ignore`).

#### Scenario: producer recovers after a real sentinel failover

- **GIVEN** a `camel-test` integration test with a Redis master, a replica, and a
  sentinel provisioned via testcontainers, and a producer writing to the master
- **WHEN** `SENTINEL FAILOVER` is issued and the deadline elapses
- **THEN** subsequent producer writes succeed against the newly elected master

#### Scenario: lint-ignore gate stays clean

- **GIVEN** the new integration suite
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** it passes (no `requires live` or bare `#[ignore]` annotations were
  introduced)

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

### Requirement: standalone redis database selection is honored by the driver

The redis component's standalone topology SHALL resolve a `redis::Client`
whose `ConnectionInfo` carries the configured database number (from the
`?db=N` URI parameter), instead of deriving the driver connection from a URL
string that drops it. Sentinel resolution already sets db explicitly and is
unchanged.

#### Scenario: standalone URI with db N resolves client with db N

- **GIVEN** a standalone `RedisEndpointConfig` parsed from
  `redis://localhost:6379?command=GET&db=2`
- **WHEN** the topology resolves a client for `ServerKind::Master`
- **THEN** `redis_settings().db()` equals 2

#### Scenario: standalone URI without db resolves db 0 (unchanged default)

- **GIVEN** a standalone `RedisEndpointConfig` parsed from
  `redis://localhost:6379?command=GET` (no `db` parameter)
- **WHEN** the topology resolves a client
- **THEN** `redis_settings().db()` equals 0

#### Scenario: TLS standalone URI keeps TcpTls addr and db

- **GIVEN** a standalone `RedisEndpointConfig` parsed from
  `rediss://localhost:6380?command=GET&db=3` (ssl enabled)
- **WHEN** the topology resolves a client
- **THEN** the connection address is `TcpTls` (insecure false,
  tls_params none) AND `redis_settings().db()` equals 3

#### Scenario: credentials ride ConnectionInfo without re-encoding

- **GIVEN** a standalone config with a password containing URI-reserved
  characters (e.g. `p@ss:word`)
- **WHEN** the topology resolves a client
- **THEN** `redis_settings().password()` equals the raw configured password
  (no percent-decode/encode on the driver path); username propagation is
  out of scope and unchanged

#### Scenario: repository service standalone backends inherit db selection

- **GIVEN** a camel-redis-repo cache or idempotent backend configured for a
  standalone endpoint with `?db=2`
- **WHEN** `connect_executor` establishes the connection
- **THEN** the connection issues `SELECT 2` (the configured db takes effect
  on the repo driver path)

#### Scenario: display strings unchanged

- **GIVEN** a standalone config with `db=2`
- **WHEN** `redis_url()`, `redis_url_safe()`, and `safe_endpoint()` render
- **THEN** the rendered strings are byte-identical to before this change
  (`?db=N` query form), and `from_uri(redis_url())` still round-trips

### Requirement: Multiplexed connection build accepts a driver response timeout

The system SHALL let `MultiplexedExecutor` (component `camel-redis`) carry an
optional driver-level response timeout, set through a builder-style
`with_response_timeout(Duration)` that does not alter the `new(...)`
signature. When no value is set, the connection build SHALL remain the
config-less `get_multiplexed_async_connection()` call, preserving the redis
driver's default per-command deadline (500 ms in redis 1.6.0) and all
existing behavior. When a value is set, every connection built through
`get_conn` — the initial connect and every rebuild performed by `refresh()`
and `reconnect` — SHALL be constructed through
`get_multiplexed_async_connection_with_config` with
`AsyncConnectionConfig::set_response_timeout` set to that value, so the
configured deadline governs each command pipelined over the connection
instead of the driver default. The setting SHALL NOT alter the connect
timeout: `connection_timeout_secs` continues to bound only the TCP connect
phase.

#### Scenario: unset response timeout keeps the driver default path

- **GIVEN** a `MultiplexedExecutor` built with `new(...)` and no
  `with_response_timeout` call
- **WHEN** `get_conn` builds the connection
- **THEN** the build uses the config-less call and the driver's 500 ms
  default response deadline continues to govern commands (behavior
  identical to before this change; existing executor tests pass unchanged)

#### Scenario: configured large timeout outlives the driver default

- **GIVEN** a `MultiplexedExecutor` with `with_response_timeout(10 s)`
  against a silent peer whose TCP connect succeeds but never replies, under
  tokio's paused clock
- **WHEN** the command future is polled and virtual time advances past the
  500 ms driver default (e.g. to 1 s virtual)
- **THEN** the command is still pending at that virtual-time boundary — the
  configured value, not the driver default, sets the deadline

#### Scenario: configured small timeout fires before the driver default

- **GIVEN** a `MultiplexedExecutor` with `with_response_timeout(100 ms)`
  against a silent peer, under tokio's paused clock
- **WHEN** the command future is polled and virtual time advances to the
  100 ms boundary
- **THEN** the command has failed by the configured deadline — before the
  500 ms driver default fires

#### Scenario: refresh rebuild carries the configured deadline

- **GIVEN** a `MultiplexedExecutor` with a configured response timeout
  holding a connection to a silent peer, under tokio's paused clock
- **WHEN** `refresh()` drops the cached connection and rebuilds, and a
  command probe is polled with virtual time advanced to the configured
  boundary
- **THEN** the probe fails by the configured deadline, not the 500 ms
  default — the rebuilt connection carries the same configured response
  timeout

#### Scenario: configured response timeout does not alter the connect timeout

- **GIVEN** a `MultiplexedExecutor` with `with_response_timeout(100 ms)`
  and `connection_timeout_secs = 1` against a peer that accepts the TCP
  connection but never completes the RESP handshake, under tokio's paused
  clock
- **WHEN** `get_conn` attempts to build the connection and virtual time is
  advanced (polling the future before each advance) to the 1 s connect
  boundary
- **THEN** the failure is the connect timeout ("Redis connection … timed
  out after 1s") — the response-timeout setting bounds only command
  round-trips after the connection is established

### Requirement: Repository service sets a driver response timeout above its local backstop

The repository service crate (`camel-redis-repo`) SHALL construct its
`MultiplexedExecutor` with a driver response timeout strictly greater than
its crate-local per-command backstop (`DEFAULT_RESPONSE_TIMEOUT` = 30 s,
ADR-0063 Decision 13) — the implementation uses a fixed 5 s margin (35 s);
the binding
contract is the strict ordering, observable through behavior, not the exact
figure. The local backstop SHALL therefore always fire first: the error
message and transient-Io classification asserted by the existing backstop
tests SHALL remain exactly the tested ones, and the driver deadline SHALL
act only as defense-in-depth for any path that bypasses the local backstop.
The repository service crate SHALL NOT disable the driver deadline (`None`)
on its connections.

#### Scenario: local backstop governs classification over the driver deadline

- **GIVEN** a repository executor built through the production
  `connect_executor` path against a silent peer, with a short injectable
  local backstop, under tokio's paused clock where both deadlines are
  deterministic
- **WHEN** a command round-trip exceeds the local backstop
- **THEN** the failure is the local backstop's error ("redis command
  response timed out after …") classifying as transient Io — the driver's
  deadline sits above the backstop and does not fire first

#### Scenario: driver deadline sits strictly above the backstop

- **GIVEN** `connect_executor_with_topology` building the executor with the
  driver response timeout configured from the fixed margin
- **WHEN** a command runs against a silent peer with the local backstop
  injected below the driver deadline, under tokio's paused clock
- **THEN** the local backstop error wins — proving the driver deadline on
  the constructed connection is strictly greater than the backstop (the
  exact margin value is an implementation constant, not a contract)

