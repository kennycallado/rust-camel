## ADDED Requirements

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
