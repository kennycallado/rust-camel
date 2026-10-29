# redis-streams-pending-recovery — spec delta

## MODIFIED Requirements

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
`CamelRedis.StreamDeliveryCount`. The delivery count is the number of
deliveries this consumer group has made for the entry including the
one being delivered: first `>` deliveries carry `1`; entries reclaimed
by the recovery loop carry the authoritative post-claim count read
from Redis immediately before delivery (XAUTOCLAIM without JUSTID
increments the delivery counter). The body SHALL be the raw field
value for a single-field entry, or a JSON object of the field map for
multi-field entries. A BLOCK timeout (nil reply) SHALL deliver nothing
and keep the connection.

#### Scenario: one entry delivered with full metadata

- **GIVEN** a fake RESP server (or seam) that answers XREADGROUP with
  one entry `1-1` carrying fields `payload=hello`
- **WHEN** the session reads and delivers the entry
- **THEN** exactly one Exchange is delivered with body `hello` and
  headers `CamelRedis.Stream=s1`, `CamelRedis.StreamId=1-1`,
  `CamelRedis.StreamGroup=g`, `CamelRedis.StreamConsumer=c`,
  `CamelRedis.StreamDeliveryCount=1`

#### Scenario: multi-field entry body is a JSON object

- **GIVEN** a seam entry `2-2` with fields `a=1` and `b=2`
- **WHEN** the entry is delivered
- **THEN** the Exchange body is the JSON object `{"a":"1","b":"2"}`

#### Scenario: count and block shape the read command

- **GIVEN** a Streams consumer configured with `count=10` and
  `block=5000`
- **WHEN** the real I/O layer builds the XREADGROUP command
- **THEN** the command arguments are
  `XREADGROUP GROUP g c COUNT 10 BLOCK 5000 STREAMS s1 >` (arg order
  verified on the recorded wire frames)

#### Scenario: recovered entry carries the post-claim delivery count

- **GIVEN** recovery is enabled, the fake RESP server answers XAUTOCLAIM
  with entry `1-1`, and the targeted post-claim
  `XPENDING s1 g IDLE 0 1-1 1-1 1` answers delivery count `3`
- **WHEN** the recovery loop delivers the claimed entry
- **THEN** the Exchange carries header
  `CamelRedis.StreamDeliveryCount=3` — the targeted post-claim reply
  used verbatim — not the pinned `1`

### Requirement: XACK only after successful pipeline completion

The session SHALL issue `XACK <key> <group> <id>` for an entry only
after that entry's `ConsumerContext::send_and_wait` completes
successfully. This applies identically to `>` reads (acked on the read
connection) and to entries delivered by the recovery loop (acked on
the recovery connection, so a recovered-entry XACK never queues behind
a blocked read). A failed send_and_wait SHALL NOT ack; a pipeline
processing failure SHALL log at `warn!` (the route's error handler owns
the operational signal) and the consumer SHALL continue with subsequent
entries; a `ChannelClosed` failure SHALL NOT ack, SHALL record a b-prime
error metric (`b-prime:redis:xreadgroup-channel-closed`), and SHALL end
the session with `Err` for supervision. Cancellation (route stop) SHALL
NOT ack undelivered or failed entries; entries already acked stay
acked. An XACK whose transport outcome is unknown (connection dropped
mid-ack) leaves the entry in the PEL from the consumer's point of view;
when recovery is enabled the recovery loop — not the read path — is
what may redeliver it, after the configured minimum idle time. Ack
transport errors follow the same transient/non-transient
classification as reads: transient ack failure → reconnect WITHOUT
claiming PEL membership either way; non-transient → session `Err`
(supervision).

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

#### Scenario: recovered entry acks only after its delivery completes

- **GIVEN** a claimed entry `3-3` delivered by the recovery loop whose
  send_and_wait completes `Ok`
- **WHEN** the recovery pass processes the claim
- **THEN** exactly one XACK names `3-3`, issued after the completion,
  observed on the recovery connection

### Requirement: Readiness, supervision and failover follow queue-mode patterns

