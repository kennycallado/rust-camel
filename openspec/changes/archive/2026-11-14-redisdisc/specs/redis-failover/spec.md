## MODIFIED Requirements

### Requirement: producer reconnects to the elected master after failover

The producer SHALL route connection establishment through the topology seam.
The executor's cached connection SHALL be generation-tagged. When a command
fails inside `MultiplexedExecutor::execute_command` with an error classified
invalidating by `invalidates_connection` (connection refused / reset / aborted /
broken pipe, driver connection-dropped, read-only-role error, transport
timeout), the executor SHALL invalidate the cached connection with a
compare-and-clear (clear only if the failing generation is still cached) and
SHALL rethrow the original error unchanged — it SHALL NOT dial in the error
path and SHALL NOT retry the command; the next `get_conn` SHALL rebuild
lazily under the existing single-flight connect gate and re-resolve the
topology master. On a qualifying transport failure of an idempotent command
while a `SentinelTopology` is configured, the producer SHALL resolve the
current master via `topology.resolve(Master)` and reconnect — it SHALL NOT
retry the command against the previous (potentially dead) master. The bounded
reconnect loop SHALL honor `NetworkRetryPolicy` (ADR-0013); its
reconnect-before-retry SHALL be generation-aware (compare against the
executor instance's last-used generation) so N producer clones failing on one
socket SHALL NOT clobber each other's fresh connection. After the budget is
exhausted the producer SHALL return `Err`. A non-idempotent command that
fails with an ambiguous transport error SHALL NOT be retried (the producer
returns `Err` immediately), but its failing connection SHALL already be
invalidated, so the next command reconnects.

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
- **THEN** it returns `Err` without re-resolving or re-executing, the original
  error is rethrown unchanged, and the cached connection is invalidated

#### Scenario: producer returns Err after failover budget exhaustion

- **GIVEN** a producer whose topology always fails to resolve a usable master
- **WHEN** the producer retries an idempotent command up to `max_attempts`
- **THEN** it returns `Err` (does not loop forever)

#### Scenario: killed connection does not poison the next command

- **GIVEN** a producer whose cached connection was killed server-side
  (`CLIENT KILL`) so its next non-idempotent command fails with an
  invalidating transport error
- **WHEN** the following command is processed
- **THEN** the executor rebuilds the connection under the connect gate,
  re-resolves the topology master, and the command succeeds on the fresh
  connection

#### Scenario: clones failing on one socket do not clobber a fresh connection

- **GIVEN** two producer clones sharing one executor cache, both having used
  generation N, and both commands failing with invalidating errors
- **WHEN** the first clone invalidates and rebuilds (generation N+1) and the
  second clone runs its generation-aware reconnect
- **THEN** the second clone keeps the N+1 connection (its last-used generation
  N no longer matches the cache) and the retried command runs on the fresh
  connection

### Requirement: sentinel-aware health check

`RedisHealthCheck` SHALL resolve the **current** master through the configured
topology and PING it, rather than probing a stale configured address. If the
topology cannot resolve a master (sentinels unreachable, no validated master),
the health check SHALL report `Unhealthy`; it SHALL NOT report `Healthy` by
probing the stale configured node. The producer health check
(`check_connection`) SHALL preserve the PING error's source chain (the error
SHALL NOT be flattened to a source-less `ProcessorError`) so classifiers can
read it, and SHALL invalidate the cached connection with a compare-and-clear
when the PING failure classifies invalidating — a failed health probe SHALL
NOT leave a dead connection cached, so health recovers on the next probe.

#### Scenario: health check pings the current master, not the configured node

- **GIVEN** a `RedisHealthCheck` with a `FakeTopology` that switched from A to B
- **WHEN** `check()` runs
- **THEN** it PINGs B and reports `Healthy` (it does not probe A)

#### Scenario: health check is unhealthy when no master is resolvable

