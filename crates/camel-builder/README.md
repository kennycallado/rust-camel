# camel-builder

> Fluent route builder API for rust-camel

## Overview

`camel-builder` provides a fluent, type-safe API for building routes in rust-camel. Inspired by Apache Camel's Java DSL, it allows you to define integration flows using method chaining with compile-time safety.

This is the primary way to define routes in rust-camel applications.

## Features

- **Fluent API**: Intuitive method chaining for route definition
- **Type Safety**: Compile-time verification of route structure
- **All EIP Patterns**: Support for filter, split, multicast, aggregate, and more
- **Error Handling**: Configure error handlers per route
- **Circuit Breaker**: Built-in resilience pattern support
- **Concurrency Control**: Sequential or concurrent processing modes
- **Route Lifecycle**: Configure auto-startup and startup order

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-builder = "0.2"
```

## Usage

### Basic Route

```rust
use camel_builder::RouteBuilder;
use camel_api::Value;

let route = RouteBuilder::from("timer:tick?period=1000")
    .set_header("source", Value::String("timer".into()))
    .log("Processing tick", camel_processor::LogLevel::Info)
    .to("mock:result")
    .build()
    .unwrap();
```

### Filter and Branching

```rust
let route = RouteBuilder::from("direct:input")
    .filter(|ex| ex.input.body.as_text().map(|t| t.starts_with("ERROR")).unwrap_or(false))
        .log("Error message detected", camel_processor::LogLevel::Warn)
        .to("direct:errors")
    .end_filter()
    .to("mock:result")
    .build()
    .unwrap();
```

### Message Transformation

```rust
use camel_api::Body;

let route = RouteBuilder::from("direct:input")
    .set_header("processed", Value::Bool(true))
    .map_body(|body: Body| {
        body.as_text()
            .map(|t| Body::Text(t.to_uppercase()))
            .unwrap_or(body)
    })
    .to("mock:output")
    .build()
    .unwrap();
```

### Splitter Pattern

```rust
use camel_api::splitter::{SplitterConfig, split_body_lines};

let route = RouteBuilder::from("file:/data/input?noop=true")
    .split(SplitterConfig::new(split_body_lines()).parallel(true))
        .log("Processing line", camel_processor::LogLevel::Debug)
        .to("direct:process-line")
    .end_split()
    .to("mock:complete")
    .build()
    .unwrap();
```

### Multicast Pattern

```rust
let route = RouteBuilder::from("direct:input")
    .multicast()
        .parallel(true)
        .to("direct:a")
        .to("direct:b")
        .to("direct:c")
    .end_multicast()
    .to("mock:result")
    .build()
    .unwrap();
```

### Error Handling

```rust
use camel_api::error_handler::ErrorHandlerConfig;

let route = RouteBuilder::from("direct:input")
    .error_handler(ErrorHandlerConfig::log_only())
    .process(|ex| async move {
        // Processing that might fail
        Ok(ex)
    })
    .to("mock:result")
    .build()
    .unwrap();
```

### Circuit Breaker

```rust
use camel_api::circuit_breaker::CircuitBreakerConfig;

let route = RouteBuilder::from("direct:input")
    .circuit_breaker(CircuitBreakerConfig::new().failure_threshold(5))
    .to("http://external-service/api")
    .build()
    .unwrap();
```

### Concurrency Control

```rust
// Concurrent processing (up to 16 in-flight)
let route = RouteBuilder::from("http://0.0.0.0:8080/api")
    .concurrent(16)
    .process(handle_request)
    .build()
    .unwrap();

// Sequential processing (ordered)
let route = RouteBuilder::from("http://0.0.0.0:8080/api")
    .sequential()
    .process(handle_request)
    .build()
    .unwrap();
```

### Route Configuration

```rust
let route = RouteBuilder::from("timer:tick")
    .route_id("my-timer-route")
    .auto_startup(true)
    .startup_order(10)
    .to("log:info")
    .build()
    .unwrap();
```

## Builder Methods

| Method | Description |
|--------|-------------|
| `from(uri)` | Start route from endpoint |
| `to(uri)` | Send to endpoint |
| `process(fn)` | Custom processing |
| `set_header(k, v)` | Set static header |
| `set_header_fn(k, fn)` | Set dynamic header |
| `set_body(v)` | Set static body |
| `set_body_fn(fn)` | Set dynamic body |
| `map_body(fn)` | Transform body |
| `filter(pred)` | Conditional routing |
| `split(config)` | Split messages |
| `multicast()` | Parallel routing |
| `wire_tap(uri)` | Side-channel copy |
| `aggregate(config)` | Combine messages |
| `log(msg, level)` | Log message |
| `stop()` | Stop processing |
| `error_handler(cfg)` | Set error handler |
| `circuit_breaker(cfg)` | Set circuit breaker |
| `concurrent(n)` | Enable concurrency |
| `sequential()` | Force sequential |
| `route_id(id)` | Set route ID |
| `auto_startup(bool)` | Auto-start control |
| `startup_order(n)` | Startup priority |

## Documentation

- [API Documentation](https://docs.rs/camel-builder)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
