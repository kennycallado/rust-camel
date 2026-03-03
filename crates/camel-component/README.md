# camel-component

> Component trait and registry for rust-camel

## Overview

`camel-component` defines the core `Component`, `Endpoint`, `Consumer`, and `Producer` traits that all rust-camel components must implement. This crate provides the foundation for creating new components that can integrate with the rust-camel routing engine.

If you're building a custom component (e.g., for a proprietary protocol or service), you'll implement the traits defined in this crate.

## Features

- **Component trait**: Factory for creating endpoints from URIs
- **Endpoint trait**: Represents a communication endpoint with consumer/producer support
- **Consumer trait**: Consumes messages from external sources
- **Producer trait**: Sends messages to external destinations
- **Concurrency support**: Built-in support for concurrent message processing

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component = "0.2"
```

## Usage

### Implementing a Custom Component

```rust
use camel_component::{Component, Endpoint, Consumer, ProducerContext, ConsumerContext};
use camel_api::{BoxProcessor, CamelError};

pub struct MyComponent;

impl Component for MyComponent {
    fn scheme(&self) -> &str {
        "mycomponent"
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        // Parse URI and create endpoint
        Ok(Box::new(MyEndpoint { uri: uri.to_string() }))
    }
}

struct MyEndpoint {
    uri: String,
}

impl Endpoint for MyEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(MyConsumer))
    }

    fn create_producer(&self, ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        // Create and return a producer
        todo!()
    }
}
```

## Core Types

| Trait | Description |
|-------|-------------|
| `Component` | Factory for creating endpoints |
| `Endpoint` | Communication endpoint (consumer/producer) |
| `Consumer` | Receives messages from external systems |
| `ProducerContext` | Context for creating producers |

## Documentation

- [API Documentation](https://docs.rs/camel-component)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