- **GIVEN** a `RedisHealthCheck` whose topology resolves no usable master
- **WHEN** `check()` runs
- **THEN** it reports `Unhealthy`

#### Scenario: health recovers after the connection is killed

- **GIVEN** a producer whose cached connection is killed server-side
- **WHEN** `check_connection` PINGs the dead connection and fails, then runs
  again
- **THEN** the first run returns `Err` (with the driver error preserved as
  source) and invalidates the cache, and the second run rebuilds the
  connection and succeeds

### Requirement: Single-flight connection refresh in MultiplexedExecutor

The component `camel-redis` `MultiplexedExecutor` SHALL collapse
concurrent cache-miss connection builds into a single topology resolve and
connect, through a dedicated single-flight gate: the first cache-miss
caller becomes the leader and holds the gate across resolve+connect, while
other concurrent callers park on the gate and then take the connection the
leader stored (double-check after the gate). The executor cache SHALL be
generation-tagged (`Option<(u64, MultiplexedConnection)>`) with a shared
generation allocator, so every stored connection carries the generation of its
build. The collapse contract is scoped: it holds for callers whose cache
invalidation (the unconditional drop phase of `refresh`, the generation-aware
drop phase of `reconnect`, or the cache-miss observation of `get_conn`)
completed before the leader's store; a straggler whose invalidation lands
after the leader's store clears the rebuilt connection and forces at most ONE
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
run to completion discarded, harmlessly). `refresh` SHALL keep its
unconditional drop-and-rebuild contract and `get_conn` SHALL keep its
signature; sequential `refresh`/`get_conn` callers SHALL observe behavior
identical to the pre-change executor. `reconnect` SHALL become
generation-aware: its drop phase SHALL clear the cache only when the cached
generation equals the calling executor instance's last-used generation, so N
clones failing on one socket SHALL NOT serially clobber each other's fresh
connection; a `reconnect` following this instance's own failed command
SHALL still drop the failed connection and re-resolve. No public signature
of `get_conn`, `refresh`, or `reconnect` SHALL change.

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

<!-- openspec-scenario-ops
renamed: sequential reconnect semantics unchanged -> sequential refresh and get_conn semantics unchanged
-->

#### Scenario: sequential refresh and get_conn semantics unchanged

- **GIVEN** the existing executor tests for `refresh` re-resolution,
  failure-not-cached behavior, and gate-free fast-path service
- **WHEN** they run against the generation-tagged executor
- **THEN** they pass unchanged (uncontended gate is transparent;
  `refresh` still clears unconditionally and re-resolves exactly once)

#### Scenario: reconnect after this instance's failed command re-resolves

- **GIVEN** an executor whose `execute_command` checked out generation N,
  failed with an invalidating error, and whose retry loop now calls
  `reconnect`
- **WHEN** the generation-aware drop phase compares its last-used generation
  N against the cache
- **THEN** the failed connection is cleared and the reconnect re-resolves the
  topology master

#### Scenario: blind reconnect on an unused instance keeps the cache

- **GIVEN** an executor instance that never executed a command (last-used
  generation unset for any cached generation) while the cache holds another
  caller's generation
- **WHEN** a standalone `reconnect` runs
- **THEN** the generation mismatch keeps the cached connection and no
  rebuild is forced

## ADDED Requirements

### Requirement: transport-failure connection invalidation classifier