The Streams session SHALL resolve the topology master, connect with no
driver-level response deadline (bd rc-y0pwj posture), and signal
readiness (`mark_ready`) after the first successful connect of the
read path. A `NOGROUP` or other non-transient server error SHALL NOT be
retried as transient: the session returns `Err` for ADR-0007 route
supervision — including a `NOGROUP` raised by a recovery command.
Transient transport errors during resolve, connect, read, ack or
recovery commands SHALL re-resolve and reconnect under the configured
retry budget. With recovery enabled the consumer holds a SECOND
deadline-free connection (read connection + recovery connection); the
two reconnect INDEPENDENTLY under the same budget, and readiness is
unaffected by recovery-connection health. The stored claim cursor is
session state and PERSISTS across reconnects. A pass aborted mid-way
by an error leaves its already-claimed entries pending under this
consumer for later passes to reclaim. Reconnects read with `>` only —
already-delivered entries are never re-read by XREADGROUP; unacked
pending entries MAY be redelivered through the recovery loop when
`streamRecoverEnabled=true` (slice 2), and never are when it is
`false`.

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

#### Scenario: NOGROUP on a recovery command fails supervision

- **GIVEN** recovery enabled and a seam whose XAUTOCLAIM answers with a
  NOGROUP-classified error
- **WHEN** the recovery pass runs
- **THEN** the session returns `Err` without transient retries

#### Scenario: transient read error reconnects within budget

- **GIVEN** a seam programmed with one transient read error followed by
  a successful connect and read on the next connection
- **WHEN** the session runs
- **THEN** a second connect occurs, the entry is delivered and acked,
  and the session continues

#### Scenario: cursor survives a recovery-connection reconnect

- **GIVEN** a first pass whose last XAUTOCLAIM returned next cursor
  `5-0`, followed by a transient error on the recovery connection
- **WHEN** the recovery connection reconnects and the next gated pass
  runs
- **THEN** its first XAUTOCLAIM names cursor `5-0`, not `0-0`

## ADDED Requirements

### Requirement: XPENDING inspection exposes a bounded pending summary per recovery pass

When recovery is enabled, each recovery pass SHALL first issue the
summary form `XPENDING <key> <group>` on the dedicated recovery
connection and log one line with the pending count through the
component's existing log style. The summary form has no count
parameter in the protocol; its reply size is bounded by the group's
consumer count (server-owned). An empty or nil summary SHALL end the
pass. Delivery counts for claimed entries SHALL be read with the
extended form used ONLY in its targeted single-entry shape
`XPENDING <key> <group> IDLE 0 <id> <id> 1`, issued on the recovery
connection AFTER the claim; the reply is already post-claim (XAUTOCLAIM
without JUSTID increments the counter) and SHALL be used VERBATIM as
the delivery count — no pre-claim snapshot, no arithmetic, no
fabricated fallback. An empty targeted reply for an entry this
consumer just claimed is anomalous (the entry is pending under this
consumer at that point): the entry SHALL be SKIPPED — no delivery, no
XACK, a `warn!` log — and the pass SHALL continue. Every
component-issued request EXCEPT the aggregate summary SHALL carry an
explicit bound (the summary form has none in the protocol); bounding
requests is the component's entire contract and Redis-side scan work
over its own PEL is server-owned and out of scope.

#### Scenario: pass logs the pending summary

- **GIVEN** a fake RESP server answering `XPENDING s1 g` with a summary
  of two pending entries
- **WHEN** one recovery pass runs
- **THEN** the pending count `2` appears in the log line

#### Scenario: claimed entry resolves its count through a targeted read

- **GIVEN** a pass whose XAUTOCLAIM returns entry `9-9`, and a fake
  server whose targeted `XPENDING s1 g IDLE 0 9-9 9-9 1`, issued after
  the claim, answers delivery count `4`
- **WHEN** the pass delivers `9-9`
- **THEN** the Exchange header `CamelRedis.StreamDeliveryCount=4` (the
  targeted post-claim reply verbatim) and exactly one targeted XPENDING
  was issued for `9-9`

