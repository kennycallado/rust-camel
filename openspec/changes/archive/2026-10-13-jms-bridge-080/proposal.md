# Proposal: jms-bridge-080

## Why

The JMS bridge acknowledges on receipt, so a crash after handoff loses the message (bd rc-0v0if) and a failed route cannot request redelivery (bd rc-ntopt). The producer leaks credentials (bd rc-3j690); native images reject `jms.*` options (bd rc-odr5c).

## What Changes

One 0.8.0 release: completion settlement, durable reservations, parking.

- Completion settlement. One unacknowledged delivery per session. Rust uses `send_and_wait`; `Ok` sends COMMIT; `Err`, including `ChannelClosed`, sends ROLLBACK or drops the stream, never COMMIT. Settlement linearizes at the owner thread: cancel before prevents ack, cancel after and a lost commit stand without a second ack. Conversion failure recovers before the next receive.
- Lazy journal ownership. The per-destination lock is acquired at first subscribe and released when the last subscription ends (cancel, disconnect, stop, shutdown); producers and topics never lock. Readiness validates only the directory; a busy lock waits a bounded time then returns `FAILED_PRECONDITION`.
- Durable reservations. The count is reserved before handoff, including conversion failures and pre-handoff crashes. Rust derives the broker key and owns the default directory (`$XDG_STATE_HOME` else `$HOME/.local/state`), passing both via env on launch and restart; unset env defers to `FAILED_PRECONDITION` at queue subscribe. The journal domain is (directory, broker key, destination key) with one live owner; sequential owners inherit counts; N is the number of distinct journal domains consuming the queue within one retention window. Topics are non-durable.
- Crash-durable journal. Framed records, torn-tail replay that keeps counts, a fsynced atomic snapshot, and compaction at open and before a ceiling decision that retires stale entries, so the cap is not absolute forever.
- Park. A startup directory failure signals `permanent-failure` and parks the slot, terminal for every waiter; later permanent failures fail only the affected route; any task exit other than requested cancellation cancels siblings and surfaces to supervision.
- Poison. Reject-then-log after six reservations: one ack and a log with the message ID and count only, no dead-letter forwarding. The cap is terminal before a provider or broker native limit; docs name precedence, and redelivery is not promised to the source queue.
- Header filtering with Java parity; a kill -9 test has the survivor acquire the journal; native invocable reflection plus explicit `jms.connectionIDPrefix`; version 0.8.0 lands last.

Excluded: durable topic subscribers, `InOut`, IBM MQ, transport options outside `jms.*`, dead-letter forwarding, and owner-only tag or publish.

## Acceptance criteria

- A killed Rust process redelivers the message and a survivor acquires the journal.
- A successful route acknowledges once after completion; `Err` never commits; a propagated error is redelivered; the cap is six.
- A startup failure parks and waiters fail fast; a destination failure is route-only.
- Reservation state survives restart, stale entries retire, and credentials never reach JMS properties.

## Risk budget

Acceptable: a pre-1.0 breaking change; bridge and Rust ship together; at-least-once with bounded duplicates; the discard is explicit; fsync adds latency.

Out of bounds: acknowledging before a terminal policy; volatile-only reservation state; credentials on the wire; fabricated broker semantics; worker tag or publish.
