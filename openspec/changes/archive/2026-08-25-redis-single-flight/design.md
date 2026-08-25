# Design: redis-single-flight

## Approach

Add a single-flight gate to `MultiplexedExecutor` so concurrent
cache-misses share one resolve+connect. New private field
`connect_gate: Arc<tokio::sync::Mutex<()>>`.

`get_conn` becomes:

1. Fast path (unchanged): lock `conn`, return a clone if `Some`, release.
2. Acquire `connect_gate` (the leader holds it across resolve + connect —
   safe because tokio's async mutex is designed for awaits under lock and
   releases on cancellation).
3. Double-check: lock `conn` again; if another leader stored a connection
   while this caller waited on the gate, return it. This is the step that
   collapses N concurrent misses to 1 resolve.
4. Leader path (unchanged body): `topology.resolve(ServerKind::Master)`,
   connect inside the `connection_timeout_secs` wrapper (with the
   `response_timeout` branch from change `redis-response-timeout`),
   store under `conn`, release gate implicitly on return.

`refresh()` is structurally unchanged (drop cache, call `get_conn`): with
the gate, N concurrent refreshes each drop (idempotent, serialized briefly
on `conn`), then serialize on the gate; the first resolves and connects,
the rest double-check into the result. Sequential callers (retry loops)
see identical behavior to today — the gate is uncontended and free.

Collapse-contract scope (straggler case): a `refresh` caller whose drop
phase lands AFTER the leader's store clears the freshly rebuilt
connection and becomes the next leader — one extra sequential rebuild.
The unconditional guarantee is storm elimination (at most one
resolve+connect in flight at any instant); exact single-resolve holds for
the herd that invalidated before the leader's store, which is the
failover shape the issue describes (all callers fail on the same dead
connection within one failure event). Generation-stamped invalidation
(collapse even stragglers to zero extra rebuilds) was rejected as
over-engineering for that edge.

Failure semantics: if the leader's resolve or connect fails, nothing is
cached (existing rule) and the gate releases; the next waiter
double-checks to an empty cache and becomes the new leader with its own
resolve. Concurrent failures are therefore sequential, not parallel —
this is deliberate. During a failover the sentinel's answer improves with
time (master promotion completes), so a waiter's fresh resolve sees newer
cluster state than the stale failure it would inherit under a
shared-future design. Concurrency of the storm is bounded to 1 while each
caller still obtains its own authoritative answer. The accepted trade:
under a PERSISTENT outage, N concurrent refresh callers pay up to N
sequential bounded `connection_timeout_secs` attempts (tail latency N×
the connect bound) instead of N parallel ones — herd pressure on the
stressed cluster is eliminated; latency is the price.

Cancellation safety: the gate owner holds a tokio async mutex, which
releases when the owner task is dropped — a dropped leader lets a waiter
proceed immediately. The component's topology resolve internally
offloads blocking sentinel work to `spawn_blocking`; if the leader is
dropped mid-offload, that blocking work may run to completion and its
result is discarded — harmless (no gate held, no cache write).

Lock ordering: gate before conn, never conn while holding gate beyond the
short guarded sections; the two locks are never held across each other's
acquisition in the reverse order, so no inversion cycle exists.

## Affected crates

- `camel-redis` (component): `executor.rs` — one field, reordered
  `get_conn`; unit tests (counting topology, dead-address leader failure);
  integration tests in `tests/` (success-collapse via a handshake stub
  with a hold-release gate so the herd provably piles up behind the
  leader).
- `crates/components/camel-redis/CONTEXT.md` — document the
  single-flight contract in the connection-ownership section.
- Test infrastructure: extract the RESP handshake stub currently in
  `tests/response_timeout.rs` into `tests/common/mod.rs` (shared by both
  integration targets — this is the "consolidate on a copy" trigger the
  previous change's review recorded), extended with a
  hold-handshake-until-signaled mode for herd determinism.

## Architecture boundaries

Data-plane only; component-internal. No public surface change (gate is a
private field), no config/schema/DSL involvement, no repository service
crate change — the repo inherits the fix through the unchanged
`get_conn`/`refresh` seams. Composes with `redis-response-timeout`: the
leader's connect still applies `with_response_timeout` settings, so
rebuilt connections carry the same deadline configuration.

## Alternatives considered

- **Hold the existing `conn` mutex across resolve+connect**: serializes
  but does not collapse — each waiter wakes to an empty cache and
  re-resolves sequentially; also blocks the fast path for readers during
  the whole connect. Rejected.
- **Shared future (`futures::FutureExt::shared`) cached in-flight, all N
  await one outcome**: collapses failures too, but propagates one stale
  failure to every waiter during failover (exactly when a later resolve
  would succeed), adds a generic `Shared<Pin<Box<dyn Future>>>` field and
  error-clone plumbing. Rejected in favor of per-caller authority.
- **Error TTL cache (remember failures briefly)**: turns transient
  failover states into served errors; adds tuning surface. Rejected.

## Testing strategy

Deterministic, no wall-clock races. Herd tests run on the
single-threaded `current_thread` tokio runtime (`#[tokio::test(flavor =
"current_thread")]`) so task interleaving is cooperative and fully
controlled by await points:

- Unit (executor.rs `mod tests`): `cached_fast_path_skips_gate` —
  with a healthy cached connection and the gate held EXTERNALLY through a
  `#[cfg(test)] gate_arc()` accessor (same pattern as `conn_arc`),
  `get_conn` returns from cache inside a 5 s bound (a gate-contending
  fast path would time out). Rationale: whenever a real leader holds the
  gate the cache is empty by construction (refresh drops it first;
  cold-start never filled it), so the gate-free fast path can only be
  proven by external gate ownership. Existing reconnect/refresh tests
  pass unchanged (transparent uncontended gate).
- Integration (`tests/single_flight.rs` + `tests/common/mod.rs`): the
  handshake stub gains hold modes — `start_silent_held` (handshake
  withheld from the start), `hold()` (latch new handshakes), `release()`
  (watch back to Free AND `Notified::enable()` wake), and
  `start_rejecting_after_hold` (held handshakes are closed unreplied on
  release and future connects are rejected — a deterministic FAILING
  leader). Each herd test: spawn N tasks, each incrementing a
  started-counter IMMEDIATELY before entering `refresh`/`get_conn`;
  await started == N (yield_now polling on the current_thread runtime);
  only then release; `join_all`; assert exact resolve counts
  (reset-before-phase where a pre-build connect happened). Named tests:
  `concurrent_refresh_collapses_to_one_resolve` (== 1),
  `cold_start_get_conn_collapses_to_one` (== 1),
  `dropped_leader_releases_gate_and_waiter_proceeds` (== 2: the aborted
  leader's resolve counted plus the waiter's own),
  `straggler_invalidation_forces_one_sequential_rebuild` (== 2:
  leader joined before the straggler starts, so its drop lands after the
  store by construction),
  `waiter_does_not_inherit_leader_failure` (== 3 against the rejecting
  stub: each of three parked callers ran its own resolve; a
  shared-future design would show 1). Outer 30 s liveness timeouts on
  every herd; no wall-clock sleeps.
