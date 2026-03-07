# camel-api

> Core traits and interfaces for rust-camel

## Overview

`camel-api` provides the fundamental building blocks for the rust-camel integration framework. This crate defines the core abstractions that all other crates build upon, including the `Exchange`, `Message`, `Processor`, and error handling types.

If you're building custom components or processors for rust-camel, you'll need to depend on this crate to implement the required traits and work with the message flow.

## Features

- **Exchange & Message**: Core message container types with headers, body, and properties
- **Processor trait**: The fundamental processing unit in the routing engine
- **Error handling**: Comprehensive error types and error handler configuration
- **Circuit breaker**: Circuit breaker configuration for resilience patterns
- **Aggregator & Splitter**: EIP patterns for message aggregation and splitting
- **Multicast**: Parallel message processing support
- **Metrics**: Metrics collection interfaces
- **Route control**: Route controller traits for lifecycle management
- **Streaming**: Lazy Body::Stream variant with materialize() helper and configurable memory limits

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-api = "0.2"
```

## Usage

```rust
use camel_api::{Exchange, Message, Body, Processor, CamelError};
use camel_api::processor::ProcessorFn;

// Create a simple exchange
let message = Message::new("Hello, World!");
let exchange = Exchange::new(message);

// Access message body
if let Some(text) = exchange.input.body.as_text() {
    println!("Body: {}", text);
}

// Create a custom processor
let processor = ProcessorFn::new(|ex: Exchange| async move {
    // Transform the exchange
    let body = ex.input.body.as_text().unwrap_or("").to_uppercase();
    let mut ex = ex;
    ex.input.body = Body::Text(body);
    Ok(ex)
});

// Streams are lazily evaluated and require explicit materialization
let bytes = body.materialize().await?; // Uses 10MB default limit
let bytes = body.into_bytes(100 * 1024 * 1024).await?; // Custom limit
```

## Core Types

| Type | Description |
|------|-------------|
| `Exchange` | The message container flowing through routes |
| `Message` | Holds body, headers, and properties |
| `Body` | Message body (Empty, Text, Json, Bytes, Stream) |
| `Processor` | Trait for processing exchanges |
| `CamelError` | Comprehensive error type |

## Documentation

- [API Documentation](https://docs.rs/camel-api)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
