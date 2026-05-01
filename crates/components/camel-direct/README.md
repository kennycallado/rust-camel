# camel-component-direct

> Direct component for rust-camel (in-memory routing)

## Overview

The Direct component provides synchronous, in-memory communication between routes. When a producer sends to a direct endpoint, it blocks until the consumer's route has finished processing the exchange.

This is ideal for connecting routes within the same Camel context and for building modular integration flows.

## Features

- **Synchronous**: Producer waits for consumer processing to complete
- **In-memory**: No network overhead, direct method calls
- **Request-Reply**: Returns the (possibly transformed) exchange
- **Dynamic**: Routes can be connected/disconnected at runtime

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-direct = "*"
```

## URI Format

```
direct:name
```

## Usage

### Connecting Routes

```rust
use camel_builder::RouteBuilder;
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;

let mut ctx = CamelContext::new();
ctx.register_component("direct", Box::new(DirectComponent::new()));

// Route 1: Producer
let route1 = RouteBuilder::from("timer:tick?period=1000")
    .to("direct:processing")
    .build()?;

// Route 2: Consumer
let route2 = RouteBuilder::from("direct:processing")
    .process(|ex| async move {
        println!("Processing: {:?}", ex.input.body.as_text());
        Ok(ex)
    })
    .to("mock:result")
    .build()?;

ctx.add_route(route1).await?;
ctx.add_route(route2).await?;
```

### Request-Reply Pattern

```rust
// Send request and receive response
let route = RouteBuilder::from("direct:request")
    .process(|mut ex| async move {
        // Process and modify exchange
        ex.input.body = camel_component_api::Body::Text("Response".to_string());
        Ok(ex)
    })
    .build()?;

// In another route
let client_route = RouteBuilder::from("timer:client")
    .set_body(camel_component_api::Body::Text("Request".to_string()))
    .to("direct:request")  // Blocks until response
    .log("Got response", camel_processor::LogLevel::Info)
    .build()?;
```

### Multiple Producers, One Consumer

```rust
// Multiple sources send to same processing route
let route_a = RouteBuilder::from("timer:a?period=1000")
    .set_header("source", Value::String("A".into()))
    .to("direct:process")
    .build()?;

let route_b = RouteBuilder::from("timer:b?period=2000")
    .set_header("source", Value::String("B".into()))
    .to("direct:process")
    .build()?;

let processor = RouteBuilder::from("direct:process")
    .log("Processing from ${header.source}", camel_processor::LogLevel::Info)
    .to("mock:output")
    .build()?;
```

## Error Propagation

Errors from the consumer route are propagated back to the producer:

```rust
let consumer = RouteBuilder::from("direct:may-fail")
    .process(|_| async {
        Err(CamelError::ProcessorError("Something went wrong".into()))
    })
    .build()?;

let producer = RouteBuilder::from("timer:tick")
    .to("direct:may-fail")  // Will receive error
    .build()?;
```

## Example: Modular Routes

```rust
use camel_builder::RouteBuilder;
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::new();
    ctx.register_component("direct", Box::new(DirectComponent::new()));

    // Validation module
    ctx.add_route(
        RouteBuilder::from("direct:validate")
            .filter(|ex| ex.input.body.as_text().is_some())
                .to("direct:process-valid")
            .end_filter()
            .build()?
    ).await?;

    // Processing module
    ctx.add_route(
        RouteBuilder::from("direct:process-valid")
            .process(|ex| async move { /* ... */ Ok(ex) })
            .to("direct:save")
            .build()?
    ).await?;

    // Storage module
    ctx.add_route(
        RouteBuilder::from("direct:save")
            .to("mock:database")
            .build()?
    ).await?;

    // Entry point
    ctx.add_route(
        RouteBuilder::from("http://0.0.0.0:8080/api")
            .to("direct:validate")
            .build()?
    ).await?;

    ctx.start().await?;
    Ok(())
}
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-direct)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
