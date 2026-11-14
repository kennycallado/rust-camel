## MODIFIED Requirements

### Requirement: Readiness, supervision and failover follow queue-mode patterns

The Streams session SHALL resolve the topology master, connect with no
driver-level response deadline (bd rc-y0pwj posture), and signal
readiness (`mark_ready`) after the first successful connect of the
read path. A `NOGROUP` or other non-transient server error SHALL NOT be
retried as transient: the session returns `Err` for ADR-0007 route
supervision — including a `NOGROUP` raised by a recovery command.
Transient transport errors during resolve, connect, read, ack or
recovery commands SHALL re-resolve and reconnect under the configured
retry budget. The retry budget SHALL count consecutive failures since
the last proven round trip, never lifetime failures. The read task
SHALL reset its attempt counter at `BatchDone` only when no failure
consumed budget since the read returned `Ok(Some)` or `Ok(None)` — the
ack settle shares the read task's counter, so a failed ack between the
read and `BatchDone` blocks the reset, and an ack that always fails
SHALL still exhaust the budget. The recovery half SHALL keep its own
attempt counter, independent of the read task's, and SHALL reset it
after each successful `XAUTOCLAIM` pass (a pass with zero claimed
entries is still a proven round trip). No loop SHALL reset its counter
on connect, and the initial `BatchDone` that precedes the first read
SHALL NOT reset. With recovery enabled the consumer holds a SECOND
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

#### Scenario: recovered blips do not consume the read budget

- **GIVEN** a read task whose connection experiences N transient read
  failures, each followed by a successful reconnect, a successful read,
  a successful ack settle, and `BatchDone`
- **WHEN** N exceeds the configured `max_attempts`
- **THEN** the read task has not exhausted its budget (the counter reset at
  each failure-free `BatchDone`)

#### Scenario: consecutive read failures still exhaust the budget

- **GIVEN** a read task whose reads fail transiently on every attempt
- **WHEN** consecutive failures reach `max_attempts`
- **THEN** the read task ends the session with `Err` (budget exhaustion)

#### Scenario: a failed ack between read and BatchDone blocks the reset

- **GIVEN** a read that returned `Ok(Some)`, followed by an ack settle that
  failed transiently and reconnected, before `BatchDone`
- **WHEN** `BatchDone` arrives
- **THEN** the attempt counter is not reset (the ack failure consumed
  budget and stays counted)

#### Scenario: recovery budget resets after a successful pass

- **GIVEN** a recovery half that consumed attempts on transient failures and
  then completes one `XAUTOCLAIM` pass successfully
- **WHEN** the next transient recovery failure occurs
- **THEN** it is counted from a reset counter (the successful pass returned
  the budget)
