# SEDA Component

In-memory asynchronous staging between Routes that share one CamelContext. The parent
[`components/CONTEXT.md`](../CONTEXT.md) defines Component, Endpoint, Consumer, Producer, SEDA,
and SEDA Fanout. This file records the crate-specific staging and lifecycle contracts.

## Staging model

A `SedaComponent` keeps one `SedaEndpointState` per endpoint name. Producers and Consumers with
the same name share that state. The bounded queue separates producer intake from Route processing.

`SedaMode::Single` owns one queue and permits one active Consumer. `SedaMode::Fanout`, enabled by
`multipleConsumers=true`, owns one queue per subscriber. A fanout Producer reserves capacity for
all subscribers before it sends, so delivery is all-or-nothing for the active subscriber set.
Fanout rejects reply-waiting modes because one request has no single valid reply.

`WaitForTaskToComplete` controls whether the Producer attaches a reply channel:

- `Never` returns after enqueue.
- `IfReplyExpected` waits only when `exchangePattern=InOut`.
- `Always` waits regardless of the Exchange pattern.

`concurrentConsumers=0` is clamped to `1`. Endpoints with the same name must agree on queue size,
mode, Exchange pattern, and `concurrentConsumers`.

## Concurrency

`concurrentConsumers` is reported to the Runtime through `ConcurrencyModel::Concurrent`. The
Consumer spawns N forwarder tasks, one per configured concurrent consumer.

`background_task_handle()` returns only one handle for Runtime supervision. The remaining N-1
handles are aborted in `stop()`. ADR-0007 crash supervision covers only the returned handle; the
others are crash-unsupervised during steady operation. This is accepted for v1.0.

## Lifecycle

SEDA honors the Component SPI shutdown contract. `background_task_handle()` transfers the primary
forwarder handle to the Runtime for supervision. On shutdown, the Runtime aborts that handle and
then calls `stop()`. `stop()` cancels the private token, aborts retained forwarders, and clears the
 active Consumer or fanout subscriber registration. Single-mode `stop()` also restores the queue
 receiver into the endpoint state (clearing the active flag first), so a fresh Consumer on the same
 endpoint can start again. Route stop/start cycles work in default mode; resume shares the same
 consumer-recreation path. Envelopes still queued inside the receiver when restoration occurs
 survive the cycle and are delivered after restart; already-dequeued or in-flight envelopes keep
 the existing best-effort shutdown behavior (no in-flight drain at stop).

Route stop does not drain an in-flight reply. An interrupted InOut Producer can receive
`CamelError::ChannelClosed`. This is the current best-effort contract for in-memory staging, not an
ADR-0004 hot-reload pipeline swap.

## `#[non_exhaustive]` posture

ADR-0049 does not bind this component crate. Its mandatory scope covers the three contract crates.
The public `ExchangePattern` and `WaitForTaskToComplete` enums are closed URI option sets, so they
remain exhaustive. No public enum in this crate uses `#[non_exhaustive]`.

## Related decisions

- ADR-0004: in-flight snapshot isolation applies to hot-reload pipeline swaps, not Route stop.
- ADR-0007: the Runtime supervises Consumer task failure.
- ADR-0019: `poll_ready` stays ready and send failures occur in `call()`.
- ADR-0049: mandatory `#[non_exhaustive]` scope excludes component implementation crates.