#### Scenario: empty targeted reply skips the entry without fabricating a count

- **GIVEN** a pass whose XAUTOCLAIM returns entry `8-8` and a fake
  server whose targeted `XPENDING s1 g IDLE 0 8-8 8-8 1` answers an
  empty array
- **WHEN** the pass processes `8-8`
- **THEN** `8-8` is NOT delivered, no XACK is issued for it, a `warn!`
  is logged, and the pass continues with any remaining claimed entries

### Requirement: XAUTOCLAIM recovery reclaims stale pending entries through the normal delivery pipeline

When `streamRecoverEnabled=true`, recovery commands and recovered-entry
XACKs run on a dedicated recovery connection (same client, same
deadline-free posture, same reconnect budget) so they never interleave
with a blocked XREADGROUP; the read connection is owned end-to-end by
a dedicated read task with a bounded, completion-bearing handoff: the
task issues ONE read, reports the outcome to the session over a
channel, and starts its next read only after the session signals the
batch done (all of the batch's deliveries and required read-side XACKs
finished, or the connection was invalidated) — a read-side XACK never
queues behind the task's own next blocked read. The read task is
explicitly torn down: on cancellation, terminal error, or session
drop, its JoinHandle is aborted (never awaited against a blocked
read), closing the socket and ending the blocked read promptly. The
session selects the read-outcome channel, the cancellation token, and
a recovery deadline `streamRecoverMinIdleMs` after the last pass
started, so recovery progress is independent of both idle reads and
read-loop boundaries — continuous traffic and quiet `block=0` streams
recover alike. The deadline stays selectable even while a read batch
is mid-flight: when it fires during a batch's delivery or read-side
XACK, the pass runs on the recovery connection (independent I/O) and
the batch resumes afterwards from its RETAINED position —
claimed-but-unprocessed entries are never re-read or re-delivered.
Recovery deliveries are sequential — one outstanding `send_and_wait`
at a time; an in-flight delivery's completion and its ack obligation
are PRESERVED across a deadline-triggered pass (never dropped or
restarted), and its ack decision follows the XACK requirement when the
completion arrives, after which delivery resumes sequentially from the
retained position. A
batch-done signal arriving after cancellation won is ignored and no
further read starts. An unexpected reader panic or control-channel
closure SHALL propagate to the session as a terminal error (supervision
`Err`); only the orderly cancellation path ends the session `Ok`. The
pass claims entries idle for at least
`streamRecoverMinIdleMs` with
`XAUTOCLAIM <key> <group> <consumer> <min-idle> <cursor> COUNT <remaining>`,
starting at the session's STORED CURSOR (initially `0-0`), never at
`0-0` every pass. `<remaining>` is the unused entry budget
(`streamRecoverBatch` minus entries already claimed this pass), so a
pass claims at most `streamRecoverBatch` entries IN TOTAL; at most
`streamRecoverBatch` XAUTOCLAIM commands per pass additionally cap
pathological empty-nonzero-cursor chains. A NONZERO next cursor with an
empty entries array (entries deleted while pending) SHALL continue the
pass from that cursor, and the stored cursor SHALL advance with it so
the next pass resumes after already-scanned entries — a poison prefix
of repeatedly failing entries is retried at most once per full cursor
cycle; a `0-0` next cursor SHALL wrap the stored cursor back to `0-0`
(cycle complete). Claimed entries go through the same `send_and_wait`
pipeline and the same XACK requirement as `>` reads, honouring the
existing deadline-free connection rules, including the `ChannelClosed`
supervision exception. The cancellation token SHALL be checked between
commands, between claimed entries, and at the pre-XACK decision point:
no delivery starts after cancellation is observed, and an XACK is
issued only if its delivery completed `Ok` before the token fired.

#### Scenario: claimed entries are delivered and acked after success

- **GIVEN** a fake RESP server whose XAUTOCLAIM returns entry `1-1` and
  whose targeted `XPENDING s1 g IDLE 0 1-1 1-1 1` answers delivery
  count `3`, and the pipeline completes successfully
