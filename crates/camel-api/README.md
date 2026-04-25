# camel-api

> Core traits and interfaces for rust-camel

## Overview

`camel-api` provides the fundamental building blocks for the rust-camel integration framework. This crate defines the core abstractions that all other crates build upon, including the `Exchange`, `Message`, `Processor`, and error handling types.

If you're building custom components or processors for rust-camel, you'll need to depend on this crate to implement the required traits and work with the message flow.

## Features

- **Exchange & Message**: Core message container types with headers, body, and properties
- **Processor trait**: The fundamental processing unit in the routing engine
- **Error handling**: Comprehensive error types and error handler configuration
- **Circuit breaker**: Circuit breaker configuration for resilience patterns
- **Aggregator & Splitter**: EIP patterns for message aggregation and splitting
- **Multicast**: Parallel message processing support
- **RecipientList**: Dynamic recipient list with expression-based endpoint resolution
- **Metrics**: Metrics collection interfaces
- **Route control**: Route controller traits for lifecycle management
- **Streaming**: Lazy `Body::Stream` variant with `materialize()`, `into_bytes()`, and `into_async_read()` for zero-copy I/O
- **Health monitoring**: Service health status tracking and Kubernetes-ready endpoints

## Unit of Work (UoW)

`UnitOfWorkConfig` enables per-route exchange lifecycle hooks.

- `on_complete`: optional producer URI invoked when an exchange finishes without error
- `on_failure`: optional producer URI invoked when processing returns an error or the exchange carries an error

```rust
use camel_api::{UnitOfWorkConfig, RuntimeQuery, RuntimeQueryResult};
use camel_core::{BuilderStep, RouteDefinition};

let route = RouteDefinition::new("direct:input", vec![BuilderStep::To("log:info".into())])
    .with_route_id("uow-route")
    .with_unit_of_work(UnitOfWorkConfig {
        on_complete: Some("direct:on-complete".to_string()),
        on_failure: Some("direct:on-failure".to_string()),
    });

// Runtime query example
let query = RuntimeQuery::InFlightCount {
    route_id: "uow-route".to_string(),
};

let _result = match query {
    RuntimeQuery::InFlightCount { route_id } => {
        RuntimeQueryResult::InFlightCount { route_id, count: 0 }
    }
    _ => unreachable!(),
};
```

Use `RuntimeQuery::InFlightCount { route_id }` to inspect current in-flight exchanges for a route.
The runtime returns either `RuntimeQueryResult::InFlightCount { route_id, count }` or
`RuntimeQueryResult::RouteNotFound { route_id }`.

## Health Monitoring

The health monitoring system provides Kubernetes-ready endpoints for monitoring service status.

### Core Types

| Type | Description |
|------|-------------|
| `HealthReport` | System-wide health report with status, services list, and timestamp |
| `ServiceHealth` | Health status of an individual service (name + status) |
| `HealthStatus` | Aggregated system health: `Healthy` or `Unhealthy` |
| `ServiceStatus` | Individual service status: `Stopped`, `Started`, or `Failed` |

### Usage Example

```rust
use camel_api::{HealthReport, HealthStatus, ServiceHealth, ServiceStatus, Lifecycle};
use chrono::Utc;

// Create a health report
let report = HealthReport {
    status: HealthStatus::Healthy,
    services: vec![
        ServiceHealth {
            name: "prometheus".to_string(),
            status: ServiceStatus::Started,
        },
    ],
    timestamp: Utc::now(),
};

// Check service status via Lifecycle trait
// (status() has a default implementation returning Stopped)
let service = MyService::new();
match service.status() {
    ServiceStatus::Started => println!("Service is running"),
    ServiceStatus::Stopped => println!("Service is stopped"),
    ServiceStatus::Failed => println!("Service failed"),
}
```

### Integration with CamelContext

Health monitoring integrates with `CamelContext` (in `camel-core`) to aggregate service status:

```rust
// In camel-core (inside an async context)
let ctx = CamelContext::builder().build().await?;
let report = ctx.health_check(); // Returns HealthReport
```

### Kubernetes Endpoints