The system SHALL provide `invalidates_connection(&CamelError) -> bool` in
`camel-component-redis`'s transport error module. It SHALL reuse the bounded
source-chain walk of `is_transient_redis_error` (same hop bound, same Arc
unwrapping) and SHALL classify invalidating exactly: the `TransportTimeout`
marker; a `redis::RedisError` of kind `Server(ReadOnly)`; a `redis::RedisError`
of kind `Io` whose inner `std::io::Error` kind is `ConnectionRefused`,
`ConnectionReset`, `ConnectionAborted`, or `BrokenPipe`, or whose Display
contains the driver's connection-dropped phrase; nothing else. `Server(ReadOnly)`
is the ONE server reply that invalidates; every other server error reply
(including settled-fatal codes and server text matching transient fallback
words) SHALL NOT invalidate. Config errors, budget-exhaustion and prose
markers, `ClusterConnectionNotFound`, io timeouts, `CamelError::Io`
(transient=true, invalidating=false — its chain carries no structural
evidence), and plain `ProcessorError` text SHALL NOT invalidate. The
classifier SHALL be a strict subset of `is_transient_redis_error`: for every
error input, `invalidates_connection(e) ⇒ is_transient_redis_error(e)`, pinned
by a property test over the crate's error corpus. The system SHALL NOT
introduce a new `CamelError` variant for this.

#### Scenario: invalidating shapes classify true

- **GIVEN** each of: a `TransportTimeout`-marked error, a `Server(ReadOnly)`
  reply, an Io-kind RedisError with an inner `ConnectionReset` io error, and
  an Io-kind RedisError whose Display contains the driver connection-dropped
  phrase
- **WHEN** `invalidates_connection` classifies each
- **THEN** every verdict is true

#### Scenario: other server replies never invalidate

- **GIVEN** each of: NOGROUP, BUSYGROUP, NOPERM, WRONGTYPE, and a
  server reply whose text contains a transient fallback word ("ERR connection
  lost while processing")
- **WHEN** `invalidates_connection` classifies each
- **THEN** every verdict is false (only `Server(ReadOnly)` invalidates among
  server replies) while `is_transient_redis_error` still reports transient for
  the fallback-word reply

#### Scenario: subset property holds over the corpus

- **GIVEN** the full corpus of errors the crate's boundaries can produce at
  the invalidation call sites (every transient fixture, every fatal fixture,
  every marker, config errors, io variants, plain ProcessorError, chained
  wraps)
- **WHEN** the property test evaluates both classifiers on each corpus entry
- **THEN** no entry satisfies `invalidates_connection` without also satisfying
  `is_transient_redis_error`

### Requirement: consumer retry budget counts consecutive failures

The PubSub and Queue consumer loops SHALL measure their retry budget as
consecutive failures since the last proven round trip, not lifetime failures.
The Queue loop SHALL reset its attempt counter on `Ok(Some)` and `Ok(None)`
from the blocking pop (a blocking-pop timeout is a proven server round trip).
The PubSub loop SHALL reset its attempt counter when the stream ends after the
connection stayed up at least `policy.max_delay` (a quiet channel delivers
nothing, so uptime is the only health proof); a stream end before that
threshold SHALL NOT reset. No loop SHALL reset its counter on connect.
Recovered blips SHALL therefore not consume budget across unrelated
incidents, and a run of consecutive failures SHALL still exhaust the budget
and return `Err` for Route supervision (ADR-0007).

#### Scenario: queue budget survives repeated recovered blips

- **GIVEN** a queue session whose connection experiences N transient pop
  failures, each followed by a successful reconnect and a successful pop
- **WHEN** N exceeds the configured `max_attempts`
- **THEN** the session has not exhausted its budget (the counter reset on each
  successful pop)

#### Scenario: queue budget still exhausts on consecutive failures

- **GIVEN** a queue session whose pops fail transiently on every attempt
- **WHEN** consecutive failures reach `max_attempts`
- **THEN** the session returns `Err` (budget exhaustion)

#### Scenario: pubsub resets after a long-lived connection ends

- **GIVEN** a pubsub session whose connection stayed up at least
  `policy.max_delay` before its stream ended
- **WHEN** the session reconnects after the stream end
- **THEN** the attempt counter is reset before the stream-end failure is
  counted

#### Scenario: pubsub does not reset after a short-lived connection ends

- **GIVEN** a pubsub session whose connection died before
  `policy.max_delay` elapsed
- **WHEN** the stream ends
- **THEN** the failure consumes budget without a reset (rapid connect-drop
  cycles still exhaust and reach supervision)
