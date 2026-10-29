# Design: redis-streams-pending-recovery

## Approach

Slice 2 extends the Streams consumer in `camel-component-redis` along the seams
slice 1 already established (`StreamIo` trait, `stream_session` loop, scripted
RESP fake server).

**Connection model.** Two deadline-free connections, same client, same
transient/non-transient classification and reconnect budget:

- Read connection: owned end-to-end by a dedicated read task so a blocked
  `XREADGROUP` future is never dropped mid-block while the session
  continues. Handoff is bounded and completion-bearing: the task issues ONE
  read, reports the outcome (entries, nil tick, error) to the session over a
  channel, and WAITS for a batch-done signal before its next read — the
  session sends that signal only after the batch's deliveries and required
  read-side XACKs have finished (or the connection was invalidated), so a
  read-side XACK never queues behind the task's own next blocked read. The
  session selects over the read channel, the recovery deadline, and the
  cancellation token. Read-path XACKs use the read connection (unchanged).
  Teardown is explicit: on cancellation, terminal error, or session-future
  drop, the read task's JoinHandle is aborted (never awaited against a
  blocked read), which closes the socket and ends the blocked read promptly.
- Recovery connection: owned by the session. Recovery passes, targeted
  count reads, and recovered-entry XACKs all run on it — no recovered-entry
  XACK ever queues behind a blocked read.

Connections reconnect independently under the existing budget. Readiness
stays the canonical contract: first successful read-connection connect. A
reconnect invalidates only connection-local state; the stored claim cursor is
session state and persists across reconnects. A pass aborted mid-way (error
or cancel) leaves its already-claimed entries pending under this consumer;
later passes reclaim them.

**Recovery pass.** Runs on the recovery connection; every request carries an
explicit bound (bounding requests is the component's whole contract — Redis
PEL scan work is server-owned). The XPENDING summary is the protocol's
aggregate form and has no count parameter; every other request carries a
positional count or `COUNT`:

1. `XPENDING <key> <group>` — summary form (aggregate form; no count
   parameter exists; its reply size is bounded by the group's consumer
   count, server-owned). The pending count feeds one observability log line
   per pass. Empty/nil ends the pass.
2. `XAUTOCLAIM <key> <group> <consumer> <min-idle> <cursor> COUNT <remaining>`
   loop — `<remaining>` is the UNUSED entry budget (`streamRecoverBatch`
   minus entries already claimed this pass), so a pass claims at most
   `streamRecoverBatch` entries in total; at most `streamRecoverBatch`
   XAUTOCLAIM commands per pass additionally cap pathological
   empty-nonzero-cursor chains. The pass starts at the session's STORED
   CURSOR (initially `0-0`), never at `0-0` every pass. A NONZERO next
   cursor advances the stored cursor (next pass resumes after scanned
   entries — a poison prefix is retried at most once per cycle); a `0-0`
   next cursor wraps the stored cursor (cycle complete). RESP2
   two-element (6.2) and three-element (7.0+) replies both parse.
3. Per claimed entry, BEFORE delivering it: one targeted bounded
   `XPENDING <key> <group> IDLE 0 <id> <id> 1` — the authoritative
   POST-claim delivery count (XAUTOCLAIM without JUSTID incremented it),
   used VERBATIM as `CamelRedis.StreamDeliveryCount`. No pre-claim map
   exists, so no race and no +1 arithmetic. An empty targeted reply is
   anomalous (post-claim the entry is pending under this consumer): the
   entry is SKIPPED — no delivery, no XACK, `warn!` log — and the pass
   continues; a delivery count is never fabricated. First `>` deliveries
   stay `1`.

