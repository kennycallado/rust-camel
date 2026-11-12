# Proposal: redisstreams

## Why

The Redis Streams consumer side is complete (`XREADGROUP` + `XACK`,
`XPENDING`/`XAUTOCLAIM`), but the producer side has no stream commands:
`RedisCommand` only parses `XREADGROUP`, and the executor rejects it on
producer endpoints. A route can consume a stream but cannot write one,
create its group, or inspect its length. Redis Streams therefore cannot be
the broker of a rust-camel-only deployment (bd rc-3rnbf).

Separately, `RedisProducer::resolve_command` lets the `CamelRedis.Command`
exchange header override the URI command and silently falls back on parse
errors. Exchange headers are untrusted (ADR-0032); this override would make
any destructive producer command reachable from message data, and it
contradicts the documented trust boundary (bd rc-oow9x, P1).

## What Changes

- Remove the `CamelRedis.Command` header override. The endpoint URI is the
  only command source. Header presence fails the exchange with a named
  processor error before connecting. Breaking: `fix(redis)!`.
- Add producer commands `XADD` (one fixed field `body` = `try_into_text`,
  entry ID `*`, body kept, `CamelRedis.StreamId`/`CamelRedis.Stream` set)
  and `XLEN` (entry count as body).
- Add consumer options `createGroup` (default `false`, operator-owned) and
  `groupStart` (`latest`/`earliest`, required when `createGroup=true`);
  session sends `XGROUP CREATE ... MKSTREAM` once before the first read;
  `BUSYGROUP` matched by server code is success; readiness only after that.
- Classify `NOGROUP`/`BUSYGROUP`/`NOPERM` by `RedisError::code()` ahead of
  the legacy substring fallback.
- New canonical capability `redis-streams-producer`; MODIFIED deltas for
  `redis-streams-consumer`, `redis-failover` (classification precedence),
  and `redis-geo` (override removed).
- Docs: XADD/XLEN producer usage, producer headers, at-least-once notes,
  Redis >= 7.0 floor, migration note for the header removal.

Excluded (later phases, tracked in bd): MAXLEN/MINID trimming and ACKED
(rc-eoh8t), XRANGE/XREVRANGE (rc-rfwcr), XINFO lag (rc-k0tkz), XADD IDMP
(rc-cr0s1), producer-side XACK/XCLAIM/XSETID, XGROUP DESTROY/DELCONSUMER,
XDEL as route commands, XREAD BLOCK on the producer, cluster topology,
broker abstraction, non-UTF-8 outside-writer stall (rc-c1t1m).

## Acceptance criteria

- Injected `DEL` header on a `command=GET` producer is rejected before any
  connection is made (regression test).
- `XADD` round-trips text byte-for-byte through the existing `XREADGROUP`
  consumer as `Body::Text`.
- `createGroup=true` without `groupStart`, and `groupStart` without
  `createGroup=true`, fail at URI parse time.
- Live testcontainers round trip on `redis:7.0-alpine`: XADD ->
  createGroup consumer -> XACK -> XLEN, pending list empty.
- `cargo fmt --check`, clippy `-D warnings` on touched crates, crate tests,
  live redis test, and `bash scripts/gates-local.sh` exit 0.

## Risk budget

Breaking-behavior risk is bounded and intentional (header override was a
documented anti-pattern). XADD is non-idempotent; the component never
retries it and an ambiguous transport failure returns `Err` with unknown
outcome — documented as at-least-once with the Idempotent Consumer
mitigation. No cluster, scripting, or trimming surface is touched.

## Affected crates

`camel-redis` (component), `camel-test` (live round-trip test),
`docs` (component guide). Bd: rc-3rnbf, rc-oow9x.
