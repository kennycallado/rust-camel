## ADDED Requirements

### Requirement: Single-flight connection refresh in MultiplexedExecutor

The component `camel-redis` `MultiplexedExecutor` SHALL collapse
concurrent cache-miss connection builds into a single topology resolve and
connect, through a dedicated single-flight gate: the first cache-miss
caller becomes the leader and holds the gate across resolve+connect, while
other concurrent callers park on the gate and then take the connection the
leader stored (double-check after the gate). The collapse contract is
scoped: it holds for callers whose cache invalidation (the drop phase of
`refresh`, or the cache-miss observation of `get_conn`) completed before
the leader's store; a straggler whose invalidation lands after the
leader's store clears the rebuilt connection and forces at most ONE
additional rebuild, sequentially — the parallel-storm elimination (at most
one resolve+connect in flight at any instant) holds unconditionally. The
cached fast path SHALL remain gate-free — steady-state traffic on a healthy
cached connection is unchanged. If the leader's resolve or connect fails,
nothing SHALL be cached (existing rule) and each waiter SHALL run its own
attempt sequentially; waiters SHALL NOT inherit another caller's failure
outcome. Under a persistent outage this sequential-failure policy trades
herd pressure for tail latency (up to N sequential bounded
`connection_timeout_secs` attempts) — accepted and documented. The gate
SHALL be cancellation-safe for the gate owner (a dropped leader releases
the gate so a waiter proceeds; sentinel resolve work already offloaded may
run to completion discarded, harmlessly). Sequential callers SHALL observe
behavior identical to the pre-change executor. No public signature of
`get_conn`, `refresh`, or `reconnect` SHALL change.

#### Scenario: concurrent refreshes during one failover collapse to one resolve

- **GIVEN** an executor whose cached connection is dead, a counting
  topology in front of a handshake-completing stub whose handshake is held
  until N concurrent `refresh` callers have all completed their drop phase
  (a started-counter incremented immediately before entering `refresh`,
  awaited == N before release) and are parked behind the leader
- **WHEN** the stub releases the handshake and all N refreshes complete
- **THEN** the topology resolve count is exactly 1 and every caller
  receives a connection backed by the same rebuild

#### Scenario: concurrent cold-start get_conn calls collapse to one resolve

- **GIVEN** a freshly constructed executor (empty cache) and N concurrent
  `get_conn` callers, piled up behind a held handshake with all N
  confirmed past the fast path before release (started-counter == N)
- **WHEN** the handshake releases and all N calls complete
- **THEN** the resolve count is exactly 1

#### Scenario: waiter does not inherit a leader failure

- **GIVEN** a cache-miss leader whose connect fails deterministically
  (dead address) and concurrent waiters parked behind the gate
- **WHEN** the leader fails and a waiter proceeds
- **THEN** the waiter performs its own resolve attempt (sequential, one
  in-flight at a time) rather than receiving the leader's error without
  attempting; no failed connection or error is cached

#### Scenario: dropped leader releases the gate and a waiter proceeds

- **GIVEN** a leader parked in its connect behind a held handshake, with a
  waiter parked on the gate
- **WHEN** the leader task is dropped (aborted) without storing
- **THEN** the gate releases and the waiter becomes the new leader (its
  own resolve proceeds; nothing the dropped leader left is cached)

#### Scenario: straggler invalidation forces at most one sequential extra rebuild

- **GIVEN** a leader that has just stored a rebuilt connection, and a
  straggler `refresh` caller whose drop phase lands after that store
- **WHEN** the straggler proceeds
- **THEN** it clears the stored connection and performs one additional
  resolve+connect while holding the gate (bounded: one in flight, never a
  parallel storm)

#### Scenario: cached fast path takes no gate

- **GIVEN** an executor holding a healthy cached connection
- **WHEN** concurrent `get_conn` calls arrive
- **THEN** they are served from the cache without contending the
  single-flight gate (steady-state behavior unchanged)

#### Scenario: sequential reconnect semantics unchanged

- **GIVEN** the existing executor tests for `refresh`/`reconnect`
  re-resolution and failure-not-cached behavior
- **WHEN** they run against the gated executor
- **THEN** they pass unchanged (uncontended gate is transparent)