**Delivery and ack.** Claimed entries go through the SAME `send_and_wait`
pipeline and the SAME XACK requirement as `>` reads, including its
exceptions: a route-owned pipeline failure logs `warn!` or below with no
error metric and processing continues (the entry stays pending); a
`ChannelClosed` failure records `b-prime:redis:xreadgroup-channel-closed`
and ends the session `Err` for supervision. No new recovery delivery starts
after cancellation is observed; an XACK is issued only if its delivery
completed `Ok` before the token fired at the ack decision point — otherwise
the entry stays pending (a later redelivery may duplicate, the accepted PEL
trade-off).

**Cadence.** One pass right after the first successful connect. Afterwards
the session selects the read-outcome channel, the cancellation token, and a
recovery deadline `streamRecoverMinIdleMs` after the last pass started; the
deadline branch runs a pass on the recovery connection while the read task
keeps blocking independently — continuous traffic and quiet `block=0`
streams recover alike. The deadline stays selectable even while a read
batch is mid-flight: when it fires during delivery/ack of a batch, the pass
runs on the recovery connection (independent I/O) and the batch resumes
afterwards from its retained position — claimed-but-unprocessed entries are
never re-read or re-delivered. Recovery deliveries are sequential (one
outstanding `send_and_wait` at a time); an in-flight delivery's completion
and its ack obligation are preserved across a deadline-triggered pass —
never dropped or restarted — and delivery resumes sequentially from the
retained position once the completion settles. A batch-done signal that
arrives after cancellation won is ignored; no further read starts. The
read task is also
spawned with panic capture: an unexpected reader panic or control-channel
closure propagates to the session as a terminal error (supervision `Err`);
only the orderly cancellation path ends the session `Ok`.

**Config.** `RedisEndpointConfig` gains `stream_recover_enabled: bool`
(default `false`), `stream_recover_min_idle_ms: u64` (default `60000`,
valid `1..=3_600_000`), `stream_recover_batch: u64` (default `10`, valid
`1..=100`); `StreamConsumerParams` carries the shorter mirrors
`recover_enabled`, `recover_min_idle_ms`, `recover_batch`. URI parsing and `RedisConsumer::new` both fail closed on
out-of-range values (the `count=0` pattern). Defaults are safe: recovery
is opt-in; when enabled it reclaims only entries idle ≥ one minute, ≤ 100
entries per pass; the min-idle value doubles as the pass cadence.
Non-Streams commands ignore the options; Streams mode validates them.
`metadata.rs` URI-option parity list updated.

**IO seam.** `StreamIo` splits into `StreamReadIo` (connect, read_group,
ack — owned by the read task) and `StreamRecoveryIo` (independent connect,
`recover(...) -> RecoveryBatch { pending_count, recovered, next_cursor }`,
`recover_ack` — owned by the session), with independent reconnect state;
readiness gates only on the read half. The claim-loop driver is a pure
function over scripted `redis::Value` replies for unit testing.
`RedisStreamIo` becomes a factory building both halves from one client.
`StreamConsumerParams` gains `recover_enabled`, `recover_min_idle_ms`,
`recover_batch`; `RedisEndpointConfig` gains `stream_recover_enabled`,
`stream_recover_min_idle_ms`, `stream_recover_batch`. `StreamEntry` gains
`delivery_count: u64`. Recovered batches produced while another delivery's
completion is still outstanding queue FIFO and are delivered only when no
delivery is outstanding (sequential-delivery rule).

## Affected crates

- `camel-component-redis`: `config.rs`, `consumer.rs`, `streams.rs`,
  `metadata.rs`, `stream_fake_tests.rs`, docs page.

## Architecture boundaries

Components only. No camel-core, DSL, or services changes. Recovery lives in
the component session loop under the existing supervision model (ADR-0007) and
ADR-0012 error taxonomy (route-owned failures `warn!`-or-below; transport
errors use the existing reconnect budget; ChannelClosed supervision
preserved). Verification stays deterministic and sleep-free: `StreamIo` seam
with programmed outcomes plus the scripted fake RESP server, bounded waits,
`tokio::time::pause`/`advance` for the cadence deadline
(`lint-test-sleep`, `lint-unbounded-wait` clean).
