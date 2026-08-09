# Direct

The Direct component routes an Exchange between two routes in the same CamelContext over an in-memory channel. The Producer blocks until the Consumer's Pipeline finishes. No serialization, no network. The transformed Exchange returns to the caller.

The multi-route-direct example wires a timer-driven producer and a transform consumer:

```rust,no_run
use camel_api::body::Body;
use camel_api::Value;
use camel_builder::RouteBuilder;
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), camel_api::CamelError> {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(DirectComponent::new());

    // Route A: timer -> direct:pipeline
    let route_a = RouteBuilder::from("timer:tick?period=1000")
        .route_id("route-a")
        .set_header("source", Value::String("timer".into()))
        .to("direct:pipeline")
        .build()?;

    // Route B: direct:pipeline -> uppercase -> log
    let route_b = RouteBuilder::from("direct:pipeline")
        .route_id("route-b")
        .map_body(|body: Body| {
            if let Some(text) = body.as_text() {
                Body::Text(text.to_uppercase())
            } else {
                body
            }
        })
        .to("log:output?showBody=true")
        .build()?;

    ctx.add_route_definition(route_a).await?;
    ctx.add_route_definition(route_b).await?;
    ctx.start().await?;
    Ok(())
}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: route-a
    from: "timer:tick?period=1000"
    steps:
      - set_header:
          key: "source"
          value: "timer"
      - to: "direct:pipeline"
  - id: route-b
    from: "direct:pipeline"
    steps:
      - to: "log:output?showBody=true"
```

Both routes use the same `direct:pipeline` name. The endpoint name must match on the producer and consumer sides.

</details>

## URI

```
direct:<name>[?timeout_ms=30000][&failIfNoConsumers=true]
```

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `timeout_ms` | no | `30000` | Producer `call()` timeout in milliseconds |
| `failIfNoConsumers` | no | `true` | Reject the call when no Consumer is registered for the name |
| `block` | no | `true` | Reserved for non-blocking send (TODO(DIR-001)) |
| `exchangePattern` | no | (none) | Reserved for pattern override (TODO(DIR-005)) |

The endpoint name must not be empty and must not contain whitespace. The Component rejects both at Endpoint creation.

## Consumer

`from: "direct:name"` registers a DirectConsumer in the shared registry. The Consumer starts a background loop that receives Exchanges from the in-memory channel. It submits each Exchange to the Route's Pipeline through `send_and_wait`. The reply carries the transformed Exchange or the failure.

One Consumer per name. A second Consumer on the same name returns `CamelError::EndpointCreationFailed` because the registry already holds an open channel. Routes that want many workers must use a different name, or pick a Component that supports fanout (SEDA with `multipleConsumers=true`).

The Consumer removes its registry entry on `stop()` or when the cancellation token fires. The next Producer call fails with `EndpointCreationFailed` until a new route registers.

## Producer

`to: "direct:name"` builds a DirectProducer. The Producer holds one in-flight call at a time through a bounded semaphore. `poll_ready` checks the registry and acquires a permit. `call` hands the Exchange to the Consumer's channel and awaits the reply.

`failIfNoConsumers=true` (default) rejects the call when no Consumer is registered. Set it to `false` to let the Producer race against late registration. The Producer still waits for the Consumer to receive the Exchange, so `false` does not give a fire-and-forget guarantee. Use SEDA for that.

The default `timeout_ms` is 30 000. A timeout returns `CamelError::ProcessorError` with a `timed out` message. The error propagates through the route's [error handler](../concepts/error-handling.md).

## Request-Reply

Direct is the natural request-reply Component for in-process calls. The Producer blocks until the Consumer's Pipeline finishes. Steps that follow `to: "direct:name"` see the transformed Exchange. Steps that set the body inside the Consumer's Route reach the caller. A timer that sends to a Direct endpoint and logs the result is a synchronous in-process function call.

## Error handling

The DirectConsumer reports unhandled pipeline failures through the `b-prime:direct:send-and-wait` metric and logs at `error!` (ADR-0012 category b'). Producer send failures (no Consumer, channel closed, reply dropped) are category (a) handler-owned. The Producer logs them at `warn!`. The route's error handler owns the operational signal.

**Reference**: [Direct crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-direct/CONTEXT.md). Example source: [`examples/multi-route-direct`](https://github.com/kennycallado/rust-camel/tree/main/examples/multi-route-direct).
