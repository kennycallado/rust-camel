# Proposal: redis-blocking-pop-timeout

## Why

External report (camel-112, camel-redis 0.61.1, bd rc-y0pwj): a route
`redis://...?command=BLPOP&key=...&timeout=1` with an empty list logs
`camel_component_redis::retry "timed out ... stage=popping with BLPOP"`
roughly every 0.5 s, before the 1 s pop timeout elapses, with a growing
attempt counter; `timeout=5` exhausted the transient budget after 10
attempts and ended the route.

Root cause, confirmed in code and in the redis 1.7.1 driver source:
`crates/components/camel-redis/src/queue.rs` (`RedisQueueIo::connect`)
builds the queue connection with `Client::get_multiplexed_async_connection()`,
which uses `AsyncConnectionConfig::new()` == `Default`, i.e.
`response_timeout: Some(500 ms)` (`DEFAULT_RESPONSE_TIMEOUT`,
redis-1.7.1 `src/client.rs:180`). A BLPOP/BRPOP whose pop timeout exceeds
500 ms returns a client-side timeout while the server is still blocking;
the queue failover loop classifies it transient and reconnects. The same
defect class exists on the producer surface: `RedisCommand::Blpop/Brpop`
(`commands/list.rs`) travel through `MultiplexedExecutor`, whose default
connection is also config-less (500 ms deadline), and the producer
blocking-timeout default is `0` = block forever.

## What Changes

- Queue mode (`RedisQueueIo::connect`) builds its connection with
  `AsyncConnectionConfig::set_response_timeout(None)` (and
  `set_connection_timeout(None)`, matching the producer precedent), so a
  blocking pop is bounded only by the pop timeout itself and by route
  cancellation. One consumer per route, so multiplexing buys nothing; the
  connection is dedicated in effect.
- `MultiplexedExecutor::get_conn` forces the driver response deadline off
  when the endpoint command is `BLPOP`/`BRPOP` (the endpoint command is
  static per endpoint), regardless of the `with_response_timeout` test
  seam. Non-blocking commands keep today's behavior unchanged.
- No polling fallback (REDIS-003). Master resolution/failover, reconnect
  backoff, transient budget, readiness (`ctx.mark_ready`), cancellation,
  and at-most-once per-item delivery are unchanged.
- Blocking regressions against the in-crate fake RESP server: 1 s pop
  (longer than the old 500 ms default) delivers once with no reconnect,
  BRPOP variant, `timeout=0` (infinite block) with clean cancellation,
  and the producer BLPOP path. Docs updated (component page + README).

## Acceptance criteria

- `cargo test -p camel-redis` green including the new blocking regressions.
- `cargo fmt --check --all`, `cargo clippy -p camel-redis --all-targets
  -- -D warnings`, `cargo xtask lint-unwrap`, `lint-test-sleep`,
  `lint-unbounded-wait`, `RUSTDOCFLAGS="-D warnings" cargo doc -p
  camel-redis --no-deps` pass.
- Fake-server regression proves: server blocking > 500 ms produces exactly
  one delivery, zero reconnects, clean cancellation.
- Component docs state the queue-mode connection rule.

## Risk budget

Small, contained: two connect-site changes in `camel-redis` plus tests and
docs. Out of bounds: Redis Streams/XREADGROUP (rc-euud8), new consumer
commands, API redesign, workspace/bridge bumps, `#[deprecated]`, changes to
non-blocking command timeout behavior.

## Affected crates

- `camel-component-redis` (code + tests + README)
- `docs` (component page)