The `camel-prometheus` crate exposes these endpoints:
- `/healthz` - Liveness probe (always 200 OK)
- `/readyz` - Readiness probe (200 if healthy, 503 if unhealthy)
- `/health` - Detailed health report (JSON)

See `camel-prometheus` documentation for Kubernetes integration examples.

## Platform SPI

`camel-api` defines platform ports for distributed coordination and platform-aware runtime wiring.

### `PlatformIdentity`

`PlatformIdentity` carries node identity (`node_id`), optional namespace, and free-form labels.

```rust
use camel_api::PlatformIdentity;

let identity = PlatformIdentity::local("dev-node");
```

### `PlatformService`

`PlatformService` is the unified entry point for platform capabilities: identity, readiness, and leadership.

```rust
use camel_api::{PlatformService, LeadershipService, PlatformIdentity};
use std::sync::Arc;

// Get the leadership service for per-lock leader election
let leadership = platform.leadership();
let handle = leadership.start("orders").await?;
assert!(handle.is_leader());
```

### `LeadershipService`

Per-lock leader election. Each call to `start(lock_name)` returns a `LeadershipHandle` for a specific lock.

```rust
use camel_api::{LeadershipService, LeadershipHandle, PlatformError};
use async_trait::async_trait;

struct MyLeadershipService;

#[async_trait]
impl LeadershipService for MyLeadershipService {
    async fn start(&self, lock_name: &str) -> Result<LeadershipHandle, PlatformError> {
        let _ = lock_name;
        todo!("start election loop for this lock")
    }
}
```

### `LeadershipHandle`

Tracks leadership state for a single lock. Use `is_leader()` to check, `step_down()` to release.

```rust
if handle.is_leader() {
    // running as leader for this lock
}
handle.step_down().await?; // releases lock, waits for cleanup
```

### `ReadinessGate`

Use `ReadinessGate` to signal ready/not-ready/starting state to your platform adapter.

```rust
use camel_api::ReadinessGate;
use async_trait::async_trait;

struct MyReadinessGate;

#[async_trait]
impl ReadinessGate for MyReadinessGate {
    async fn notify_ready(&self) {}
    async fn notify_not_ready(&self, _reason: &str) {}
    async fn notify_starting(&self) {}
}
```

### `HealthSource`

`HealthSource` exposes liveness/readiness/startup state for platform integrations without coupling crates. See `camel-health` for the HTTP endpoint implementation (`/healthz`, `/readyz`, `/startupz`) and concrete adapters.

### Local and test defaults

`NoopPlatformService`, `NoopLeadershipService`, and `NoopReadinessGate` are provided for local development and tests.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-api = "0.2"
```

## Usage

```rust
use camel_api::{Exchange, Message, Body, Processor, CamelError};
use camel_api::processor::ProcessorFn;

// Create a simple exchange
let message = Message::new("Hello, World!");
let exchange = Exchange::new(message);

// Access message body
if let Some(text) = exchange.input.body.as_text() {
    println!("Body: {}", text);
}

// Create a custom processor
let processor = ProcessorFn::new(|ex: Exchange| async move {
    // Transform the exchange
    let body = ex.input.body.as_text().unwrap_or("").to_uppercase();
    let mut ex = ex;
    ex.input.body = Body::Text(body);
    Ok(ex)
});

// Streams are lazily evaluated — materialize into bytes or stream directly
let bytes = body.materialize().await?;                    // Uses 10MB default limit
let bytes = body.into_bytes(100 * 1024 * 1024).await?;   // Custom limit
let reader = body.into_async_read();                      // Zero-copy AsyncRead (no RAM limit)
```

## Core Types

| Type | Description |
|------|-------------|
| `Exchange` | The message container flowing through routes |
| `Message` | Holds body, headers, and properties |
| `Body` | Message body (Empty, Text, Json, Bytes, Xml, Stream) |
| `Processor` | Trait for processing exchanges |
| `CamelError` | Comprehensive error type |
| `DataFormat` | Pluggable serialization trait for marshal/unmarshal EIP (implement to add custom formats; JSON and XML built-in) |
| `HealthReport` | System-wide health report |
| `ServiceStatus` | Service lifecycle status enum |
| `HealthStatus` | Aggregated health status enum |

## Documentation

- [API Documentation](https://docs.rs/camel-api)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
