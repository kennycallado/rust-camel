# Proposal: redis-single-flight

## Why

`MultiplexedExecutor::get_conn` resolves the topology and builds the
connection with no lock held across that span; `refresh()` merely drops the
cached connection and calls `get_conn`. At sentinel failover, every
concurrent task whose command fails calls `refresh` — each independently
re-resolves the sentinel topology and opens its own connection, and the
last writer wins the cache slot. Correctness holds, but the cost is an
N-x sentinel query and connection storm exactly when the cluster is most
stressed (e_opus review P1, bd rc-wmwu). The repository service crate's
execute-retry-safe path makes this the default behavior for every repo
command that hits a transient failure at failover.

## What Changes

- `camel-redis` (component): `MultiplexedExecutor` gains a single-flight
  gate around the cache-miss path of `get_conn`. A dedicated
  `tokio::sync::Mutex<()>` gate plus a double-check of the cache after the
  gate: the first cache-miss caller becomes the leader (resolve + connect,
  gate held), every other concurrent caller parks on the gate, then takes
  the freshly stored connection from the cache. Concurrent cache-misses
  collapse from N resolves to exactly 1. On leader failure nothing is
  cached and each waiter re-checks and runs its own attempt sequentially —
  bounded concurrency (never a parallel storm), with later attempts seeing
  newer cluster state. Lock order is fixed (gate before conn); no public
  signature changes.

Excluded: no change to `get_conn`/`refresh`/`reconnect` signatures or
semantics on sequential callers; no shared-future error propagation
(rejected — waiters should not inherit a stale failure during an ongoing
failover); no change to `RedisCommandExecutor` retry policy; no
camel-redis-repo code changes.

## Acceptance criteria

- N concurrent `refresh` calls that complete their invalidation before
  the leader's store (the failover herd shape) produce exactly 1 topology
  resolve + 1 connect; all N return connections backed by the same
  rebuild. Unconditionally: at most one resolve+connect is in flight at
  any instant (a straggler invalidation after the leader's store forces
  at most one additional sequential rebuild).
- N concurrent cold-start `get_conn` calls produce exactly 1 resolve.
- Concurrent refresh where the leader's connect fails never caches a
  failure and never lets a waiter inherit it; waiters attempt
  sequentially (no parallel storm; under persistent outage the accepted
  price is up to N sequential bounded connect attempts).
- The cached fast path takes no gate (healthy steady-state traffic is
  unchanged).
- All pre-existing executor/repo tests pass unchanged; fmt/clippy gates
  green.

## Risk budget

Low-moderate. Internal locking change to one executor; the hazard is
deadlock or lock-order inversion — mitigated by a fixed two-lock ordering
(gate → conn, never nested the other way) and cancellation safety of
tokio's async mutex (gate released if the leader is dropped). Out of
bounds: any public API change, any change to sequential-caller behavior,
any error-type or message change.
