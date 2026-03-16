# camel-processor

> Message processors for rust-camel (EIP implementations)

## Overview

`camel-processor` provides implementations of Enterprise Integration Patterns (EIP) as processors that transform, filter, and route messages in rust-camel pipelines. These are the building blocks you use when defining routes.

## Features

- **Filter**: Conditional message routing based on predicates
- **Choice**: Content-based routing with `when`/`otherwise` branches
- **Log**: Logging processor for debugging and monitoring
- **SetHeader / DynamicSetHeader**: Set message headers
- **SetBody / MapBody**: Transform message bodies
- **Splitter**: Split messages into multiple fragments
- **Aggregator**: Aggregate multiple messages into one
- **Multicast**: Send to multiple destinations in parallel
- **WireTap**: Fire-and-forget message copying
- **Circuit Breaker**: Resilience pattern implementation
- **Error Handler**: Centralized error handling
- **Stop**: Stop processing immediately
- **Script**: Execute mutating expressions via `ScriptMutator`; changes to headers, properties, and body propagate back with atomic rollback on error
- **Stream Handling**: Processors that consume streams replace the body with a JSON placeholder `{"placeholder": true}`

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-processor = "0.2"
```

## Usage

### Using Processors Directly

```rust
use camel_processor::{
    FilterService, LogProcessor, LogLevel, SetHeader, SetBody,
    MapBody, SplitterService, MulticastService
};
use camel_api::{Exchange, Message, Body, Value, BoxProcessor};

// Filter exchanges
let filter = FilterService::new(
    |ex: &Exchange| ex.input.body.as_text().map(|t| t.len() > 5).unwrap_or(false),
    my_processor
);

// Log messages
let logger = LogProcessor::new(LogLevel::Info, "Processing message".to_string());

// Set a header
let set_header = SetHeader::new(identity, "source", Value::String("api".into()));

// Map body
let upper = MapBody::new(identity, |body: Body| {
    body.as_text().map(|t| Body::Text(t.to_uppercase())).unwrap_or(body)
});
```

### With RouteBuilder (Recommended)

```rust
use camel_builder::RouteBuilder;

let route = RouteBuilder::from("timer:tick")
    .set_header("processed", Value::Bool(true))
    .filter(|ex| ex.input.body.as_text().is_some())
        .log("Processing text message", LogLevel::Info)
        .map_body(|b| Body::Text(b.as_text().unwrap_or("").to_uppercase()))
    .end_filter()
    .to("mock:result")
    .build()
    .unwrap();
```

## Available Processors

| Processor | Purpose |
|-----------|---------|
| `FilterService` | Route based on conditions |
| `ChoiceService` | Content-based routing (when/otherwise) |
| `LogProcessor` | Log exchange information |
| `SetHeader` | Set static header values |
| `DynamicSetHeader` | Set headers from expressions |
| `SetBody` | Replace message body |
| `MapBody` | Transform message body |
| `SplitterService` | Split messages |
| `AggregatorService` | Combine messages |
| `MulticastService` | Parallel routing |
| `WireTapService` | Side-channel routing |
| `CircuitBreakerLayer` | Fault tolerance |
| `ErrorHandlerLayer` | Error handling |
| `StopService` | Stop processing |
| `ScriptMutator` | Execute mutating scripts that modify Exchange headers, properties, or body |

## Documentation

- [API Documentation](https://docs.rs/camel-processor)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
