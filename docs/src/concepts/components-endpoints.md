# Components and endpoints

A Component owns a URI scheme and builds the Endpoints that connect a Route to an external system. The Endpoint then creates the Consumer that pulls data in, or the Producer that sends data out.

```rust,ignore
{{#include ../../../examples/hello-world/src/main.rs:first-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: "hello-world"
    from: "timer:tick?period=1000&repeatCount=5"
    steps:
      - set_header:
          key: "source"
          value: "timer"
      - to: "log:info?showHeaders=true&showCorrelationId=true"
```

</details>

Two registrations sit before the Route. `TimerComponent` owns the `timer:` scheme. `LogComponent` owns the `log:` scheme. The Route then refers to those schemes by URI: `timer:tick?period=1000&repeatCount=5` as its source, and `log:info?showHeaders=true&showCorrelationId=true` as its sink.

## Component

A Component is a factory. It is identified by a URI scheme. Each scheme (`timer`, `log`, `http`, `kafka`) has exactly one Component. Components register into `CamelContext` by scheme at startup. The Runtime resolves every Route URI through this registry. The Component trait and the startup and shutdown contracts live in the [Component SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-api/CONTEXT.md).

## Endpoint

An Endpoint is an instantiated communication point. The Component creates it from a specific URI. The same Component can build many Endpoints from different URIs. `timer:tick?period=1000` and `timer:once?delay=2000` are two Endpoints from one `TimerComponent`. An Endpoint creates a Consumer for inbound traffic, or a Producer for outbound traffic.

## Consumer

A Consumer is the source side of a Route. The Runtime starts it for the `from:` Endpoint. It is event-driven. It receives data from an external system and submits Exchanges to the Pipeline. The Consumer runs for the lifetime of the Route.

Some Components also expose a pull-based `PollingConsumer`. A PollingConsumer does not start a Route. The `pollEnrich` verb and the WASM `camel_poll` host function use it to read a resource on demand ([ADR-0015](../adr/0015-endpoint-created-polling-consumer-for-pollenrich.md)).

## Producer

A Producer is the sink side. The Runtime creates it for each `to:` Endpoint. It sends an Exchange to an external system. Producers are strictly write and send. Every Producer is a Tower `Service<Exchange>`. To read a resource mid-route, use a `PollingConsumer`. Do not use a producer mode for reads.

## URI scheme resolution

When the Runtime builds a Route, it resolves each URI the same way:

1. Extract the scheme. This is the part before the first `:`. For `timer:tick?period=1000`, the scheme is `timer`.
2. Look up the registered Component for that scheme.
3. Call `Component::create_endpoint(uri)` to build the Endpoint.
4. The Endpoint creates the Consumer for a `from:` URI, or the Producer for a `to:` URI.

The path (`tick`) and the query (`period=1000&repeatCount=5`) belong to the Endpoint. The Component interprets them. A missing scheme or an unregistered scheme fails at startup, before the Route runs.

For the component catalog, see the [Components section](../components/index.md).

**Reference**: [Component SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-api/CONTEXT.md) · [Components bounded context](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md)
