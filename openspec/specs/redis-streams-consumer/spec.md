# redis-streams-consumer Specification

## Purpose
TBD - created by archiving change redis-streams-consumer-groups. Update Purpose after archive.
## Requirements
### Requirement: XREADGROUP activates Streams consumer mode without fallback

The Redis consumer SHALL accept `command=XREADGROUP` on `redis://` and
`redis-sentinel://` consumer endpoints and resolve it to a Streams
consumer mode. The mode SHALL require a non-empty `group`, a non-empty
`consumer`, and a `key`; any of them missing or empty SHALL fail at
consumer construction with an invalid-URI error (fail closed, REDIS-003).
`XREADGROUP` mode SHALL NOT fall back to BLPOP/BRPOP or any other
consume path. Optional parameters: `count` (positive integer, absent =
no COUNT argument) and `block` (milliseconds, default 10000, `0` = block
indefinitely); invalid values SHALL fail at construction.

#### Scenario: valid XREADGROUP URI resolves to Streams mode

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XREADGROUP&key=s1&group=g&consumer=c`
- **WHEN** `RedisConsumer::new` builds the consumer
- **THEN** the consumer is in Streams mode with group `g`, consumer `c`,
  no count, and block 10000 ms

#### Scenario: missing group fails closed

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XREADGROUP&key=s1&consumer=c`
- **WHEN** `RedisConsumer::new` builds the consumer
- **THEN** construction fails with an invalid-URI error naming `group`,
  and no connection is attempted

#### Scenario: empty consumer name fails closed

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XREADGROUP&key=s1&group=g&consumer=`
- **WHEN** `RedisConsumer::new` builds the consumer
- **THEN** construction fails with an invalid-URI error naming `consumer`

#### Scenario: invalid block value fails closed

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XREADGROUP&key=s1&group=g&consumer=c&block=abc`
- **WHEN** `RedisConsumer::new` builds the consumer
- **THEN** construction fails with an invalid-URI error naming `block`

#### Scenario: no fallback to BLPOP

- **GIVEN** a Streams consumer whose session hits a transient transport
  error on every connect attempt within budget
- **WHEN** the retry budget is exhausted
- **THEN** the session returns `Err` for supervision and no BLPOP/BRPOP
  command was ever issued (verified by the recorded command log)

### Requirement: Group reads deliver one Exchange per stream entry with delivery metadata

The Streams consumer SHALL read new entries with
`XREADGROUP GROUP <group> <consumer> [COUNT <count>] [BLOCK <block>]
STREAMS <key> >` on a deadline-free connection and deliver exactly one
Exchange per returned stream entry through
`ConsumerContext::send_and_wait` (the completion-bearing handoff; the
fire-and-forget `send` is insufficient because the ack decision
requires the pipeline outcome). Each Exchange SHALL carry headers:
`CamelRedis.Stream` (stream key), `CamelRedis.StreamId` (entry id),
`CamelRedis.StreamGroup`, `CamelRedis.StreamConsumer`, and
`CamelRedis.StreamDeliveryCount` (always `1` in this slice — `>` reads
are first deliveries). The body SHALL be the raw field value for a
single-field entry, or a JSON object of the field map for multi-field
entries. A BLOCK timeout (nil reply) SHALL deliver nothing and keep the
connection.

#### Scenario: one entry delivered with full metadata

- **GIVEN** a fake RESP server (or seam) that answers XREADGROUP with
  one entry `1-1` carrying fields `payload=hello`
- **WHEN** the session reads and delivers the entry
- **THEN** exactly one Exchange is delivered with body `hello` and
  headers `CamelRedis.Stream=s1`, `CamelRedis.StreamId=1-1`,
  `CamelRedis.StreamGroup=g`, `CamelRedis.StreamConsumer=c`,
  `CamelRedis.StreamDeliveryCount=1`

#### Scenario: multi-field entry body is a JSON object

- **GIVEN** an entry `2-2` with fields `a=1`, `b=2`
- **WHEN** the entry is converted to an Exchange
- **THEN** the body parses as a JSON object `{"a":"1","b":"2"}`

#### Scenario: count and block shape the read command

- **GIVEN** a Streams consumer configured with `count=10` and `block=5000`
- **WHEN** the real I/O layer builds the XREADGROUP command
- **THEN** the command arguments are `XREADGROUP GROUP g c COUNT 10
  BLOCK 5000 STREAMS s1 >` (arg order verified on the recorded wire
  frames)

### Requirement: XACK only after successful pipeline completion

