# camel-core

> Core routing engine with DDD/CQRS architecture for rust-camel

## Overview

`camel-core` is the heart of the rust-camel framework. It implements a **Domain-Driven Design** architecture with **CQRS** (Command Query Responsibility Segregation) and **Hexagonal Architecture** (Ports and Adapters).

### Architecture Layers

```
┌─────────────────────────────────────────────────────────────────┐
│                         Domain Layer                            │
│   RouteRuntimeAggregate, RouteRuntimeState, RuntimeEvent        │
│   (Pure business logic, no external dependencies)               │
├─────────────────────────────────────────────────────────────────┤
│                      Application Layer                          │
│   RuntimeBus, Command Handlers, Query Handlers                  │
│   (Orchestrates domain operations via ports)                    │
├─────────────────────────────────────────────────────────────────┤
│                         Ports Layer                             │
│   RouteRepositoryPort, ProjectionStorePort, RuntimeExecutionPort│
│   (Interfaces, no implementations)                              │
├─────────────────────────────────────────────────────────────────┤
│                       Adapters Layer                            │
│   InMemory*, FileRuntimeEventJournal, RuntimeExecutionAdapter   │
│   (Concrete implementations)                                    │
└─────────────────────────────────────────────────────────────────┘
```

## Features

- **CamelContext**: Central context for managing routes and components
- **DDD Aggregate**: `RouteRuntimeAggregate` with state machine and optimistic locking
- **CQRS Runtime Bus**: Separate command/query paths with projection-backed reads
- **Event Sourcing**: Optional durable journal for crash recovery
- **Hexagonal Architecture**: Clean separation via ports and adapters
- **Hot-reload**: Live route updates with zero downtime
- **Supervision**: Auto-recovery with configurable exponential backoff

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-core = "0.4"
```

## Usage

### Creating a Camel Context

```rust
use camel_core::CamelContext;
use camel_builder::RouteBuilder;
use camel_component_timer::TimerComponent;
use camel_component_log::LogComponent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::new();

    // Register components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Build and add routes
    let route = RouteBuilder::from("timer:hello?period=1000")
        .route_id("hello-route")
        .to("log:info")
        .build()?;

    ctx.add_route_definition(route).await?;

    // Start all routes
    ctx.start().await?;

    // Graceful shutdown
    ctx.stop().await?;

    Ok(())
}
```

### Runtime Bus (CQRS)

Control routes via the runtime bus:

```rust
use camel_api::RuntimeCommand;

// Start a route
ctx.runtime().execute(RuntimeCommand::StartRoute {
    route_id: "my-route".into(),
    command_id: "cmd-start-1".into(),
    causation_id: None,
}).await?;

// Query route status (reads from projection)
let status = ctx.runtime_route_status("my-route").await?;
println!("Status: {:?}", status);
```

### Optional Durability

Enable file-backed event journal for runtime state recovery across restarts:

```rust
// With durability
let ctx = CamelContext::new_with_runtime_journal_path(".camel/runtime.jsonl");

// Events are persisted and replayed on startup
```

## Core Types

### Domain Layer

| Type | Description |
|------|-------------|
| `RouteRuntimeAggregate` | DDD aggregate with lifecycle state and version |
| `RouteRuntimeState` | Enum: Registered, Starting, Started, Suspended, Stopping, Stopped, Failed |
| `RouteLifecycleCommand` | Domain commands: Start, Stop, Suspend, Resume, Reload, Fail |
| `RuntimeEvent` | Domain events: RouteStarted, RouteStopped, RouteFailed, etc. |

### Ports Layer

| Port | Purpose |
|------|---------|
| `RouteRepositoryPort` | Load/save aggregates |
| `ProjectionStorePort` | Read/write route status projections |
| `RuntimeExecutionPort` | Execute side effects on route controller |
| `EventPublisherPort` | Publish domain events |
| `RuntimeEventJournalPort` | Durable event persistence |
| `CommandDedupPort` | Idempotent command handling |
| `RuntimeUnitOfWorkPort` | Atomic aggregate + projection + event persistence |

### Adapters Layer

| Adapter | Implements |
|---------|------------|
| `InMemoryRouteRepository` | RouteRepositoryPort |
| `InMemoryProjectionStore` | ProjectionStorePort |
| `InMemoryRuntimeStore` | Combined in-memory implementation |
| `FileRuntimeEventJournal` | RuntimeEventJournalPort |
| `RuntimeExecutionAdapter` | RuntimeExecutionPort |

## Health Monitoring

```rust
use camel_api::{HealthStatus, ServiceStatus};

let report = ctx.health_check();

match report.status {
    HealthStatus::Healthy => println!("All services healthy"),
    HealthStatus::Unhealthy => {
        for service in &report.services {
            println!("{}: {:?}", service.name, service.status);
        }
    }
}
```

## Hot-Reload System

Live route updates without service restart:

```rust
use camel_core::reload_watcher::{watch_and_reload, resolve_watch_dirs};

let handle = ctx.runtime_execution_handle();
let patterns = vec!["routes/*.yaml".to_string()];
let watch_dirs = resolve_watch_dirs(&patterns);

watch_and_reload(
    watch_dirs,
    handle,
    || camel_dsl::discover_routes(&patterns)
        .map_err(|e| CamelError::RouteError(e.to_string())),
    Some(cancel_token),
).await?;
```

## Supervision

`SupervisingRouteController` wraps any controller with automatic crash recovery:

```rust
use camel_core::SupervisingRouteController;
use camel_api::supervision::SupervisionConfig;

let config = SupervisionConfig {
    initial_delay_ms: 1000,
    backoff_multiplier: 2.0,
    max_delay_ms: 60000,
    max_attempts: 5,
};

let ctx = CamelContext::with_supervision(config);
```

## Architecture Tests

The crate includes hexagonal architecture boundary tests to ensure clean separation:

```bash
cargo test -p camel-core --test hexagonal_architecture_boundaries_test
```

Tests verify:
- Domain layer has no infrastructure dependencies
- Application layer depends only on ports and domain
- Ports layer has no adapter dependencies
- Runtime side effects flow through `RuntimeExecutionPort`

## Documentation

- [API Documentation](https://docs.rs/camel-core)
- [DDD/CQRS Report](./DDD_CQRS_GO_NO_GO_REPORT.md)
- [Facade V1 Contract](./DDD_CQRS_FACADE_V1_CONTRACT.md)

## License

Apache-2.0
