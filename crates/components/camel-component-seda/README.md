# camel-component-seda

> SEDA component for rust-camel (asynchronous in-memory staging)

## Overview

The SEDA (Staged Event-Driven Architecture) component provides asynchronous, in-memory communication between routes via bounded queues. Unlike the Direct component (synchronous), a producer sends to a SEDA endpoint and returns immediately — the consumer processes the exchange asynchronously from a background task.

This is ideal for decoupling routes, buffering bursts of messages, and building staged processing pipelines.

## Features

- **Asynchronous**: Producer returns immediately; consumer processes in background
- **Bounded queues**: Configurable queue size with fail-fast or blocking behavior
- **Exchange patterns**: InOnly (fire-and-forget) and InOut (request-reply with timeout)
- **Fanout**: Multiple consumers on the same SEDA name (pub-sub)
- **Configurable concurrency**: `concurrentConsumers` hint for parallel processing

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-seda = "*"
```

## URI Format

```
seda:name[?options]
```

### Options

| Option | Default | Description |
|--------|---------|-------------|
| `size` | `1000` | Queue capacity |
| `exchangePattern` | `InOnly` | `InOnly` or `InOut` |
| `multipleConsumers` | `false` | Enable fanout (multiple consumers) |
| `concurrentConsumers` | `1` | Concurrency hint for the runtime |
| `blockWhenFull` | `false` | Block producer if queue is full |
| `timeout` | `30000` | Timeout (ms) for InOut and blockWhenFull |
| `waitForTaskToComplete` | `IfReplyExpected` | `Never`, `IfReplyExpected`, or `Always` |
| `discardIfNoConsumers` | `false` | Silently discard if no active consumers |

## Usage

### Connecting Routes (Fire-and-Forget)

```rust
use camel_builder::RouteBuilder;
use camel_component_seda::SedaComponent;
use camel_core::context::CamelContext;

let mut ctx = CamelContext::new();
ctx.register_component(SedaComponent::new());

// Route 1: Producer (returns immediately)
let route1 = RouteBuilder::from("timer:tick?period=1000")
    .to("seda:processing")
    .build()?;

// Route 2: Consumer (processes asynchronously)
let route2 = RouteBuilder::from("seda:processing")
    .to("log:output?showBody=true")
    .build()?;

ctx.add_route(route1).await?;
ctx.add_route(route2).await?;
```

### Request-Reply (InOut)

```rust
// Consumer processes and replies
let consumer = RouteBuilder::from("seda:bridge?exchangePattern=InOut")
    .process(|mut ex| async move {
        // Transform and reply
        Ok(ex)
    })
    .build()?;

// Producer waits for reply (up to timeout)
let producer = RouteBuilder::from("timer:tick?period=1000")
    .to("seda:bridge?exchangePattern=InOut&timeout=5000")
    .to("log:reply?showBody=true")
    .build()?;
```

### Fanout (Multiple Consumers)

```rust
// Enable multipleConsumers — all subscribers receive each exchange
let publisher = RouteBuilder::from("timer:events?period=500")
    .to("seda:events?multipleConsumers=true")
    .build()?;

let subscriber_a = RouteBuilder::from("seda:events?multipleConsumers=true")
    .to("log:A?showBody=true")
    .build()?;

let subscriber_b = RouteBuilder::from("seda:events?multipleConsumers=true")
    .to("log:B?showBody=true")
    .build()?;
```

### Backpressure with blockWhenFull

```rust
// Producer blocks (up to timeout) when queue is full
let route = RouteBuilder::from("timer:fast?period=10")
    .to("seda:buffer?size=50&blockWhenFull=true&timeout=5000")
    .build()?;
```

## Differences from Direct

| | Direct | SEDA |
|---|---|---|
| Communication | Synchronous | Asynchronous |
| Producer blocks? | Yes (until consumer done) | No (unless `blockWhenFull`) |
| Queue | None | Bounded (`size`) |
| Multiple consumers | No | Yes (`multipleConsumers`) |
| Use case | Modular route linking | Decoupling, buffering, staging |

## Example

See [`examples/seda-demo/`](../../examples/seda-demo/) for a complete working example.

## Documentation

- [API Documentation](https://docs.rs/camel-component-seda)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