The session SHALL issue `XACK <key> <group> <id>` for an entry only
after that entry's `ConsumerContext::send_and_wait` completes
successfully. A failed send_and_wait SHALL NOT ack; a pipeline
processing failure SHALL log at `warn!` (the route's error handler owns
the operational signal) and the consumer SHALL continue with subsequent
entries; a `ChannelClosed` failure SHALL NOT ack, SHALL record a
b-prime error metric (`b-prime:redis:xreadgroup-channel-closed`), and
SHALL end the session with `Err` for supervision. Cancellation (route
stop) SHALL NOT ack undelivered or failed entries; entries already
acked stay acked. Ack transport errors follow the same
transient/non-transient classification as reads: transient ack failure
→ reconnect WITHOUT claiming PEL membership either way (the XACK may
have executed before the connection dropped — the ack outcome is
unknown; redelivery recovery is deferred work); non-transient → session
`Err` (supervision).

#### Scenario: successful delivery acks exactly once

- **GIVEN** a seam (or fake server) with one entry whose
  send_and_wait completes successfully
- **WHEN** the session processes the entry
- **THEN** exactly one XACK for that entry id is issued

#### Scenario: pipeline failure does not ack and delivery continues (live wiring)

- **GIVEN** a `RedisConsumer` in Streams mode against the fake RESP
  server (one XREADGROUP reply carrying entries `1-1` and `2-2`) and a
  `ConsumerContext` whose mpsc receiver is a live test pipeline that
  replies `Err` for the exchange carrying id `1-1` and `Ok` for `2-2`
- **WHEN** the consumer runs until both entries are processed
- **THEN** XACK is observed only for `2-2`, the failure was logged at
  `warn!` or below with no error metric emitted for the route-owned
  pipeline failure, and entry `2-2` was still delivered (processing
  continued). A regression to fire-and-forget `send` (no reply path)
  fails this test: the `Err` replay is only observed through
  `send_and_wait`.

#### Scenario: channel-closed delivery does not ack and fails supervision

- **GIVEN** a ConsumerContext whose pipeline receiver is dropped
- **WHEN** the consumer delivers an entry via send_and_wait
- **THEN** no XACK is issued, the metric label
  `b-prime:redis:xreadgroup-channel-closed` was recorded, and the
  session returns `Err`

#### Scenario: cancellation while blocked issues no ack

- **GIVEN** a session blocked in XREADGROUP (withheld reply / pending
  seam) with no entries read
- **WHEN** the cancellation token fires
- **THEN** the session ends `Ok` and no XACK was issued

### Requirement: Readiness, supervision and failover follow queue-mode patterns

The Streams session SHALL resolve the topology master, connect with no
driver-level response deadline (bd rc-y0pwj posture), and signal
readiness (`mark_ready`) after the first successful connect. A `NOGROUP`
or other non-transient server error SHALL NOT be retried as transient:
the session returns `Err` for ADR-0007 route supervision. Transient
transport errors during resolve, connect, read or ack SHALL re-resolve
and reconnect under the configured retry budget. Reconnects read with
`>` only — already-delivered entries are never re-read in this slice
(redelivery of unacked entries is deferred recovery work).

#### Scenario: readiness after connect with zero entries

- **GIVEN** a seam whose first read pends (no entries) and connect that
  succeeds
- **WHEN** the session starts and reaches the blocked read
- **THEN** `mark_ready` has fired exactly once before any entry exists

#### Scenario: NOGROUP fails supervision without transient retries

- **GIVEN** a seam that answers the first read with a non-transient
  NOGROUP-classified error
- **WHEN** the session processes the read
- **THEN** the session returns `Err` without reconnect attempts

#### Scenario: transient read error reconnects within budget

- **GIVEN** a seam programmed with one transient read error followed by
  a successful connect and read on the next connection
- **WHEN** the session runs
- **THEN** a second connect occurs, the entry is delivered and acked,
  and the session continues

### Requirement: Deterministic verification without sleeps

All Streams behavior SHALL be verifiable through the `StreamIo` seam
(programmed outcomes, recorded commands) and a scripted-reply fake RESP
server (the `blocking_pop_tests.rs` pattern) with no sleeps: bounded
watch-channel waits, cancellation tokens, and `tokio::test` current-time
only. `lint-test-sleep` and `lint-unbounded-wait` pass on the new tests.

#### Scenario: fake server round trip delivers and acks on the wire

- **GIVEN** a scripted fake RESP server answering one XREADGROUP with a
  single-entry reply and accepting XACK
- **WHEN** the real `RedisStreamIo` runs one session over it until
  cancellation
- **THEN** the server observed one XREADGROUP with the configured group,
  consumer, key and `>` argument, one delivered Exchange with the
  expected id, and one XACK naming that id

