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
| `camel-processor` | EIP processors: `Filter`, `SetHeader`, `MapBody` + Tower `Layer` types |
| `camel-endpoint` | Endpoint URI parsing utilities |
| `camel-timer` | Timer source component |
| `camel-log` | Log sink component |
| `camel-direct` | In-memory synchronous component |
| `camel-mock` | Test component with assertions on received exchanges |
| `camel-test` | Integration test harness |

## Building & Testing

```sh
cargo build --workspace
cargo test --workspace
```

Run an example:

```sh
cargo run -p hello-world
cargo run -p content-based-routing
cargo run -p multi-route-direct
cargo run -p transform-pipeline
cargo run -p showcase
```

## License

Apache-2.0
