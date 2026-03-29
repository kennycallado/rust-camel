# camel-core

> Core routing engine with DDD/CQRS/Hexagonal architecture for rust-camel

## Overview

`camel-core` is the heart of the rust-camel framework. It implements a **Domain-Driven Design** architecture with **CQRS** and **Hexagonal Architecture** organized as vertical bounded contexts.

### Bounded Contexts

```
crates/camel-core/src/
  lifecycle/          ← Route lifecycle management
    domain/           │  RouteRuntimeAggregate, RouteRuntimeState, RuntimeEvent
    application/      │  RuntimeBus, Command/Query handlers, RouteDefinition
    ports/            │  RouteRepositoryPort, ProjectionStorePort, …
    adapters/         │  InMemory*, RedbRuntimeEventJournal, DefaultRouteController
  hot_reload/         ← Live route updates
    domain/           │  ReloadAction
    application/      │  compute_reload_actions, execute_reload_actions
    adapters/         │  ReloadWatcher
  shared/             ← Cross-cutting concerns
    observability/    │  TracerConfig, TracingProcessor (OTel adapter)
    components/       │  Registry (component lookup by URI scheme)
  context.rs          ← CamelContext (composition root)
```

## Features

- **CamelContext**: Central context for managing routes and components
- **DDD Aggregate**: `RouteRuntimeAggregate` with state machine and optimistic locking
- **CQRS Runtime Bus**: Separate command/query paths with projection-backed reads
- **Event Sourcing**: Optional durable journal (redb v2) for crash recovery
- **Hexagonal Architecture**: Clean separation via ports and adapters
- **Hot-reload**: Live route updates with zero downtime
- **Supervision**: Auto-recovery with configurable exponential backoff
- **Tracer EIP**: Automatic message-flow tracing with configurable detail levels
- **Metrics**: Pluggable `MetricsCollector` integration
- **Optional languages**: `lang-js` and `lang-rhai` feature flags

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-core = "0.5"
```

### Optional Features

| Feature | Description |
|---------|-------------|
| `lang-js` | JavaScript scripting via `camel-language-js` |
| `lang-rhai` | Rhai scripting via `camel-language-rhai` |

```toml
[dependencies]
camel-core = { version = "0.5", features = ["lang-rhai"] }
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

Enable redb-backed event journal for runtime state recovery across restarts:

```rust
use camel_core::{CamelContext, RedbJournalOptions, JournalDurability};

// Default options (Immediate durability, compaction at 10_000 events)
let ctx = CamelContext::new_with_redb_journal(
    ".camel/runtime.redb",
    RedbJournalOptions::default(),
).await?;

// Eventual durability (dev/test — no fsync)
let ctx = CamelContext::new_with_redb_journal(
    ".camel/runtime.redb",
    RedbJournalOptions {
        durability: JournalDurability::Eventual,
        compaction_threshold_events: 1_000,
    },
).await?;

// With supervision and metrics
let ctx = CamelContext::with_supervision_and_metrics_and_redb_journal(
    supervision_config,
    metrics,
    ".camel/runtime.redb",
    RedbJournalOptions::default(),
).await?;
```

Events are persisted and replayed on startup for crash recovery.

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
| `RedbRuntimeEventJournal` | RuntimeEventJournalPort (redb v2) |
| `RuntimeExecutionAdapter` | RuntimeExecutionPort |

## Tracer EIP

Automatic message-flow tracing across all route steps:

```rust
use camel_core::{CamelContext, TracerConfig, DetailLevel, TracerOutputs, StdoutOutput, OutputFormat};

// Simple toggle
let mut ctx = CamelContext::new();
ctx.set_tracing(true);

// Full configuration
let config = TracerConfig {
    enabled: true,
    detail_level: DetailLevel::Medium,
    outputs: TracerOutputs {
        stdout: Some(StdoutOutput { enabled: true, format: OutputFormat::Json }),
        file: None,
    },
};
ctx.set_tracer_config(config);
```

Or via `Camel.toml`:

```toml
[observability.tracer]
enabled = true
detail_level = "minimal"  # minimal | medium | full

[observability.tracer.outputs.stdout]
enabled = true
format = "json"
```

**Detail levels:**

| Level | Fields |
|-------|--------|
| `minimal` | `correlation_id`, `route_id`, `step_id`, `step_index`, `timestamp`, `duration_ms`, `status` |
| `medium` | + `headers_count`, `body_type`, `has_error`, `output_body_type` |
| `full` | + up to 3 message headers (`header_0`…`header_2`) |

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

## Metrics

Plug in a custom `MetricsCollector` at construction time:

```rust
use camel_core::CamelContext;
use std::sync::Arc;

let ctx = CamelContext::with_metrics(Arc::new(my_metrics_collector));

// Access the collector later
let metrics = ctx.metrics();
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
- Bounded contexts do not bypass each other's layers
- `shared/` cross-cutting types are only accessed via canonical paths
- Runtime side effects flow through `RuntimeExecutionPort`

## Documentation

- [API Documentation](https://docs.rs/camel-core)
- [DDD/CQRS Report](./DDD_CQRS_GO_NO_GO_REPORT.md)
- [Facade V1 Contract](./DDD_CQRS_FACADE_V1_CONTRACT.md)

## License

Apache-2.0
