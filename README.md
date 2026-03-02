# rust-camel

A Rust integration framework inspired by [Apache Camel](https://camel.apache.org/), built on [Tower](https://docs.rs/tower).

> **Status:** Pre-release (`0.1.0`). APIs will change.

## Overview

rust-camel lets you define message routes between components using a fluent builder API. The data plane (exchange processing, EIP patterns, middleware) is Tower-native — every processor and producer is a `Service<Exchange>`. The control plane (components, endpoints, consumers, lifecycle) uses its own trait hierarchy.

Current components: `timer`, `log`, `direct`, `mock`, `file`.

## Quick Example

```rust
use camel_api::{CamelError, Value};
use camel_builder::RouteBuilder;
use camel_core::context::CamelContext;
use camel_log::LogComponent;
use camel_timer::TimerComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
        .set_header("source", Value::String("timer".into()))
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    Ok(())
}
```

```sh
cargo run -p hello-world
```

## Crate Map

| Crate | Description |
|-------|-------------|
| `camel-api` | Core types: `Exchange`, `Message`, `CamelError`, `BoxProcessor`, `ProcessorFn` |
| `camel-core` | Runtime: `CamelContext`, `Route`, `RouteDefinition`, `SequentialPipeline` |
| `camel-builder` | Fluent `RouteBuilder` API |
| `camel-component` | `Component`, `Endpoint`, `Consumer` traits |
| `camel-processor` | EIP processors: `Filter`, `Splitter`, `Aggregator`, `WireTap`, `Multicast`, `SetHeader`, `MapBody` + Tower `Layer` types |
| `camel-endpoint` | Endpoint URI parsing utilities |
| `camel-timer` | Timer source component |
| `camel-log` | Log sink component |
| `camel-direct` | In-memory synchronous component |
| `camel-mock` | Test component with assertions on received exchanges |
| `camel-test` | Integration test harness |
| `camel-controlbus` | Control routes dynamically from within routes |

## Building & Testing

```sh
cargo build --workspace
cargo test --workspace
```

## Implemented EIP Patterns

| Pattern | Builder Method | Description |
|---------|---------------|-------------|
| Filter | `.filter(predicate)` | Forward exchange only when predicate is true |
| Splitter | `.split(config)` | Split one exchange into multiple fragments |
| Aggregator | `.aggregate(config)` | Correlate and aggregate multiple exchanges |
| WireTap | `.wire_tap(uri)` | Fire-and-forget copy to a tap endpoint |
| Multicast | `.multicast()` | Send the same exchange to multiple endpoints |
| Content-Based Router | `.choice()` / `.when()` | Route based on exchange content |

Run an example:

```sh
cargo run -p hello-world
cargo run -p content-based-routing
cargo run -p multi-route-direct
cargo run -p transform-pipeline
cargo run -p splitter
cargo run -p wiretap
cargo run -p multicast
cargo run -p showcase
```

## Security Features

rust-camel includes production-ready security features:

### SSRF Protection (HTTP Component)

```rust
// Block private IPs by default
RouteBuilder::from("direct:start")
    .to("http://api.example.com?allowPrivateIps=false")
    .build()?

// Custom blocked hosts
RouteBuilder::from("direct:start")
    .to("http://api.example.com?blockedHosts=localhost,internal.company.com")
    .build()?
```

### Path Traversal Protection (File Component)

All file operations validate that resolved paths remain within the configured base directory. Attempts to use `../` or absolute paths outside base are rejected.

### Timeouts

All I/O operations have configurable timeouts:
- File: `readTimeout`, `writeTimeout` (default: 30s)
- HTTP: `connectTimeout`, `responseTimeout`

### Memory Limits

Aggregator supports `max_buckets` and `bucket_ttl` to prevent memory leaks.

## Observability

### Correlation IDs

Every exchange has a unique `correlation_id` for distributed tracing.

### Metrics

Implement `MetricsCollector` trait to integrate with Prometheus, OpenTelemetry, etc.

## Route Lifecycle Management

rust-camel supports controlling when and how routes start:

### Auto Startup

By default, all routes start automatically when `ctx.start()` is called. You can disable this:

```rust
let route = RouteBuilder::from("timer:tick")
    .route_id("lazy-route")
    .auto_startup(false)  // Won't start automatically
    .to("log:info")
    .build()?;
```

### Startup Order

Control the order in which routes start (useful for dependencies):

```rust
let route_a = RouteBuilder::from("direct:a")
    .route_id("route-a")
    .startup_order(10)  // Starts first
    .to("log:info")
    .build()?;

let route_b = RouteBuilder::from("direct:b")
    .route_id("route-b")
    .startup_order(20)  // Starts after route-a
    .to("direct:a")
    .build()?;
```

### Runtime Control

Control routes dynamically from code or from other routes:

```rust
// From code:
ctx.route_controller().lock().await.start_route("lazy-route").await?;
ctx.route_controller().lock().await.stop_route("route-a").await?;

// From another route (using controlbus):
RouteBuilder::from("timer:monitor")
    .set_header("CamelRouteId", Value::String("backup-route".into()))
    .to("controlbus:route?action=start")
    .build()?
```

See `examples/lazy-route` for a complete example.

## License

Apache-2.0
