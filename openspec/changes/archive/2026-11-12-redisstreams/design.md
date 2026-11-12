# Design: redisstreams

## Context

Consumer-side Streams support landed (rc-euud8, rc-x8i4k): `XREADGROUP`
reads with group/consumer/count/block on a deadline-free connection,
`XACK` only after pipeline success, `XPENDING`/`XAUTOCLAIM` recovery.
The producer path has zero stream commands, and the producer lets the
`CamelRedis.Command` header override the URI command (producer.rs
`resolve_command`). The 2026-11-11 owner + e_opus audit settled the design;
the decisions below are binding and are not reopened here.

## Goals / Non-Goals

Goals: producer-side `XADD`/`XLEN`; explicit opt-in group creation on the
consumer; header-override removal with a rejection guarantee; code-based
classification of the three stream server codes; docs and specs updated.

Non-Goals (later phases, bd-tracked): trimming/ACKED (rc-eoh8t),
XRANGE/XREVRANGE (rc-rfwcr), XINFO lag (rc-k0tkz), IDMP (rc-cr0s1),
producer-side XACK/XCLAIM/XSETID, XGROUP DESTROY/DELCONSUMER and XDEL as
route commands, XREAD BLOCK on the producer, cluster topology, broker
abstraction, non-UTF-8 outside-writer stall (rc-c1t1m).

## Decisions (binding, owner + e_opus audit 2026-11-11)

### D1. Header override removed (breaking)

`resolve_command` returns the URI command only. If `CamelRedis.Command`
is present on a producer exchange — whatever its value, valid or not — the
exchange fails with a named `CamelError::ProcessorError` naming the header,
before any connection is attempted. Existing unit tests that pin the
override/fallback behavior are rewritten to pin the rejection. Docs gain a
migration note (`choice`/`when`, one endpoint per command).

### D2. XADD encoding

One fixed field named `body`; value is `exchange.body.try_into_text()`
(text as-is, JSON compact, UTF-8 bytes as text). `Body::Empty`, `Body::Stream`,
and non-UTF-8 bytes fail with a named processor error before sending.
Entry ID is always `*`. Stream key comes from `CamelRedis.Key`, falling
back to the URI `key=`. The body is KEPT (not replaced with the reply);
`CamelRedis.StreamId` is set to the returned ID and `CamelRedis.Stream`
to the key, overwriting upstream values. Round-trip contract: XADD of text
`S` is read by the existing XREADGROUP consumer as `Body::Text(S)` byte for
byte.

`XLEN` returns the entry count as the body (a read; body replaced).

### D3. Group creation

Default stays operator-owned: `createGroup=false` keeps today's fail-closed
NOGROUP. Opt-in `createGroup=true` REQUIRES `groupStart=latest|earliest`
(no default); they map to `$`/`0`. The session sends
`XGROUP CREATE <key> <group> <start> MKSTREAM` once per session on the read
connection before the first XREADGROUP; readiness (`mark_ready`) fires only
after success. `BUSYGROUP`, matched by `RedisError::code()`, is success with
an `info!` log. The existing group always wins; `XGROUP SETID` is never
sent. `NOPERM`/`WRONGTYPE` are non-transient and go to supervision.
`groupStart` without `createGroup=true` fails at URI parse time.

### D4. Version floor 7.0

Docs state "Redis >= 7.0 (Valkey >= 7.2 compatible)". `ACKED` (8.2) and
`IDMP` (8.6) are out of scope; when added later they must fail closed below
their floor. The live test pins `redis:7.0-alpine`.

### D5. At-least-once

XADD is not idempotent and stays out of `is_idempotent_command`; the
component never retries it. An ambiguous transport failure returns `Err`
with unknown outcome. Duplicates on route redelivery and the Idempotent
Consumer + `RedisIdempotentRepository` mitigation are documented.
`CamelRedis.StreamId` is NOT a dedup key.

### D6. Classification precedence (redis-failover delta)

`is_transient_redis_error` gains a named-code rule ahead of the legacy
substring fallback: a preserved `redis::RedisError` whose server code
(`RedisError::code()`, verified present in redis-rs 1.x) is `NOGROUP`,
`BUSYGROUP`, or `NOPERM` is not transient, decided by the code alone. The
legacy-verdict identity guarantee now EXEMPTS these three codes: a legacy
classifier word inside their messages (e.g. `NOPERM connection denied`)
deliberately diverges from the legacy table and stays not-transient. For
every other realizable input the verdict table remains behaviorally
identical to the legacy table. This makes the verdict explicit and
order-stable for the group-creation logic.

## Test Strategy

- Unit tests: `resolve_command` rejection matrix (valid header, invalid
  header, non-string header, injected DEL on GET endpoint); URI parse
  matrix for `createGroup`/`groupStart`; `is_idempotent_command` excludes
  XADD; classification by code (NOGROUP/BUSYGROUP/NOPERM vs WRONGTYPE).
- Fake-server wire tests (existing `stream_fake_tests` RESP harness):
  XADD frame shape (`XADD key * body <text>`, field order, ID `*`), XLEN
  frame, headers/body postconditions; group creation wire (`XGROUP CREATE
  key group $|0 MKSTREAM` once, before first XREADGROUP, readiness after);
  BUSYGROUP-as-success, NOPERM non-transient, transient-error recovery.
- Live testcontainers round trip (`redis:7.0-alpine`, `#[ignore]`-free but
  infra-gated by Docker like existing camel-test redis tests): XADD ->
  createGroup consumer -> XACK -> XLEN, pending list empty after ack.
- Gates: fmt, clippy `-D warnings` on touched crates, crate tests, live
  redis test, doc build with touched crates, `bash scripts/gates-local.sh`.

## Phases

Single delivery phase (phase 1 of the mission roadmap). Ordered task flow:
header rejection first (unblocks safe command surface), then classification,
then producer commands, then consumer group creation, then live round trip,
then docs. Each task is independently reviewable against the delta specs.

## Migration

`fix(redis)!`: routes that relied on `CamelRedis.Command` to switch commands
per message must use one endpoint per command (`choice`/`when` EIP with
producers whose URI pins the command). Header presence now fails the
exchange before connecting. Documented in `docs/src/components/redis.md`.