- **WHEN** the recovery pass processes the claim
- **THEN** the entry is delivered through `send_and_wait` with header
  `CamelRedis.StreamDeliveryCount=3` and exactly one XACK naming `1-1`
  follows the successful completion

#### Scenario: failed route-owned delivery keeps the entry pending

- **GIVEN** a claimed entry `2-2` whose `send_and_wait` completes with
  a non-`ChannelClosed` `Err`
- **WHEN** the recovery pass processes the claim
- **THEN** no XACK is issued for `2-2`, the failure is logged at
  `warn!` or below with no error metric for the route-owned pipeline
  failure, the entry remains pending for the next pass, and processing
  continues with any remaining claimed entries

#### Scenario: channel-closed during recovery fails supervision

- **GIVEN** a claimed entry whose `send_and_wait` fails with
  `ChannelClosed`
- **WHEN** the recovery pass processes the claim
- **THEN** no XACK is issued, the
  `b-prime:redis:xreadgroup-channel-closed` metric is recorded, and the
  session returns `Err`

#### Scenario: shutdown during recovery stops promptly with no ack after cancel

- **GIVEN** a recovery pass in flight with claimed entries pending
  delivery
- **WHEN** the cancellation token fires between two claimed entries
- **THEN** the session ends `Ok`, no further delivery starts, and no
  XACK is issued after the cancel

#### Scenario: cancel during a withheld recovery reply issues no ack

- **GIVEN** a seam whose XAUTOCLAIM reply is withheld (pending) and no
  entries delivered yet
- **WHEN** the cancellation token fires while the reply is withheld
- **THEN** the session ends `Ok` with no XACK issued

#### Scenario: completion after cancel does not ack

- **GIVEN** a claimed entry whose `send_and_wait` completion is
  resolved after the cancellation token fires (seam-ordered race)
- **WHEN** the session observes the cancel before the `Ok` completion
  at the ack decision point
- **THEN** no XACK is issued for that entry and the session ends `Ok`

#### Scenario: empty claim cursor ends the cycle without looping

- **GIVEN** a fake RESP server whose XAUTOCLAIM replies with next
  cursor `0-0` and an empty entries array
- **WHEN** the recovery pass runs
- **THEN** the pass ends after that reply with no additional XAUTOCLAIM
  command issued until the next gated pass, and the stored cursor is
  wrapped back to `0-0`

#### Scenario: nonzero cursor with empty entries continues without discarding progress

- **GIVEN** a fake RESP server whose first XAUTOCLAIM (cursor `0-0`)
  replies with next cursor `5-0` and an empty entries array, and whose
  second XAUTOCLAIM (from cursor `5-0`) replies with cursor `0-0` and
  entry `7-7`
- **WHEN** the recovery pass runs
- **THEN** the second XAUTOCLAIM names cursor `5-0`, entry `7-7` is
  delivered, and the pass ends

#### Scenario: the next pass resumes from the stored cursor

- **GIVEN** a first pass whose last XAUTOCLAIM returned next cursor
  `5-0` (nonzero, entries claimed below the batch budget)
- **WHEN** the next gated pass runs
- **THEN** its first XAUTOCLAIM names cursor `5-0`, not `0-0`, so a
  poison prefix of repeatedly failing entries is retried at most once
  per full cursor cycle instead of starving entries behind it

#### Scenario: each claim request uses the remaining entry budget

- **GIVEN** `streamRecoverBatch=5` and a fake RESP server whose first
  XAUTOCLAIM (cursor `0-0`, `COUNT 5`) returns 4 entries with next
  cursor `3-0`
- **WHEN** the pass continues from cursor `3-0`
- **THEN** the second XAUTOCLAIM carries `COUNT 1` (the remaining
  budget), and no request of the pass can push the total claims past 5

#### Scenario: quiet block=0 stream still recovers by deadline

- **GIVEN** `block=0` (read blocks indefinitely) with no traffic, and
  recovery enabled with `streamRecoverMinIdleMs=1000`
