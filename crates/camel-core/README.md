# camel-core

> Core routing engine for rust-camel

## Overview

`camel-core` is the heart of the rust-camel framework. It provides the `CamelContext` for managing routes, the `Registry` for component registration, and the core route execution engine. This crate orchestrates all the other components to enable message routing.

This is the main crate you'll use when building a rust-camel application. It brings together components, processors, and routes into a cohesive integration framework.

## Features

- **CamelContext**: Central context for managing routes and components
- **Registry**: Component registry for endpoint resolution
- **Route**: Route definitions and lifecycle management
- **RouteController**: Start, stop, suspend, and resume routes
- **Pipeline composition**: Tower-based middleware composition

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-core = "0.2"
```

## Usage

### Creating a Camel Context

```rust
use camel_core::CamelContext;
use camel_builder::RouteBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create the context
    let mut ctx = CamelContext::new();

    // Register components
    ctx.register_component("timer", Box::new(camel_component_timer::TimerComponent::new()));
    ctx.register_component("log", Box::new(camel_component_log::LogComponent::new()));
    ctx.register_component("mock", Box::new(camel_component_mock::MockComponent::new()));

    // Build and add routes
    let route = RouteBuilder::from("timer:hello?period=1000")
        .log("Timer fired!", camel_processor::LogLevel::Info)
        .to("mock:result")
        .build()?;

    ctx.add_route(route).await?;

    // Start all routes
    ctx.start().await?;

    // ... run your application

    // Graceful shutdown
    ctx.stop().await?;

    Ok(())
}
```

### Route Lifecycle Management

```rust
// Start a specific route
ctx.start_route("my-route").await?;

// Suspend a route (pause without stopping consumers)
ctx.suspend_route("my-route").await?;

// Resume a suspended route
ctx.resume_route("my-route").await?;

// Stop a route
ctx.stop_route("my-route").await?;

// Check route status
let status = ctx.route_status("my-route");
```

## Core Types

| Type | Description |
|------|-------------|
| `CamelContext` | Main context for route management |
| `Registry` | Component and endpoint registry |
| `Route` | A configured route |
| `RouteDefinition` | Route builder output |
| `RouteController` | Lifecycle management trait |

## Architecture

```
┌─────────────────────────────────────┐
│           CamelContext              │
│  ┌─────────────────────────────┐   │
│  │         Registry            │   │
│  │  ┌───────┐ ┌───────┐       │   │
│  │  │Timer  │ │ Log   │  ...  │   │
│  │  │Comp   │ │ Comp  │       │   │
│  │  └───────┘ └───────┘       │   │
│  └─────────────────────────────┘   │
│  ┌─────────────────────────────┐   │
│  │         Routes              │   │
│  │  Route 1  │  Route 2  │ ...│   │
│  └─────────────────────────────┘   │
└─────────────────────────────────────┘
```

## Documentation

- [API Documentation](https://docs.rs/camel-core)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
