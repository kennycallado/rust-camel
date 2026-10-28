# Design: redis-blocking-pop-timeout

## Root cause (line refs)

- redis 1.7.1 `src/client.rs:180`: `DEFAULT_RESPONSE_TIMEOUT = Some(500 ms)`;
  `:181`: `DEFAULT_CONNECTION_TIMEOUT = Some(1 s)`; `:206-222`:
  `AsyncConnectionConfig::default()` uses both; `:425-430`:
  `get_multiplexed_async_connection()` builds with `AsyncConnectionConfig::new()`
  (`new()` == `default()`).
- `crates/components/camel-redis/src/queue.rs:66-90` (`RedisQueueIo::connect`):
  uses `get_multiplexed_async_connection()` → every BLPOP/BRPOP response races a
  500 ms client-side deadline. Pop timeout > 500 ms ⇒ driver timeout error while
  the server still blocks ⇒ `transient_retry_step` reconnects ⇒ loop on an empty
  list; budget exhaustion ends the route (camel-112).
- `crates/components/camel-redis/src/executor.rs:336-346`
  (`MultiplexedExecutor::get_conn`): the default (`response_timeout: None`)
  branch is also config-less ⇒ same 500 ms deadline for producer commands.
  `commands/list.rs:148-158` builds producer BLPOP/BRPOP with
  `resolve_blocking_timeout` (`:52-58`) whose default is `0` — Redis's
  block-forever sentinel — so a producer BLPOP waits forever against a deadline
  of 500 ms, then fails (BLPOP is non-idempotent ⇒ `NetworkRetryPolicy::disabled()`
  ⇒ immediate Err, no retry loop).

## Audit: blocking commands on timed connections

| Site | Command | Connection build | Response deadline | Verdict |
|---|---|---|---|---|
| `queue.rs:69` `RedisQueueIo::connect` | BLPOP/BRPOP (consumer) | `get_multiplexed_async_connection()` | driver default 500 ms | **BUG — fixed here** |
| `executor.rs:346` `MultiplexedExecutor::get_conn` default arm | any producer cmd, incl. BLPOP/BRPOP (`commands/list.rs`) | `get_multiplexed_async_connection()` | driver default 500 ms | **BUG, same class — fixed here** |
| `executor.rs:339-345` `with_response_timeout(Some)` arm | test-only seam | explicit config | explicit `t` | seam kept; blocking endpoints bypass it |
| `pubsub.rs:65` `RedisPubSubIo::connect` | SUBSCRIBE/PSUBSCRIBE | `get_async_pubsub()` (dedicated PubSub conn) | n/a — push-based, no per-command deadline | OK |
| `health.rs:40` | PING | config-less multiplexed | 500 ms | OK — non-blocking; component wrapper bounds connect; PING reply is sub-ms on a live server |
| `pool_factory.rs:42` | PING (datasource) | config-less multiplexed | 500 ms | OK — non-blocking |
| `commands/mod.rs:547,753` | GET/SETEX in `#[cfg(test)]` | test code | n/a | N/A |
| `crates/services/camel-redis-repo` (`MultiplexedRepoExecutor`) | grep: no BLPOP/BRPOP anywhere | — | — | N/A — the repo executor never runs blocking pops |

## Chosen fix (option 1 from bd rc-y0pwj)

1. **Consumer (`queue.rs`)**: `RedisQueueIo::connect` builds the connection with
   `get_multiplexed_async_connection_with_config(&AsyncConnectionConfig::new()
   .set_response_timeout(None).set_connection_timeout(None))`.
   - `set_response_timeout(None)`: the pop timeout and route cancellation are the
     only bounds; no polling fallback (REDIS-003).
   - `set_connection_timeout(None)`: matches the producer precedent
     (`executor.rs:340-345`) — the component wrapper
     (`tokio::time::timeout(connection_timeout_secs, …)`, unchanged) stays the
     sole connect bound; the driver's 1 s default no longer races it.
   - The `MultiplexedConnection` type is kept: queue mode has exactly one
     consumer per route, so the connection is dedicated in effect; multiplexing
     is inert and no type churn is needed. No timeout-dependent response-margin
     arithmetic (option 2) — `timeout=0` (block forever) is handled by the same
     `None` with no special case.
2. **Producer (`executor.rs`)**: `MultiplexedExecutor::get_conn` computes the
   effective driver deadline as
   `if endpoint command ∈ {Blpop, Brpop} { None } else { self.response_timeout }`.
   The endpoint command is operator-owned URI config, static per endpoint, so
   the decision is per-connection-build, not per-command. `with_response_timeout`
   (test seam) keeps its meaning for non-blocking endpoints; a blocking endpoint
   always builds deadline-free connections.

## Semantics preserved (unchanged on purpose)

- Topology seam + master re-resolution on reconnect (sentinel failover).
- Reconnect backoff and the transient-error budget via `transient_retry_step`;
  `Ok(None)` (pop timeout) still keeps the connection.
- Readiness (`ctx.mark_ready`), cancellation via `CancellationToken`, and
  at-most-once delivery (bd rc-mqoxp).
- Non-blocking producer commands: unchanged deadlines (default arm keeps the
  config-less build; seam arm keeps `Some(t)`).

## Test strategy

Regressions in `crates/components/camel-redis/tests/blocking_pop.rs` against the
in-crate fake RESP server (`tests/common` stub), which can serve a programmed
reply after a delay:

1. **1 s pop > old default**: queue session (`queue_session` + real
   `RedisQueueIo` + fake topology) against a stub that answers BLPOP after
   1 s ⇒ exactly one delivery, `connect` called once (no retry loop), session
   ends cleanly on cancellation.
2. **BRPOP**: same harness with `QueuePopCommand::Brpop`.
3. **timeout=0**: stub withholds the reply > 500 ms (old default) ⇒ no client
   timeout, delivery still arrives; cancellation while blocked ends the session.
4. **Producer BLPOP**: real `MultiplexedExecutor` with endpoint command BLPOP
   and `CamelRedis.Timeout=1` against a stub answering after 800 ms ⇒ command
   succeeds (pre-fix it fails at 500 ms). Unit twin:
   `effective_response_timeout` returns `None` for a blocking endpoint even
   when `with_response_timeout(Some)` was set, and `Some` for a non-blocking
   endpoint.
5. Existing fake-I/O failover tests (`queue.rs` unit tests) must stay green
   unchanged — they prove failover/budget/cancellation semantics did not move.

## Risks

- A blocked pop can now wait arbitrarily long on the socket. Acceptable: route
  cancellation is the bound (same as a plain dedicated-connection consumer), and
  pop timeouts are operator config. The connect path stays bounded by
  `connection_timeout_secs`.
- Producer BLPOP on a shared endpoint executor is still one dedicated
  connection per endpoint; blocking pops on it serialize other commands to the
  same endpoint. Pre-existing property of the endpoint model (one command per
  endpoint URI); documented, not changed.

## Phases

Single phase. Four tasks: consumer fix, producer fix, blocking regressions,
docs/spec delta.
