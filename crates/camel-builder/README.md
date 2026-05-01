# camel-builder

> Fluent route builder API for rust-camel

## Overview

`camel-builder` provides a fluent, type-safe API for building routes in rust-camel. Inspired by Apache Camel's Java DSL, it allows you to define integration flows using method chaining with compile-time safety.

This is the primary way to define routes in rust-camel applications.

Routes can also be defined via YAML using camel-dsl. See the camel-dsl crate for details.

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
camel-builder = "*"
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

### Content-Based Router (Choice)

```rust
use camel_api::Value;

let route = RouteBuilder::from("direct:input")
    .choice()
    .when(|ex| ex.input.header("priority").map(|v| v == &Value::String("high".into())).unwrap_or(false))
        .to("direct:high-priority")
    .end_when()
    .when(|ex| ex.input.header("priority").map(|v| v == &Value::String("medium".into())).unwrap_or(false))
        .to("direct:medium-priority")
    .end_when()
    .otherwise()
        .to("direct:low-priority")
    .end_otherwise()
    .end_choice()
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

### RecipientList Pattern

```rust
use camel_api::recipient_list::{RecipientListConfig, RecipientListExpression};

let expression: RecipientListExpression = Arc::new(|ex: &Exchange| {
    ex.input.header("destinations")
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
});

let route = RouteBuilder::from("direct:input")
    .recipient_list(RecipientListConfig::new(expression))
    .to("mock:result")
    .build()
    .unwrap();
```

### Aggregate Pattern

```rust
use camel_api::aggregator::AggregatorConfig;
use std::time::Duration;

// Size-based completion (basic)
let route = RouteBuilder::from("timer:orders?period=200")
    .aggregate(
        AggregatorConfig::correlate_by("orderId")
            .complete_when_size(3)
            .build(),
    )
    .to("log:info")
    .build()
    .unwrap();

// Size OR timeout completion (v2)
let route = RouteBuilder::from("timer:orders?period=200")
    .aggregate(
        AggregatorConfig::correlate_by("orderId")
            .complete_on_size_or_timeout(3, Duration::from_secs(5))
            .force_completion_on_stop(true)
            .build(),
    )
    .to("log:info")
    .build()
    .unwrap();
```

### Delay Pattern

```rust
use std::time::Duration;

let route = RouteBuilder::from("direct:input")
    // Delay processing by 500ms
    .delay(Duration::from_millis(500))
    // Delay with dynamic header override
    .delay_with_header(Duration::from_secs(1), "CamelDelayMs")
    .to("mock:result")
    .build()
    .unwrap();
```

#### AggregatorConfig Builder Methods

| Method | Description |
|--------|-------------|
| `correlate_by(header)` | Correlate exchanges by header value |
| `complete_when_size(n)` | Complete when bucket reaches `n` exchanges |
| `complete_on_timeout(duration)` | Complete on inactivity timeout (resets on each exchange) |
| `complete_on_size_or_timeout(n, duration)` | Complete on either condition (first wins) |
| `force_completion_on_stop(bool)` | Force-complete all buckets when route stops |
| `discard_on_timeout(bool)` | Discard incomplete exchanges on timeout instead of emitting |
| `strategy(strategy)` | Custom aggregation strategy (`Fn(Exchange, Exchange) -> Exchange`) |
| `max_buckets(n)` | Maximum concurrent correlation buckets |
| `bucket_ttl(duration)` | TTL for idle buckets |

### Script Step

Execute a script that can modify the Exchange in-place (headers, properties, body):

```rust
RouteBuilder::from("direct:input")
    .script("rhai", r#"headers["tenant"] = "acme"; body = body + "_processed""#)
    .to("log:result")
    .route_id("script-route")
    .build()?;
```

The language must be registered in the `CamelContext`. On error, all modifications are rolled back.

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

Shorthand route-level exception clauses:

```rust
use camel_api::CamelError;
use std::time::Duration;

let route = RouteBuilder::from("direct:input")
    .route_id("shorthand-errors")
    .dead_letter_channel("log:dlc")
    .on_exception(|e| matches!(e, CamelError::Io(_)))
        .retry(3)
        .with_backoff(Duration::from_millis(100), 2.0, Duration::from_secs(1))
        .handled_by("log:io")
    .end_on_exception()
    .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
        .retry(1)
    .end_on_exception()
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

### Marshal/Unmarshal

```rust
use camel_builder::RouteBuilder;

let route = RouteBuilder::from("direct:in")
    .unmarshal("json")
    .marshal("xml")
    .to("mock:out")
    .route_id("marshal-unmarshal-route")
    .build()
    .unwrap();
```

This converts the message body from JSON to XML using the built-in data formats.

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
| `transform(v)` | Alias for `set_body` |
| `map_body(fn)` | Transform body |
| `unmarshal(format)` | Unmarshal body using a data format |
| `marshal(format)` | Marshal body using a data format |
| `filter(pred)` | Conditional routing |
| `choice()` | Open content-based router block |
| `when(pred)` | Open when-clause (inside `choice`) |
| `end_when()` | Close when-clause |
| `otherwise()` | Open fallback branch (inside `choice`) |
| `end_otherwise()` | Close fallback branch |
| `end_choice()` | Close content-based router block |
| `split(config)` | Split messages |
| `multicast()` | Parallel routing |
| `recipient_list(config)` | Dynamic recipient list from expression |
| `wire_tap(uri)` | Side-channel copy |
| `aggregate(config)` | Combine messages |
| `log(msg, level)` | Log message |
| `stop()` | Stop processing |
| `script(language, source)` | Execute a script that can modify headers, properties, and body |
| `error_handler(cfg)` | Set explicit error handler config |
| `dead_letter_channel(uri)` | Set DLC for shorthand error mode |
| `on_exception(pred)` | Open shorthand exception clause |
| `end_on_exception()` | Close shorthand exception clause |
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
