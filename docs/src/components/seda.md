# SEDA

The SEDA (Staged Event-Driven Architecture) component stages exchanges in memory between routes that share one CamelContext. A producer sends to `seda:name` and returns. A consumer on the same name pulls from a bounded queue and processes asynchronously.

`seda:` is the asynchronous counterpart to `direct:`. Reach for it to decouple route lifetimes, smooth traffic bursts, or fan out to multiple subscribers.

The seda-demo wires a timer-driven producer against an asynchronous consumer that uppercases the body:

```rust,ignore
use camel_api::body::Body;
use camel_builder::RouteBuilder;
use camel_component_log::LogComponent;
use camel_component_seda::SedaComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
ctx.register_component(TimerComponent::new());
ctx.register_component(LogComponent::new());
ctx.register_component(SedaComponent::new());

let route_a = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
    .route_id("producer-route")
    .to("seda:processing")
    .build()?;

let route_b = RouteBuilder::from("seda:processing?concurrentConsumers=2")
    .route_id("consumer-route")
    .map_body(|body: Body| {
        if let Some(text) = body.as_text() {
            Body::Text(text.to_uppercase())
        } else {
            body
        }
    })
    .to("log:output?showBody=true&showHeaders=true")
    .build()?;

ctx.add_route_definition(route_a).await?;
ctx.add_route_definition(route_b).await?;
ctx.start().await?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: producer-route
    from: "timer:tick?period=1000&repeatCount=5"
    steps:
      - to: "seda:processing"
  - id: consumer-route
    from: "seda:processing?concurrentConsumers=2"
    steps:
      - to: "log:output?showBody=true&showHeaders=true"
```

Both interfaces compile to the same `RouteDefinition`. The example source is at [`examples/seda-demo`](https://github.com/kennycallado/rust-camel/tree/main/examples/seda-demo).

</details>

## URI

```
seda:<name>[?size=<n>][&concurrentConsumers=<n>][&multipleConsumers=<bool>][&blockWhenFull=<bool>][&discardIfNoConsumers=<bool>][&timeout=<ms>][&waitForTaskToComplete=<mode>][&exchangePattern=<pattern>]
```

| Parameter | Default | Description |
| --- | --- | --- |
| `size` | `1000` | Bounded queue capacity. Must be greater than 0 |
| `concurrentConsumers` | `1` | Concurrency hint. Clamped to 1 minimum. `0` becomes 1 with a warning |
| `multipleConsumers` | `false` | Fanout mode. One queue per subscriber. All-or-nothing delivery |
| `blockWhenFull` | `false` | Block the producer up to `timeout` when the queue is full. Default fails fast |
| `discardIfNoConsumers` | `false` | Drop silently when no consumer is active. Default returns an error |
| `timeout` | `30000` | Timeout in milliseconds for enqueue and reply wait |
| `waitForTaskToComplete` | `IfReplyExpected` | `Never`, `IfReplyExpected`, or `Always` |
| `exchangePattern` | `InOnly` | `InOnly` (fire-and-forget) or `InOut` (request-reply) |

Endpoints that share a name must agree on `size`, `multipleConsumers`, `exchangePattern`, and `concurrentConsumers`. The component rejects mismatched shared options with an `EndpointCreationFailed` error.

## Consumer

`seda:processing?concurrentConsumers=2` registers a consumer that pulls from the endpoint's bounded queue. The Runtime starts one queue forwarder task per consumer. That forwarder awaits `send_and_wait` for `InOut` and `waitForTaskToComplete=Always` exchanges, so those exchanges remain serial even when `concurrentConsumers` is greater than 1. `InOnly` exchanges without a reply channel do not block the forwarder.

`concurrentConsumers` is reported to the Runtime through `ConcurrencyModel::Concurrent`. Finding I1 and bd issue `rc-exa2` track the limitation that blocks true concurrent `InOut` processing.

The consumer transfers the primary forwarder handle to the Runtime through `background_task_handle()`. On shutdown, the Runtime aborts that handle, then calls `stop()`. `stop()` cancels the private token, aborts retained forwarders, and clears the active consumer registration.

## Producer

`seda:processing` creates a producer that enqueues the exchange. The producer returns immediately for fire-and-forget patterns. Behavior depends on `waitForTaskToComplete`:

- `Never` returns after enqueue. No reply channel is attached.
- `IfReplyExpected` waits only when `exchangePattern=InOut`.
- `Always` waits regardless of the exchange pattern.

When the producer waits, the forwarder on the consumer side uses `send_and_wait` to route the pipeline result back through a oneshot channel. A timeout returns `EndpointCreationFailed`. A closed channel returns `ChannelClosed`.

A full queue returns `EndpointCreationFailed` with the queue name and size. `blockWhenFull=true` makes the producer wait up to `timeout` for capacity. The route `ErrorHandler` owns both signals per [ADR-0019](../adr/0019-error-disposition-pipeline-recovery.md).

## Modes

`SedaMode::Single` owns one queue and permits one active consumer. A second registration on the same name returns an `EndpointCreationFailed` error.

`SedaMode::Fanout`, enabled by `multipleConsumers=true`, owns one queue per subscriber. A fanout producer reserves capacity for all subscribers before it sends, so delivery is all-or-nothing for the active subscriber set. Fanout rejects reply-waiting modes because one request has no single valid reply. The combination `multipleConsumers=true` with `waitForTaskToComplete=Never` is the only legal configuration.

## SEDA vs Direct

| | `direct:` | `seda:` |
| --- | --- | --- |
| Synchrony | Synchronous. Producer blocks until consumer finishes | Asynchronous. Producer returns after enqueue |
| Queue | None | Bounded (`size`) |
| Multiple consumers | No | Yes with `multipleConsumers=true` |
| Reply semantics | Reply is the consumer's pipeline result | Reply is optional. Controlled by `waitForTaskToComplete` |
| Failure propagation | Synchronous to the producer | Surfaces as `EndpointCreationFailed` or `ChannelClosed` after enqueue |
| Use case | Modular route linking with strict ordering | Decoupling, burst buffering, fanout, staging |

Reach for `direct:` when two routes must share a call stack and ordering is strict. Reach for `seda:` when you need to decouple producer and consumer lifetimes, smooth bursts, or broadcast to multiple subscribers.

## Error handling

A producer that targets an endpoint with no active consumer returns `EndpointCreationFailed` with the message `SEDA endpoint '<name>' has no active consumers`. Set `discardIfNoConsumers=true` to drop silently in this case.

A full queue with `blockWhenFull=false` returns `EndpointCreationFailed` with the queue name and configured size. Set `blockWhenFull=true` to wait up to `timeout`.

Route stop does not drain an in-flight reply. An interrupted `InOut` producer can receive `CamelError::ChannelClosed`. This is the current best-effort contract for in-memory staging. It is not an [ADR-0004](../adr/0004-hot-reload-atomic-pipeline-swap.md) hot-reload pipeline swap.

The public `ExchangePattern` and `WaitForTaskToComplete` enums are closed URI option sets. They stay exhaustive. [ADR-0049](../adr/0049-workspace-non-exhaustive-policy-for-v1-contract-enums.md) does not bind this component crate.

**Reference**: [SEDA crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-seda/CONTEXT.md). Example source: [`examples/seda-demo`](https://github.com/kennycallado/rust-camel/tree/main/examples/seda-demo).
