# Design: redis-sentinel-failover

## Approach

Introduce a **thin topology-resolution seam** that returns a `redis::Client`
instead of hardcoding `redis::Client::open(config.redis_url())` at the three
connect sites. The seam is the *only* place that knows how to find the current
master.

```text
pub enum ServerKind { Master /* MVP */, Replica /* excluded */ }

#[async_trait]
pub trait RedisTopology: Send + Sync {
    async fn resolve(&self, kind: ServerKind) -> Result<redis::Client, CamelError>;
}
```

- `StandaloneTopology { url }` → `redis::Client::open(url)` (today's behavior,
  bit-identical for `redis://` endpoints).
- `SentinelTopology { sentinel_client: Mutex<SentinelClient> }` → wraps
  redis-rs 1.5 `SentinelClient::build(nodes, master_name, node_conn_info,
  SentinelServerType::Master)`. `resolve()` calls `get_async_connection()`'s
  client-resolution path again — it does **not** cache a resolved master URL
  across reconnects (caching would reintroduce the bug).

The topology is constructed once per `RedisEndpointConfig` (or per global
`RedisConfig`) and held alongside the connection cache. Every connect/reconnect
site calls `topology.resolve(Master)` first.

**Two orthogonal seams, both injectable.** Deterministic failover testing (the
user's explicit "testear y cubrir" ask) needs to fake **two** things: *where to
connect* (the topology) and *what the command does once connected* (execution).
A single topology seam is insufficient — `FakeTopology` returns real
`redis::Client` values that a unit test cannot drive. Therefore:

- `RedisTopology` — the **resolution** seam (`resolve(Master) -> Client`),
  faked by `FakeTopology` (records resolve calls, simulates A→B election).
- `RedisCommandExecutor` — the **execution** seam (already exists in
  `executor.rs`, currently implemented only by `FakeExecutor`). The producer is
  **refactored to route production command execution through it** via a new real
  impl `MultiplexedExecutor` (wraps the cached `MultiplexedConnection` +
  `dispatch_command`). The `execute_with_retry` signature is **unchanged** —
  topology re-resolution is reached **indirectly**: `execute_with_retry` already
  calls `executor.reconnect()` before each retry, and `MultiplexedExecutor::
  reconnect()` clears the connection cache and calls
  `self.topology.resolve(Master)` to rebuild it. This keeps `execute_with_retry`
  generic (so the existing `FakeExecutor` unit tests stay intact) while the real
  executor consults the topology on every reconnect. This unifies the dead
  abstraction with production instead of deleting it — the executor is no longer
  fake-only. For layering hygiene, `dispatch_command` (the redis-command match)
  moves OUT of `producer.rs` and INTO `executor.rs` (or a `commands` module) so
  the executor owns the full execute path and there is no executor↔producer
  upward coupling.

A unit test composes two facts deterministically: (a) `execute_with_retry` +
`FakeExecutor` proves reconnect-before-retry (assert `reconnect_count`); (b)
`MultiplexedExecutor::reconnect` + `FakeTopology` proves re-resolution (assert
`resolve_call_count`). The two compose correctly in production; no single
combined assertion is needed and none is possible with the pure `FakeExecutor`.

The **producer** has these two seams. The **consumers and health check** have a
*third* seam category — I/O seams (`QueueIo`, `PubSubIo`, `HealthProbe`) that
abstract "turn a resolved `redis::Client` into a working connection and operate
on it" so the consumer failover loops and the health PING are testable WITHOUT a
broker (a `FakeQueueIo`/`FakePubSubIo`/`FakeHealthProbe` drives connect-failure,
stream-end, and probe outcomes deterministically). So three seam kinds total:
`RedisTopology` (resolution), `RedisCommandExecutor` (producer execution), and
the consumer/health I/O traits (loop testability). The trait returns a plain
`redis::Client` and wraps no driver command types, respecting the
dependency-boundary note in `camel-redis/CONTEXT.md` (ADR-0020 does not govern
redis; a thin adapter is acceptable, a heavy one is not). Precedent:
`CacheRepository` port (ADR-0056).

## Affected crates

- **camel-component-redis**: new `topology.rs` module (`RedisTopology`,
  `StandaloneTopology`, `SentinelTopology`, `ServerKind`, `FakeTopology` for
  tests). `config.rs` delegates sentinel concerns to a new sibling
  `sentinel_config.rs` module (`SentinelConfig`, `TopologyKind`, `redis-sentinel:`
  / `rediss-sentinel:` URI parsing, `validate_topology`, feature-disabled
  fail-closed) so the already-large `config.rs` does not deepen. `executor.rs`
  gains a real `MultiplexedExecutor` impl (owns `dispatch_command`, moved here
  from `producer.rs`) and its `reconnect()` re-resolves via the topology.
  `producer.rs`, `consumer.rs`, `health.rs` route connection establishment
  through the topology seam. `Cargo.toml` enables `redis = { features =
  ["tokio-comp", "aio", "sentinel"] }` behind the `sentinel` feature flag
  (already a placeholder in the `add-mcp-component` worktree).
- **camel-config** (if it owns the `[components.redis]` TOML schema): add the
  `[components.redis.sentinel]` sub-block.

## Architecture boundaries

This change is entirely **within the Components layer** — it adds no Runtime,
DSL, or control-plane surface. It respects the supervision contract
(ADR-0007): the consumer does **not** self-heal or call
`force_unhealthy_for_route`. Failover recovery is **bounded transport
reconnect** — re-resolve then reconnect, capped by `NetworkRetryPolicy`
(ADR-0013); on exhaustion the path returns `Err`, Runtime pins health via
`FailRoute` (ADR-0004), and Route supervision restarts. This keeps the
"consumer returns Err → supervision is authoritative" invariant intact.

ADRs referenced: 0004 (atomic swap), 0007 (supervised consumer failure), 0012
(log levels), 0013 (NetworkRetryPolicy boundaries), 0032 (trust boundary —
master name comes from operator config, never exchange data), 0033 (safe
defaults / fail-closed validation), 0046 (Apache Camel is inspiration, not
conformance — we improve on the standalone-config trap), 0054 (#[ignore] policy
for the Docker suite), 0056 (port precedent).

## Phases

### Phase 1: Topology seam + config + producer integration

- **Goal:** introduce `RedisTopology`, `StandaloneTopology`, `SentinelTopology`
  (with separate sentinel-vs-node credentials), `SentinelConfig` + `TopologyKind`
  in a new `sentinel_config.rs`, `redis-sentinel:` URI parsing, mutual-exclusion
  validation, and the feature-disabled fail-closed reject; move
  `dispatch_command` into `executor.rs`; add the real `MultiplexedExecutor` impl
  whose `reconnect()` re-resolves via the topology; wire the producer onto the
  executor seam.
- **Dependencies:** redis-rs `sentinel` feature; ADR-0013, ADR-0033.
- **Externally-visible types/interfaces:** `RedisTopology` trait,
  `StandaloneTopology`, `SentinelTopology`, `SentinelConfig`, `ServerKind`,
  new URI schemes.
- **Deliverable:** producer reconnects to the elected master; standalone paths
  unchanged.
- **Exit-criteria:** `producer_reconnects_to_new_master_after_failover`,
  `producer_does_not_replay_non_idempotent_command`,
  `standalone_endpoint_unchanged`, and config-validation tests pass.

### Phase 2: Consumer paths — queue + pubsub resubscription replay

- **Goal:** route both consumer paths through the seam; on failover re-resolve
  then reconnect; PubSub replays all SUBSCRIBE/PSUBSCRIBE before resuming.
- **Dependencies:** Phase 1 seam.
- **Externally-visible types/interfaces:** none new (behavioral).
- **Deliverable:** both consumers survive a master switch bounded by policy.
- **Exit-criteria:** `queue_reresolves_master_after_connection_loss`,
  `pubsub_resubscribes_after_master_switch`,
  `consumer_returns_error_when_failover_budget_exhausted` pass.

### Phase 3: Health, logs, docs

- **Goal:** sentinel-aware health check; redacted logs (ADR-0051); CONTEXT.md +
  docs/src/components/redis.md + example.
- **Dependencies:** Phase 1–2.
- **Exit-criteria:** `health_checks_current_sentinel_master`; docs updated;
  lint-context-citations / lint-log-levels green.

### Phase 4: Sentinel failover integration suite (camel-test + testcontainers)

- **Goal:** self-provisioned integration test in `camel-test/tests/` behind
  `--features integration-tests`: provisions Redis + Sentinel containers via
  testcontainers, triggers `SENTINEL FAILOVER`, asserts producer/queue/PubSub
  recovery within a bounded deadline. No `#[ignore]` — runs in CI's
  `full-tests-linux` job per ADR-0054 (the camel-test + testcontainers pattern,
  already used for the existing `redis_test.rs`).
- **Exit-criteria:** suite runs green in CI; the `lint-ignore` gate stays clean
  (no `requires live` annotations).

## Alternatives considered

- **`SentinelClient` directly at each connect site** — rejected: untestable
  without Docker; reintroduces duplication; no seam for fake failover.
- **Hand-rolled `SENTINEL get-master-addr-by-name` resolver** — rejected: loses
  redis-rs role validation + upstream fixes; the trait can still call through
  `SentinelClient` internally.
- **Proactive sentinel polling** (background task) — deferred: adds tasks,
  sentinel load, and split-brain surface without removing the disconnect window.
  Reactive rediscovery (re-resolve on failure) is the MVP.
- **Inner unbounded reconnect loop in the consumer** — rejected: violates
  ADR-0007 (consumer must not self-heal; supervision stays authoritative).
- **Single topology seam that returns a real `redis::Client`** — rejected: a
  unit test cannot drive a real `redis::Client`, so failover could not be tested
  deterministically without a second (execution) seam. Two orthogonal injectable
  seams (`RedisTopology` + `RedisCommandExecutor`) are required.
