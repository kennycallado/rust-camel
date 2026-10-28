# Proposal: redis-streams-consumer-groups

## Why

bd rc-euud8 (camel-112): users need Redis Streams with consumer groups for
competing-consumer workloads (at-least-once delivery, horizontal scale-out).
Today `RedisConsumer::new` accepts only SUBSCRIBE, PSUBSCRIBE, BLPOP and
BRPOP (`crates/components/camel-redis/src/consumer.rs:94`); an
`XREADGROUP` URI fails with REDIS-003's "invalid consumer command" error.
There is no path to consume a stream as a group member.

## What Changes

**Included (bounded first slice):**

- New consumer mode `Streams` activated by `command=XREADGROUP` with URI
  params `group`, `consumer`, optional `count`, optional `block`
  (milliseconds, default 10000; `0` = block indefinitely).
- XREADGROUP reads with the `>` ID (new entries only), one Exchange per
  stream entry, delivered through `ConsumerContext::send_and_wait` (the
  completion-bearing handoff: ack requires knowing the pipeline
  outcome; fire-and-forget `send` cannot provide that).
- Delivery metadata on every Exchange: `CamelRedis.Stream` (key),
  `CamelRedis.StreamId`, `CamelRedis.StreamGroup`,
  `CamelRedis.StreamConsumer`, `CamelRedis.StreamDeliveryCount` (always
  `1` in this slice: `>` reads are first deliveries).
- Ack policy: `XACK` per entry on the session connection, issued only
  after that entry's `send_and_wait` completes successfully. Delivery
  failure or cancellation leaves the entry pending (at-least-once); a
  transient ack transport error leaves the ack outcome unknown (no PEL
  claim; recovery deferred). No fallback to BLPOP (REDIS-003).
- Failover/readiness/cancellation follow the queue-mode patterns:
  resolve → connect → read loop on one deadline-free connection,
  transient errors re-resolve and reconnect under the retry budget,
  non-transient errors (e.g. `NOGROUP`) return `Err` for ADR-0007
  supervision.
- Deterministic tests on the `StreamIo` seam plus a scripted-reply fake
  RESP server (the `blocking_pop_tests.rs` pattern): delivered+acked,
  failure → no ack, cancellation while blocked, readiness, malformed
  group/consumer names fail closed.

**Excluded (deferred, bd follow-ups):** pending-entry recovery
(XPENDING/XAUTOCLAIM), multi-consumer rebalancing support, trimming
policy, batching beyond `count`, automatic group creation (`NOGROUP`
fails closed; the operator creates the group).

## Acceptance criteria

- `cargo test -p camel-component-redis` green, including the new stream
  tests (seam + fake server).
- `cargo fmt --check --all`; `cargo clippy -p camel-component-redis
  --all-targets -- -D warnings`; mission lint gates (lint-unwrap,
  lint-test-sleep, lint-unbounded-wait, lint-glob-imports,
  lint-unsafe-safety, lint-missing-docs-ratchet); `RUSTDOCFLAGS="-D
  warnings" cargo doc -p camel-component-redis --no-deps` all green.
- `openspec validate --all --strict` passes; delta archived before park.
- No ack is issued when the pipeline send fails or the route is
  cancelled; verified by tests that observe XACK on the wire / seam.

## Risk budget

Acceptable: `delivery count` is always 1 in this slice (documented
invariant of `>` reads). Out of bounds: new dependencies, producer-side
XADD (unless read tests need it), workspace bump, silent fallbacks,
auto group creation.

Affected crates: `camel-component-redis` only. bd: rc-euud8.
