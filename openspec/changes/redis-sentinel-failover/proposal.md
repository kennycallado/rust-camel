# Proposal: redis-sentinel-failover

## Why

When a Redis Sentinel-managed master dies, a topology-unaware client keeps
reconnecting to the **dead master** and never discovers the elected replacement.
The maintainer hit this exact bug in production Apache Camel (camel-redis wired
with a standalone `RedisStandaloneConfiguration`). rust-camel reproduces it
**today**: all three connection paths in `camel-component-redis` open
`redis::Client::open(fixed_url)` with no master rediscovery.

- **Producer** (`producer.rs`): on a transient error it clears the cached
  connection and reconnects to the same URL — the dead master — until
  `NetworkRetryPolicy` exhausts, then errors the exchange.
- **Consumer — PubSub** (`consumer.rs`): stream end → task returns `Err` → Route
  supervision restarts (ADR-0007) → the new consumer reopens the same dead URL.
  Manifestation: an infinite route-restart loop against the dead master.
- **Consumer — Queue/BLPOP**: retries the command on the same dead connection,
  terminates after `max_attempts`, supervision restarts, same dead URL.

The framework must detect Sentinel failover and reconnect to the elected master
**autonomously, and this behavior must be tested and covered by the framework**
(deterministic unit tests; a Docker-gated integration suite). bd: `rc-upda`.

## What Changes

**In scope:**

- A thin **`RedisTopology`** boundary in `camel-component-redis`:
  `resolve(ServerKind) -> Client`. Two implementations: `StandaloneTopology`
  (fixed URL, current behavior) and `SentinelTopology` (wraps redis-rs 1.5
  `SentinelClient`, re-resolves the master on every reconnect). No driver-type
  wrapping — the trait returns a `redis::Client`. Precedent: the `CacheRepository`
  port (ADR-0056) and the thin-port pattern.
- New URI schemes **`redis-sentinel:`** and **`rediss-sentinel:`**, plus a
  structured `[components.redis.sentinel]` config block (`nodes`,
  `master_name`, separate sentinel credentials). Sentinel and Cluster topologies
  are mutually exclusive.
- Sentinel-aware reconnect in all three paths: re-resolve **before** each
  reconnect (not retry against the dead master), bounded by `NetworkRetryPolicy`
  (ADR-0013). On policy exhaustion the path returns `Err` so Route supervision
  (ADR-0007) stays authoritative — this is bounded transport reconnect, **not**
  consumer self-supervision.
- PubSub resubscription replay: after a failover-induced reconnect, replay all
  `SUBSCRIBE`/`PSUBSCRIBE` before resuming reads.
- Sentinel-aware `RedisHealthCheck`: resolves the **current** master via the
  topology, PINGs it (not the stale configured node).
- Deterministic failover unit tests via injectable `RedisTopology` +
  `RedisCommandExecutor` seams. Sentinel integration suite self-provisioned via
  testcontainers in `camel-test/` (runs in CI's `full-tests-linux` job behind
  `--features integration-tests`, per ADR-0054 — no `#[ignore]`).

**Explicitly excluded:** Cluster mode rediscovery (REDIS-012, separate change),
replica reads, proactive sentinel polling, message replay/dedup guarantees
(PubSub is best-effort — loss and duplicates possible; documented).

## Acceptance criteria

- `redis-sentinel://` endpoints reconnect to the elected master after a failover
  within a bounded retry budget, proven by a fake-topology unit test per path.
- `redis://` standalone endpoints behave bit-identically to today (no regression).
- PubSub consumer replays subscriptions after a master switch.
- On failover-budget exhaustion the consumer returns `Err` (supervision fires);
  it never silently masks a fatal failure.
- `RedisHealthCheck` reports the current master, not the configured node.
- Sentinel and Cluster configs are rejected together at startup (ADR-0033).

## Risk budget

- **Low:** enabling redis-rs `sentinel` feature (adds only `log`+`rand` deps).
- **Medium:** the failover window itself (best-effort PubSub delivery — loss and
  duplicates possible; split-brain during
  Sentinel election handled by requiring role validation before use).
- **Out of bounds:** strong delivery guarantees; Cluster mode; replica routing.
