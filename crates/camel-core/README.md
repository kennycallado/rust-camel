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
- **Hot-reload**: Live route updates with ArcSwap (no downtime)
- **Supervision**: Auto-recovery with configurable exponential backoff
- **Tracer Integration**: Automatic message flow tracing support
- **Health Monitoring**: Service health checks via `health_check()` method
- **Bean Integration**: BeanRegistry support for YAML DSL bean step resolution

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

// Suspend a route (pauses consumer intake, allows in-flight exchanges to complete)
ctx.suspend_route("my-route").await?;

// Resume a suspended route (restarts consumer intake)
ctx.resume_route("my-route").await?;

// Stop a route
ctx.stop_route("my-route").await?;

// Check route status
let status = ctx.route_status("my-route");
```

## Health Monitoring

CamelContext provides a `health_check()` method to monitor the status of all registered services.

### Usage

```rust
use camel_core::context::CamelContext;
use camel_api::{HealthStatus, ServiceStatus};

let ctx = CamelContext::new();

// Add services (they implement the Lifecycle trait)
// ctx.with_lifecycle(prometheus_service);

// Check health of all services
let report = ctx.health_check();

match report.status {
    HealthStatus::Healthy => println!("All services are healthy"),
    HealthStatus::Unhealthy => {
        for service in &report.services {
            if service.status != ServiceStatus::Started {
                println!("Service {} is {:?}", service.name, service.status);
            }
        }
    }
}
```

### Integration with Prometheus

When using `camel-prometheus`, the health check is automatically exposed via HTTP endpoints:
- `/healthz` - Kubernetes liveness probe
- `/readyz` - Kubernetes readiness probe (returns 503 if unhealthy)
- `/health` - Detailed JSON health report

See the `camel-prometheus` crate for Kubernetes integration examples.

## Core Types

| Type | Description |
|------|-------------|
| `CamelContext` | Main context for route management |
| `Registry` | Component and endpoint registry |
| `Route` | A configured route |
| `RouteDefinition` | Route builder output |
| `RouteController` | Lifecycle management trait |
| `SupervisingRouteController` | Auto-recovery with exponential backoff for crashed consumers |
| `DefaultRouteController` | Default implementation with optional BeanRegistry (`with_beans()`) |

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

## Advanced Features

### SupervisingRouteController

Wraps any route controller with automatic recovery using configurable exponential backoff. When a consumer crashes, it automatically restarts after a delay that increases with each failure.

### Hot-reload System

Live route updates without service restart using `ArcSwap` and `ReloadCoordinator`. Update route definitions at runtime with zero downtime.

### ControlBus Integration

Dynamic route lifecycle management via the control bus pattern. Start, stop, suspend, and resume routes programmatically.

### Bean Integration

Use `DefaultRouteController::with_beans()` to enable bean step resolution in YAML DSL routes:

```rust
use camel_core::DefaultRouteController;
use camel_bean::BeanRegistry;

let mut bean_registry = BeanRegistry::new();
bean_registry.register("orderService", OrderService);

let controller = DefaultRouteController::with_beans(bean_registry);
// Pass controller to CamelContext::with_controller()
```

See `examples/bean-demo` for a complete example.

## Documentation

- [API Documentation](https://docs.rs/camel-core)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
