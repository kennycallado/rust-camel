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

## Concurrency limitation

`concurrentConsumers` is reported to the Runtime through `ConcurrencyModel::Concurrent`. Each SEDA
Consumer currently starts one queue forwarder. That forwarder awaits `send_and_wait` for InOut and
`waitForTaskToComplete=Always`, so these exchanges remain serial even when
`concurrentConsumers > 1`. InOnly exchanges without a reply channel do not wait in the forwarder.
Finding I1 and bd issue `rc-exa2` track this limitation.

## Lifecycle

SEDA honors the Component SPI shutdown contract. `background_task_handle()` transfers the primary
forwarder handle to the Runtime for supervision. On shutdown, the Runtime aborts that handle and
then calls `stop()`. `stop()` cancels the private token, aborts retained forwarders, and clears the
active Consumer or fanout subscriber registration.

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
