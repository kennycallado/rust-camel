# Design: redis-streams-consumer-groups

## Approach

Mirror the queue-mode architecture (`queue.rs`): an injectable I/O seam +
a failover session loop, wired into `RedisConsumer` as a third mode.

- `config.rs`: add `RedisCommand::Xreadgroup` (+ `"XREADGROUP"` in
  `FromStr`) and four endpoint fields parsed in both `from_uri` branches
  (standalone and sentinel): `group: Option<String>`,
  `consumer: Option<String>`, `count: Option<u64>`, `block_ms: u64`
  (default 10000; `block=0` → `BLOCK 0`, block indefinitely). Extend
  `Debug` (no secrets involved) and `RedisMetadataDescriptor` with
  `group`, `consumer`, `count`, `block`.
- New `streams.rs` (pub(crate), like `queue.rs`):
  - `StreamConsumerParams { key, group, consumer, count, block_ms }`.
  - `StreamIo` seam: `connect(&mut self, &Client)`, `read_group(...)`
    returning `Option<Vec<StreamEntry>>` (`None` = BLOCK timeout),
    `ack(&mut self, key, group, ids) -> Result<i64, CamelError>`.
  - `RedisStreamIo`: deadline-free multiplexed connection (rc-y0pwj
    precedent); builds
    `XREADGROUP GROUP g c [COUNT n] [BLOCK ms] STREAMS key >` and parses
    via `redis::streams::StreamReadReply`; `XACK key group id...`.
  - `StreamEntry { id, fields: Vec<(String, String)> }` — seam-facing
    parsed shape (keeps redis types behind the real impl).
  - `build_exchange_from_stream_entry`: body = the field value when the
    entry has exactly one field, else a JSON object of the field map;
    headers `CamelRedis.Stream{,Id,Group,Consumer,DeliveryCount}`
    (delivery count always `1` — `>` reads are first deliveries).
  - `stream_session(...)`: the `queue_session` loop shape — resolve →
    connect (`ctx.mark_ready()` here, queue parity) → read/deliver/ack
    on one connection. Per entry: `deliver(entry)` returns
    `DeliveryOutcome::{Completed, Failed}`; `Completed` → `io.ack(...)`
    (ack errors follow the same transient/non-transient split — a
    transient ack error leaves the ack OUTCOME UNKNOWN: Redis may have
    executed the XACK before the connection dropped; the session never
    claims PEL membership either way, recovery is deferred). `Failed`
    → no ack, continue with the next entry. Cancellation between/
    during reads ends the session `Ok` with no ack. `NOGROUP` and other
    non-transient read/ack errors return `Err` → ADR-0007 supervision.
  - Delivery handoff: `run_stream_consumer` delivers via
    `ConsumerContext::send_and_wait` (NOT queue's fire-and-forget
    `send` — an ack requires the pipeline outcome). Mapping:
    `Ok(_)` → `Completed`; `Err(pipeline failure)` → `Failed`, log at
    `warn!` (the route's error handler owns the operational signal,
    ADR-0012 b-bridged semantics) and continue;
    `Err(ChannelClosed)` → no ack, `increment_errors("b-prime:redis:
    xreadgroup-channel-closed")`, return `Err` → supervision (the
    pipeline receiver is gone; continuing would error per entry).
  - Wiring regression: a live-receiver test drives the real consumer
    against the fake RESP server with an mpsc pipeline that replays
    `Err`/`Ok` through `send_and_wait`'s reply path — proving the
    closure mapping end-to-end (no XACK on `Err`, continued delivery,
    no duplicate metric; a regression to fire-and-forget `send` cannot
    observe the replay and fails the test).
- `consumer.rs`: `RedisConsumerMode::Streams { params }`; `new()` maps
  `command=XREADGROUP` to it and fails closed (REDIS-003) when `group`
  or `consumer` is missing/empty or `key` is absent; `start()` spawns
  `run_stream_consumer` (same lifecycle as `run_queue_consumer`).

## Affected crates

- `camel-component-redis`: config fields + variant, new `streams.rs`
  module, consumer wiring, tests (`streams.rs` unit tests,
  `stream_fake_tests.rs` fake RESP server, `consumer_tests.rs`
  constructor cases).

## Architecture boundaries

Component-internal only: data-plane reads stay inside the component;
exchanges cross to the pipeline solely via `ConsumerContext::send`
(data/control plane split). Readiness via `mark_ready`, supervision via
returned `Err` (ADR-0007); error taxonomy via b-prime metric labels
(ADR-0012). Timeout posture per ADR/precedent rc-y0pwj (deadline-free
connection; `connection_timeout_secs` bounds connect only). No DSL,
Runtime, or camel-api changes.

## Design decisions (the ack/failure table)

| Event | XACK? | Next action |
|---|---|---|
| `send_and_wait` Ok | yes (per entry) | next entry / block again |
| `send_and_wait` Err (pipeline failure) | no | `warn!` (route error handler owns the signal), continue next entry |
| `send_and_wait` Err (`ChannelClosed`) | no | b-prime metric, session `Err` → supervision |
| cancel while blocked | no | session returns `Ok` |
| cancel between entries | no for undelivered | delivered+acked stay acked |
| read transient error | no | reconnect under retry budget |
| read `NOGROUP`/other server error | no | session `Err` → supervision |
| ack Ok | — | entry removed from PEL |
| ack transient error | — | outcome UNKNOWN (XACK may have executed); reconnect; no PEL claim; recovery deferred |
| ack non-transient | — | session `Err` → supervision |

Readiness: after the first successful connect (queue parity) — group
membership is confirmed by the first read; `NOGROUP` surfaces as a
supervision failure. Group creation is the operator's job (no
auto-create; `NOGROUP` fails closed — least-magic, REDIS-003 spirit).
Delivery count: constant `1` in this slice (spec'd invariant).

## Phases

Omitted — single coherent slice.

## Alternatives considered

- Separate connection for XACK: rejected — the session connection is
  already mutable-owned by the loop; a second connection doubles the
  failover surface for no semantic gain.
- Queue-style fire-and-forget `send`: rejected for Streams — an ack
  decision needs the pipeline outcome; `send` returning `Ok` only
  proves enqueue. (`send_and_wait` is the completion-bearing API,
  ADR-0012 request-reply path.)
- Auto-create group with `$`/`0` on NOGROUP: rejected — silently
  choosing a start position is a data-loss decision the operator must
  own (REDIS-003: no silent fallbacks).
- Batch-ack (one XACK for COUNT entries): rejected — per-entry ack
  gives the exact ack/failure table above with no partial-batch
  reasoning; COUNT stays a read-side bound only.
- Reusing `QueueIo` with a mode flag: rejected — different reply shape
  and an extra command (XACK); a dedicated seam keeps both loops honest.
