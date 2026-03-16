# rust-camel

A Rust integration framework inspired by [Apache Camel](https://camel.apache.org/), built on [Tower](https://docs.rs/tower).

> **Status:** Pre-release (`0.4.0`). APIs will change.

## Overview

rust-camel lets you define message routes between components using a fluent builder API. The data plane (exchange processing, EIP patterns, middleware) is Tower-native — every processor and producer is a `Service<Exchange>`. The control plane (components, endpoints, consumers, lifecycle) uses its own trait hierarchy.

Current components: `timer`, `log`, `direct`, `mock`, `file`, `http`, `kafka`, `redis`, `sql`, `container`.

## Architecture

rust-camel implements **Domain-Driven Design (DDD)** with **CQRS** and **Hexagonal Architecture**:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              camel-core                                      │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                         Domain Layer                                 │   │
│  │   RouteRuntimeAggregate, RouteRuntimeState, RouteLifecycleCommand   │   │
│  │   RuntimeEvent (internal contract, no external deps)                │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                         │
│                                    ▼                                         │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                       Application Layer                              │   │
│  │   RuntimeBus (CommandBus + QueryBus), Command Handlers, Queries     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                         │
│                                    ▼                                         │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                          Ports Layer                                 │   │
│  │   RouteRepositoryPort, ProjectionStorePort, RuntimeExecutionPort,   │   │
│  │   EventPublisherPort, RuntimeEventJournalPort, CommandDedupPort     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                         │
│                                    ▼                                         │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        Adapters Layer                                │   │
│  │   InMemory* adapters, FileRuntimeEventJournal, RuntimeExecutionAdap │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Key Concepts

| Concept | Description |
|---------|-------------|
| **Aggregate** | `RouteRuntimeAggregate` owns route lifecycle state with optimistic locking |
| **Projection** | `RouteStatusProjection` is the read model for queries |
| **Command** | `RuntimeCommand` (RegisterRoute, StartRoute, StopRoute, etc.) |
| **Query** | `RuntimeQuery` (GetRouteStatus, ListRoutes) |
| **Event** | `RuntimeEvent` (RouteStarted, RouteStopped, RouteFailed, etc.) |
| **Journal** | Optional durable event journal for crash recovery |

### Runtime Bus

```rust
// Commands modify state
ctx.runtime().execute(RuntimeCommand::StartRoute {
    route_id: "my-route".into(),
    command_id: "cmd-1".into(),
    causation_id: None,
}).await?;

// Queries read from projections (not controller)
let status = ctx.runtime_route_status("my-route").await?;
```

### Optional Durability

```rust
// Enable file-backed event journal for runtime state recovery
let ctx = CamelContext::new_with_runtime_journal_path(".camel/runtime.jsonl");
```

## Quick Example

```rust
use camel_api::{CamelError, Value};
use camel_builder::RouteBuilder;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

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

    println!("Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await.ok();

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
| `camel-api` | Core types: `Exchange`, `Message`, `Body`, `CamelError`, `BoxProcessor`, `ProcessorFn`, `RuntimeCommand`, `RuntimeQuery`, `RuntimeEvent` |
| `camel-core` | Runtime engine with DDD/CQRS: `CamelContext`, domain aggregates, ports, adapters, event journal |
| `camel-config` | Configuration: `CamelConfig`, route discovery from YAML files with glob patterns |
| `camel-builder` | Fluent `RouteBuilder` API |
| `camel-component` | `Component`, `Endpoint`, `Consumer` traits |
| `camel-processor` | EIP processors: `Filter`, `Choice`, `Splitter`, `Aggregator`, `WireTap`, `Multicast`, `SetHeader`, `MapBody` + Tower `Layer` types |
| `camel-endpoint` | Endpoint URI parsing utilities; `UriConfig` derive macro for typed component config |
| `camel-endpoint-macros` | Proc-macro crate backing `#[derive(UriConfig)]` |
| `camel-bean` | Bean/Registry system for dependency injection and business logic integration |
| `camel-bean-macros` | Proc-macro crate for `#[bean]` attribute |
| `camel-dsl` | YAML DSL: load and run routes from `.yaml` files |
| `camel-health` | Health check types and endpoint support |
| `camel-timer` | Timer source component |
| `camel-log` | Log sink component |
| `camel-direct` | In-memory synchronous component |
| `camel-mock` | Test component with assertions on received exchanges (`await_exchanges`, `ExchangeAssert`) |
| `camel-test` | Integration test harness |
| `camel-controlbus` | Control routes dynamically from within routes |
| `camel-http` | HTTP producer (client) and HTTP consumer (server, native streaming) |
| `camel-file` | File producer and consumer |
| `camel-kafka` | Kafka producer and consumer with SSL/SASL and manual commit |
| `camel-redis` | Redis producer and consumer |
| `camel-sql` | SQL producer (query/insert/update) with streaming result support |
| `camel-container` | Docker container producer/consumer via `bollard` |
| `camel-language-api` | Language trait API: `Language`, `Expression`, `Predicate` |
| `camel-language-simple` | Simple Language: `${header.x}`, `${body}`, operators |
| `camel-language-rhai` | Rhai scripting language for full expression power |
| `camel-prometheus` | Prometheus metrics exporter with /metrics endpoint |
| `camel-otel` | OpenTelemetry tracing and metrics exporter |

## Building & Testing

```sh
cargo build --workspace
cargo test --workspace
```

## Implemented EIP Patterns

| Pattern | Builder Method | Description |
|---------|---------------|-------------|
| Aggregator | `.aggregate(config)` | Correlate and aggregate multiple exchanges |
| Content-Based Router | `.choice()` / `.when()` | Route based on exchange content |
| Dynamic Router | `.dynamic_router(expr)` | Expression-based routing with slip pattern |
| Filter | `.filter(predicate)` | Forward exchange only when predicate is true |
| Load Balancer | `.load_balance()` | Distribute across endpoints with RoundRobin/Random/Weighted/Failover |
| Multicast | `.multicast()` | Send the same exchange to multiple endpoints |
| Splitter | `.split(config)` | Split one exchange into multiple fragments |
| Throttler | `.throttle(n, duration)` | Rate limiting with Delay/Reject/Drop strategies |
| WireTap | `.wire_tap(uri)` | Fire-and-forget copy to a tap endpoint |

Run an example:

```sh
 cargo run -p aggregator
 cargo run -p bean-demo
 cargo run -p circuit-breaker
 cargo run -p config-basic
 cargo run -p config-profiles
 cargo run -p container-example
 cargo run -p content-based-routing
 cargo run -p controlbus
 cargo run -p dynamic-router
 cargo run -p error-handling
 cargo run -p file-pipeline
 cargo run -p file-polling
 cargo run -p hello-world
 cargo run -p http-client
 cargo run -p http-server
 cargo run -p http-streaming
 cargo run -p kafka-example
 cargo run -p language-rhai
 cargo run -p language-simple
 cargo run -p lazy-route
 cargo run -p load-balancer
 cargo run -p log-eip
 cargo run -p metrics-demo
 cargo run -p multicast
 cargo run -p multi-route-direct
 cargo run -p otel-demo
 cargo run -p pipeline-concurrency
 cargo run -p prometheus-demo
 cargo run -p showcase
 cargo run -p splitter
 cargo run -p sql-example
 cargo run -p sql-streaming
 cargo run -p throttler
 cargo run -p transform-pipeline
 cargo run -p wiretap
 cargo run -p xml-body
 cargo run -p yaml-dsl
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

### Prometheus Metrics

Export metrics to Prometheus with automatic lifecycle management:

```rust
use camel_prometheus::PrometheusService;

let ctx = CamelContext::new()
    .with_lifecycle(PrometheusService::new(9090))
    .with_tracing();

ctx.start().await?;
// Prometheus server starts automatically
```

Available metrics:
- `camel_exchanges_total{route}` - Total exchanges processed
- `camel_errors_total{route, error_type}` - Total errors
- `camel_exchange_duration_seconds{route}` - Exchange processing duration (histogram)
- `camel_queue_depth{route}` - Current queue depth
- `camel_circuit_breaker_state{route}` - Circuit breaker state

**Architecture:** `PrometheusService` implements `Lifecycle` trait (following Apache Camel's Service pattern, adapted to avoid tower::Service confusion).

### Health Monitoring

Built-in health endpoints for Kubernetes:

- `/healthz` - Liveness probe
- `/readyz` - Readiness probe  
- `/health` - Detailed health report

```yaml
livenessProbe:
  httpGet:
    path: /healthz
    port: 9090
```

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
let runtime = ctx.runtime();
runtime.execute(RuntimeCommand::StartRoute {
    route_id: "lazy-route".into(),
    command_id: "cmd-start-lazy-route".into(),
    causation_id: None,
}).await?;
runtime.execute(RuntimeCommand::StopRoute {
    route_id: "route-a".into(),
    command_id: "cmd-stop-route-a".into(),
    causation_id: None,
}).await?;

// From another route (using controlbus):
RouteBuilder::from("timer:monitor")
    .set_header("CamelRouteId", Value::String("backup-route".into()))
    .to("controlbus:route?action=start")
    .build()?
```

See `examples/lazy-route` for a complete example.

Canonical route compile support in Facade V1 is intentionally limited to `to/log/stop`.

## Error Handling

rust-camel provides sophisticated error handling with retry policies and dead letter channels.

### RedeliveryPolicy with Jitter

Configure retry behavior with exponential backoff and jitter:

```rust
use camel_api::error_handler::{ErrorHandlerConfig, RedeliveryPolicy};
use std::time::Duration;

let error_handler = ErrorHandlerConfig::dead_letter_channel("log:errors")
    .on_exception(|e| matches!(e, CamelError::Io(_)))
    .retry(3)  // Max 3 retry attempts
    .with_backoff(
        Duration::from_millis(100),  // Initial delay: 100ms
        2.0,                          // Multiplier: 2x
        Duration::from_secs(10)      // Max delay: 10s
    )
    .with_jitter(0.2)  // ±20% randomization (recommended: 0.1-0.3)
    .build();
```

**Jitter Benefits:**
- Prevents thundering herd in distributed systems
- Recommended values: 0.1-0.3 (10-30%)
- Adds randomization: `delay ± (delay * jitter_factor)`

### Camel-Compatible Headers

During retries, these headers are automatically set:

- `CamelRedelivered` - `true` when exchange is being retried
- `CamelRedeliveryCounter` - Current retry attempt (1-indexed)
- `CamelRedeliveryMaxCounter` - Maximum retry attempts

### YAML Configuration

```yaml
routes:
  - id: "retry-example"
    from: "timer:tick"
    error_handler:
      dead_letter_uri: "log:dlc"
      retry:
        max_attempts: 3
        initial_delay_ms: 100
        multiplier: 2.0
        max_delay_ms: 10000
        jitter_factor: 0.2
    steps:
      - to: "direct:processor"
```

See `examples/error-handling` for complete examples.

## Configuration

rust-camel supports external configuration via `Camel.toml` files using the `camel-config` crate:

### Configuration File

Create a `Camel.toml` file:

```toml
[default]
routes = ["routes/**/*.yaml"]
log_level = "INFO"

# Component defaults - apply to all endpoints
[default.components.http]
connect_timeout_ms = 5000
allow_private_ips = false

[default.components.kafka]
brokers = "localhost:9092"
group_id = "camel"

[default.components.redis]
host = "localhost"
port = 6379

[default.components.sql]
max_connections = 5

[default.components.file]
delay_ms = 500

[default.components.container]
docker_host = "unix:///var/run/docker.sock"

# Observability
[default.observability.prometheus]
enabled = true
port = 9090

[production]
log_level = "ERROR"

[production.components.kafka]
brokers = "prod-kafka:9092"

[production.components.redis]
host = "prod-redis"
```

### Component Defaults

Each component supports global defaults that apply to all endpoints. URI parameters always take precedence:

```rust
// Uses global connect_timeout_ms (5000) from Camel.toml
.to("http://api.example.com/data")

// Overrides global setting with URI parameter
.to("http://api.example.com/data?connectTimeout=10000")
```

Supported component configurations:
- **`[components.http]`**: `connect_timeout_ms`, `response_timeout_ms`, `max_connections`, `max_body_size`, `max_request_body`, `allow_private_ips`
- **`[components.kafka]`**: `brokers`, `group_id`, `session_timeout_ms`, `request_timeout_ms`, `auto_offset_reset`, `security_protocol`
- **`[components.redis]`**: `host`, `port`
- **`[components.sql]`**: `max_connections`, `min_connections`, `idle_timeout_secs`, `max_lifetime_secs`
- **`[components.file]`**: `delay_ms`, `initial_delay_ms`, `read_timeout_ms`, `write_timeout_ms`
- **`[components.container]`**: `docker_host`

### Loading Configuration

```rust
use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_config::{CamelConfig, discover_routes};
use camel_core::CamelContext;

// Load configuration
let config = CamelConfig::from_file("Camel.toml")
    .map_err(|e| CamelError::Config(e.to_string()))?;

// Create context and register components
let mut ctx = CamelContext::new();
ctx.register_component(TimerComponent::new());
ctx.register_component(LogComponent::new());

// Discover and load routes from config patterns
let routes = discover_routes(&config.routes)
    .map_err(|e| CamelError::Config(e.to_string()))?;
for route in routes {
    ctx.add_route_definition(route)?;
}

ctx.start().await?;
```

### Route Files

Create YAML route files:

```yaml
# routes/hello.yaml
routes:
  - id: "hello-timer"
    from: "timer:tick?period=1000"
    steps:
      - to: "log:info"
```

### Environment Variables

Override configuration with environment variables:

```bash
# Select profile
export CAMEL_PROFILE=production

# Override specific values
export CAMEL_LOG_LEVEL=DEBUG
export CAMEL_ROUTES_0="custom/*.yaml"
```

### Features

- **Profile support**: Multiple environments in one file
- **Route discovery**: Auto-load routes from glob patterns
- **Component defaults**: Set global defaults for HTTP, Kafka, Redis, SQL, File, Container
- **Environment overrides**: Override any value with `CAMEL_*` prefix
- **Deep merging**: Nested configs merge properly
- **URI precedence**: URI parameters always override global defaults

### Component Defaults

Configure global defaults for all component endpoints in `Camel.toml`:

```toml
[default.components.http]
connect_timeout_ms = 5000
allow_private_ips = false

[default.components.kafka]
brokers = "localhost:9092"
group_id = "my-app"

[default.components.redis]
host = "localhost"
port = 6379

[default.components.sql]
max_connections = 10

[default.components.file]
delay_ms = 1000

[default.components.container]
docker_host = "unix:///var/run/docker.sock"
```

URI parameters always take precedence over global defaults:

```rust
// Uses global connect_timeout_ms (5000) from Camel.toml
.to("http://api.example.com/data")

// Overrides global setting with URI parameter
.to("http://api.example.com/data?connectTimeout=10000")
```

See `docs/configuration.md` for full details.

## License

Apache-2.0
