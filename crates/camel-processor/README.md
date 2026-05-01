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
- **RecipientList**: Dynamically resolve endpoints from expression at runtime
- **Dynamic Router**: Route to endpoints determined at runtime by a closure; includes same-destination loop detection
- **WireTap**: Fire-and-forget message copying
- **Circuit Breaker**: Resilience pattern implementation
- **Error Handler**: Centralized error handling
- **Stop**: Stop processing immediately
- **Script**: Execute mutating expressions via `ScriptMutator`; changes to headers, properties, and body propagate back with atomic rollback on error
- **Delayer**: Delay message processing with fixed or dynamic (header-based) duration
- **Stream Handling**: Processors that consume streams replace the body with a JSON placeholder `{"placeholder": true}`
- **Marshal / Unmarshal**: Serialize/deserialize message bodies using pluggable data formats (JSON, XML)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-processor = "*"
```

## Usage

### Using Processors Directly

```rust
use camel_processor::{
    FilterService, LogProcessor, LogLevel, SetHeader, SetBody,
    MapBody, SplitterService, MulticastService, DelayerService
};
use camel_api::{Exchange, Message, Body, Value, BoxProcessor, DelayConfig};

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

// Delay with fixed duration
let delayer = DelayerService::new(DelayConfig::new(500));

// Delay with dynamic header
let dynamic_delayer = DelayerService::new(
    DelayConfig::new(1000).with_dynamic_header("CamelDelayMs")
);
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
| `MarshalService` | Marshal body using a DataFormat (e.g., Json → Text) |
| `UnmarshalService` | Unmarshal body using a DataFormat (e.g., Text → Json) |
| `DelayerService` | Delay message processing by a fixed or dynamic duration |
| `RecipientListService` | Dynamic recipient list — resolve endpoints from expression at runtime |
| `DynamicRouterService` | Dynamic router — resolve destination at runtime via closure; detects same-destination loops |

## Data Formats

The `DataFormat` trait defines serialization/deserialization for message bodies:

| Format | Marshal (structured → wire) | Unmarshal (wire → structured) |
|--------|---------------------------|------------------------------|
| `json` | `Body::Json` → `Body::Text` | `Body::Text`/`Body::Bytes` → `Body::Json` |
| `xml`  | `Body::Json` → `Body::Text` | `Body::Text`/`Body::Bytes`/`Body::Xml` → `Body::Json` |

### Usage with RouteBuilder

```rust
use camel_builder::RouteBuilder;

let route = RouteBuilder::from("direct:in")
    .unmarshal("json")
    .marshal("xml")
    .to("mock:out")
    .build()
    .unwrap();
```

### YAML DSL

```yaml
steps:
  - unmarshal: json
  - marshal: xml
```

## Documentation

- [API Documentation](https://docs.rs/camel-processor)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