- **WHEN** the session runs with the tokio clock paused and time is
  advanced past the recovery deadline
- **THEN** a recovery pass runs on the recovery connection while the
  read task remains blocked on the read connection, without sleeps

#### Scenario: read-side ack precedes the next blocked read

- **GIVEN** `block=0` and a first read whose entries were delivered
  successfully
- **WHEN** the read task issues its next XREADGROUP
- **THEN** the XACKs for the first batch were already observed on the wire
  before the second XREADGROUP frame (handoff barrier held)

#### Scenario: cancellation terminates the read task promptly

- **GIVEN** a read task blocked in `XREADGROUP` with `block=0`
- **WHEN** the cancellation token fires
- **THEN** the session ends `Ok`, the read task is aborted (not awaited
  against the blocked read), and the read connection's socket is closed
  (the fake server observes the connection end)

#### Scenario: deadline fires while a read-side XACK is withheld

- **GIVEN** a read batch whose read-side XACK is withheld (pending
  seam) and a recovery deadline that passes while the XACK is
  outstanding
- **WHEN** time advances past the recovery deadline
- **THEN** the recovery pass runs on the recovery connection (XAUTOCLAIM
  observed) while the XACK remains outstanding, the batch's progress is
  retained, and after the XACK completes no claimed entry is re-read or
  re-delivered

#### Scenario: unexpected reader failure propagates as a terminal error

- **GIVEN** a read task that panics (or whose control channel closes)
  without a cancellation request
- **WHEN** the session observes the failure
- **THEN** the session returns `Err` for supervision — distinct from the
  orderly cancellation path, which ends the session `Ok`

#### Scenario: withheld pipeline completion survives a deadline pass

- **GIVEN** a recovery pass whose first claimed entry's `send_and_wait`
  completion is withheld (pending seam), and the recovery deadline
  passes while it is outstanding
- **WHEN** time advances past the deadline
- **THEN** a pass runs on the recovery connection while the completion
  remains outstanding, the in-flight delivery is neither dropped nor
  restarted, and when the completion arrives `Ok` its XACK is issued
  and the next claimed entry is delivered from the retained position

#### Scenario: recovery command shape on the wire

- **GIVEN** a Streams consumer with `streamRecoverMinIdleMs=30000` and
  `streamRecoverBatch=5`
- **WHEN** the first recovery pass runs against the fake RESP server
- **THEN** the server observes
  `XAUTOCLAIM s1 g c 30000 0-0 COUNT 5` with exactly those arguments

### Requirement: Recovery options validate ranges and default safe

The consumer accepts `streamRecoverEnabled` (boolean, default `false`),
`streamRecoverMinIdleMs` (default `60000`, valid range `1..=3600000`),
and `streamRecoverBatch` (default `10`, valid range `1..=100`). URI
parsing and consumer construction SHALL both fail closed on
out-of-range values. Recovery is off by default: enabling it is an
explicit operator decision. Options set on a non-XREADGROUP command
SHALL be ignored, and Streams mode SHALL validate them fail-closed
(the `count=0` pattern). The component metadata URI-option list and
the component documentation SHALL include the new options and the
recovery semantics, including the meaning of
`CamelRedis.StreamDeliveryCount`.

#### Scenario: zero batch fails closed

- **GIVEN** a URI with `command=XREADGROUP&...&streamRecoverBatch=0`
- **WHEN** the endpoint config is parsed
- **THEN** construction fails with a validation error naming the
  option, and no connection is attempted

#### Scenario: min-idle above the cap fails closed

- **GIVEN** a programmatic config with
  `stream_recover_min_idle_ms = 3_600_001`
- **WHEN** `RedisConsumer::new` builds a Streams consumer
- **THEN** construction fails with a validation error naming the option

#### Scenario: recovery off by default

- **GIVEN** a Streams consumer without any `streamRecover*` options
- **WHEN** the session runs
- **THEN** no XPENDING or XAUTOCLAIM command is ever issued
