# rust-camel

A Rust-native, Tower-native integration framework inspired by [Apache Camel](https://camel.apache.org/), built for async pipelines, EIP patterns, and production observability.

> **Status:** Pre-release (`0.24.0`). APIs will change.

## Overview

rust-camel lets you define message routes between components using a fluent builder API. The data plane (exchange processing, EIP patterns, middleware) is Tower-native — every processor and producer is a `Service<Exchange>`. The control plane (components, endpoints, consumers, lifecycle) uses its own trait hierarchy.

Current components: `timer`, `cron`, `log`, `direct`, `exec`, `mock`, `file`, `http`, `ws`/`wss`, `kafka`, `mqtt`, `redis`, `sql`, `jms`, `cxf`, `container`, `controlbus`, `validator`, `xslt`, `xj`, `master`, `opensearch`, `llm`, `surrealdb`, `grpc`, `seda`, `keycloak`, `wasm`.

## Architecture

rust-camel separates two planes:

- **Data plane** — Tower-native. Every processor and producer is a `Service<Exchange>`. EIP patterns compose as Tower middleware.
- **Control plane** — its own trait hierarchy: `Component`, `Endpoint`, `Consumer`, route lifecycle, supervision, hot-reload.

```
┌──────────────────────────────────────────────────────┐
│                    Your Application                  │
│         RouteBuilder / YAML DSL / camel-config       │
└──────────────────────┬───────────────────────────────┘
                       │
┌──────────────────────▼───────────────────────────────┐
│                    camel-core                        │
│  CamelContext — composition root                     │
│                                                      │
│  ┌──────────────────────────────────────────────┐    │
│  │  Data plane (Tower)                          │    │
│  │  Exchange → Service<Exchange> pipeline       │    │
│  │  EIP processors as Tower middleware          │    │
│  └──────────────────────────────────────────────┘    │
│                                                      │
│  ┌──────────────────────────────────────────────┐    │
│  │  Control plane                               │    │
│  │  Route lifecycle, supervision, hot-reload    │    │
│  │  RuntimeBus (CQRS), event journal (redb)     │    │
│  └──────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────┘
          │                          │
┌─────────▼──────────┐   ┌───────────▼────────────────┐
│   camel-processor  │   │   Components               │
│   EIP patterns     │   │   timer, log, http, file,  │
│   (Tower layers)   │   │   kafka, redis, sql, opensearch, ...,  │
└────────────────────┘   └────────────────────────────┘
```

> The internal DDD/CQRS/hexagonal structure of `camel-core` is an implementation detail — see [`crates/camel-core/README.md`](crates/camel-core/README.md) for those internals.

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
// Enable redb-backed event journal for runtime state recovery
let ctx = CamelContext::builder()
    .runtime_store(camel_core::InMemoryRuntimeStore::default().with_journal(std::sync::Arc::new(
        camel_core::RedbRuntimeEventJournal::new(
            ".camel/runtime.redb",
            RedbJournalOptions::default(),
        )
        .await?,
    )))
    .build()
    .await?;
```

## Quick Start with the CLI

```bash
cargo install camel-cli
camel new my-integration
cd my-integration
camel run
```

This scaffolds a project with a `Camel.toml`, `routes/hello.yaml`, and runs it.
See [`crates/camel-cli/README.md`](crates/camel-cli/README.md) for all CLI commands.

```bash
# Generate OpenAPI document from REST route files
camel openapi generate routes.yaml --title "My API"
```

## Docker

Pre-built images are available on [GHCR](https://github.com/kennycallado/rust-camel/pkgs/container/rust-camel) and [Docker Hub](https://hub.docker.com/r/kennycallado/rust-camel).

```bash
# Pull the scratch image (production)
docker pull ghcr.io/kennycallado/rust-camel:latest

# Pull the alpine image (debugging)
docker pull ghcr.io/kennycallado/rust-camel:latest-alpine

# Run with routes mounted
docker run -v $(pwd)/routes:/app/routes ghcr.io/kennycallado/rust-camel:latest camel run

# Interactive shell (alpine)
docker run -it -v $(pwd):/app ghcr.io/kennycallado/rust-camel:latest-alpine sh

# Docker Hub equivalent
docker pull kennycallado/rust-camel:latest
```

Two image variants are published per release:

| Tag suffix | Base        | Use case                            |
| ---------- | ----------- | ----------------------------------- |
| _(none)_   | scratch     | Production. Minimal attack surface. |
| `-alpine`  | alpine:3.21 | Debugging. Includes busybox shell.  |

Both variants support `linux/amd64` and `linux/arm64`.

## Quick Rust Example

```rust
use camel_api::{CamelError, Value};
use camel_builder::RouteBuilder;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
        .set_header("source", Value::String("timer".into()))
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await.ok();

    ctx.stop().await?;
    Ok(())
}
```

```sh
cargo run
```

## REST DSL & OpenAPI

Define REST APIs declaratively in YAML with automatic JSON binding, path
templates, schema validation, and OpenAPI document generation.

```yaml
rest:
  - host: 0.0.0.0
    port: 8080
    path: /api/users
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
        produces: application/json
      - method: POST
        operation_id: createUser
        consumes: application/json
        produces: application/json
        success_status: 201
        to: direct:createUser
        request_schema:
          type: object
          properties:
            name:
              type: string
            email:
              type: string
          required: [name, email]
      - method: GET
        path: /{id}
        operation_id: getUser
        to: direct:getUser
      - method: DELETE
        path: /{id}
        operation_id: deleteUser
        to: direct:deleteUser
        success_status: 204
```

The `rest:` block lowers to `http:` consumer routes with:
- `unmarshal(json)` for body verbs (POST/PUT/PATCH)
- JSON schema validation when `request_schema` is present (→ 400 on failure)
- `marshal(json)` + `Content-Type: application/json` on the response path
- Default status codes (200/201/204) via `CamelHttpResponseCode` header

Generate an OpenAPI 3.0.3 document from REST routes:

```bash
camel openapi generate routes.yaml --title "Users API" --version "1.0.0"
```

See [`examples/rest-crud/`](examples/rest-crud/README.md) for a complete
runnable example with in-memory CRUD storage and static file co-hosting.

## Crate Map

| Crate                       | Description                                                                                                                                                                                                                                                  |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --- | ------------------------------------ |
| `camel-api`                 | Core types: `Exchange`, `Message`, `Body`, `CamelError`, `BoxProcessor`, `ProcessorFn`, `RuntimeCommand`, `RuntimeQuery`, `RuntimeEvent`, `FromBody`, `impl_from_body_via_serde!`                                                                            |
| `camel-core`                | Runtime engine with DDD/CQRS: `CamelContext`, domain aggregates, ports, adapters, event journal                                                                                                                                                              |
| `camel-config`              | Configuration: `CamelConfig`, route discovery from YAML files with glob patterns                                                                                                                                                                             |
| `camel-builder`             | Fluent `RouteBuilder` API                                                                                                                                                                                                                                    |
| `camel-component`           | `Component`, `Endpoint`, `Consumer` traits                                                                                                                                                                                                                   |
| `camel-processor`           | EIP processors: `Filter`, `Choice`, `Splitter`, `StreamingSplitter`, `Aggregator`, `WireTap`, `Multicast`, `SetHeader`, `MapBody`, `Marshal`/`Unmarshal` + Tower `Layer` types                                                                               |
| `camel-endpoint`            | Endpoint URI parsing utilities; `UriConfig` derive macro for typed component config                                                                                                                                                                          |
| `camel-endpoint-macros`     | Proc-macro crate backing `#[derive(UriConfig)]`                                                                                                                                                                                                              |
| `camel-wit`                 | WIT interface definitions crate for camel components                                                                                                                                                                                                            |
| `camel-bean`                | Bean/Registry system for dependency injection and business logic integration                                                                                                                                                                                 |
| `camel-bean-macros`         | Proc-macro crate for `#[bean]` attribute                                                                                                                                                                                                                     |
| `camel-dsl`                 | YAML DSL: load and run routes from `.yaml` files                                                                                                                                                                                                             |
| `camel-health`              | Health check types and endpoint support                                                                                                                                                                                                                      |
| `camel-platform-kubernetes` | Kubernetes-native platform SPI: leader election, readiness gates, pod identity                                                                                                                                                                               |
| `camel-timer`               | Timer source component                                                                                                                                                                                                                                       |
| `camel-cron`                | Cron source component — calendar-triggered scheduling via Unix 5-field cron expressions, backed by `CronService` SPI                                                                                                                                        |
| `camel-log`                 | Log sink component                                                                                                                                                                                                                                           |
| `camel-direct`              | In-memory synchronous component                                                                                                                                                                                                                              |
| `camel-mock`                | Test component with assertions on received exchanges (`await_exchanges`, `ExchangeAssert`)                                                                                                                                                                   |
| `camel-test`                | Integration test harness                                                                                                                                                                                                                                     |
| `camel-controlbus`          | Control routes dynamically from within routes                                                                                                                                                                                                                |
| `camel-validator`           | Validate body against JSON/YAML schemas, plus XSD via xml-bridge                                                                                                                                                                                             |
| `camel-http`                | HTTP producer (client) and HTTP consumer (server, native streaming)                                                                                                                                                                                          |
| `camel-file`                | File producer and consumer                                                                                                                                                                                                                                   |
| `camel-kafka`               | Kafka producer and consumer with SSL/SASL and manual commit                                                                                                                                                                                                  |
| `camel-mqtt`                | MQTT 3.1.1 producer and consumer (rumqttc-v4-next) with QoS 0/1/2, manual/auto ack, TLS via mqtts://                                                                                                                                                         |
| `camel-redis`               | Redis producer and consumer                                                                                                                                                                                                                                  |
| `camel-opensearch`          | OpenSearch producer with 7 operations (INDEX, SEARCH, GET, DELETE, UPDATE, BULK, MULTIGET)                                                                                                                                                                   |
| `camel-sql`                 | SQL producer/consumer with IN clause separator, SSL/TLS, streaming result support                                                                                                                                                                            |
| `camel-surrealdb`           | SurrealDB multi-model (document, graph, vector, live) with 9 operations                                                                                                                                                                                      |
| `camel-jms`                 | JMS producer and consumer via native-image bridge (ActiveMQ Classic, Artemis)                                                                                                                                                                                |
| `camel-component-cxf`       | SOAP/Web Services via Apache CXF native-image bridge: SOAP 1.1/1.2, WSDL, WS-Security, multi-tenant profiles, PAYLOAD mode                                                                                                                                   |
| `camel-component-exec`      | Fail-closed system command execution with profile-pinned binaries, per-element arg-policy, env sanitization, cwd confinement, timeout with process-group kill (`exec:{profile}`)                                                                             |
| `camel-grpc`                | gRPC producer and consumer with dynamic proto resolution, unary + server/client/bidi streaming                                                                                                                                                               |
| `camel-xslt`                | XSLT 3.0 transformation via xml-bridge (`xslt:<stylesheet>`) — [example](examples/xslt-example/README.md)                                                                                                                                                    |
| `camel-xj`                  | XML↔JSON conversion via xml-bridge (`xj:<stylesheet>?direction=xml2json\|json2xml`) — [example](examples/xj-example/README.md)                                                                                                                              |
| `camel-container`           | Docker container producer/consumer via `bollard`. Container lifecycle, volume mounts, exec, network operations                                                                                                                                               |
| `camel-component-llm`       | LLM chat (streaming + materialized), embeddings, tool calling, multi-turn conversations, response cache, cost observability, retry, and concurrency control via OpenAI, Ollama, or Mock. Strict siumai adapter boundary (ADR-0020) (`llm:chat`, `llm:embed`) |
| `camel-language-api`        | Language trait API: `Language`, `Expression`, `Predicate`                                                                                                                                                                                                    |
| `camel-language-simple`     | Simple Language: `${header.x}`, `${body}`, operators, `&&`/\`                                                                                                                                                                                                |     | \`, boolean literals, null semantics |
| `camel-language-js`         | JavaScript scripting language for expressions and side effects                                                                                                                                                                                               |
| `camel-language-rhai`       | Rhai scripting language for full expression power                                                                                                                                                                                                            |
| `camel-language-jsonpath`   | RFC 9535 JSONPath expressions: `$.items[*].price`. Requires `lang-jsonpath` feature.                                                                                                                                                                         |
| `camel-language-xpath`      | XPath 1.0 language for XML body queries: `/books/book[1]/title`. Requires `lang-xpath` feature.                                                                                                                                                              |
| `camel-language-minijinja`  | MiniJinja inline template rendering (HTML/JSON/none autoescape). Requires `lang-minijinja` feature. Phase 1: inline only; Phase 2 (bd rc-64if) adds external file loading.                                                                                    |
| `camel-prometheus`          | Prometheus metrics exporter with /metrics endpoint                                                                                                                                                                                                           |
| `camel-otel`                | OpenTelemetry tracing and metrics exporter                                                                                                                                                                                                                   |
| [`examples/rest-crud`]       | REST DSL + OpenAPI example — CRUD API with schema validation, static co-hosting                                                                                                                                                                               |

## Building & Testing

```sh
cargo build --workspace
cargo test --workspace
cargo bench --workspace
```

The benchmark suite uses [Criterion](https://bheisler.github.io/criterion.rs/book/) with HTML reports. Per-crate inline benchmarks cover core types, processors, and the DSL. The `camel-bench` crate provides integration pipeline benchmarks.

### Test Coverage

```sh
scripts/coverage.sh
```

Requires `cargo-llvm-cov`. Coverage baseline is enforced via `coverage.toml` (currently 75% minimum). Adjust the baseline there if coverage changes intentionally.

## Implemented EIP Patterns

| Pattern                     | Builder Method                                | Description                                                                                                                                               |
| --------------------------- | --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Aggregator                  | `.aggregate(config)`                          | Correlate and aggregate exchanges with size/timeout completion, expression correlation, force-complete-on-stop                                            |
| Content-Based Router        | `.choice()` / `.when()`                       | Route based on exchange content                                                                                                                           |
| Content Enricher            | `.enrich(uri)` / `.poll_enrich(uri, timeout)` | Merge additional content mid-route via producer (`enrich`) or polling consumer (`pollEnrich`); pluggable `EnrichmentStrategy` (default `UseEnrichedBody`) |
| Delayer                     | `.delay()` / `delay:`                         | Fixed or dynamic delay (header-based)                                                                                                                     |
| doTry / doCatch / doFinally | `.do_try()` / `do_try:`                       | Lexical-scope try/catch/finally with ADR-0019 dispositions                                                                                                |
| Dynamic Router              | `.dynamic_router(expr)`                       | Expression-based routing with slip pattern                                                                                                                |
| Routing Slip                | `.routing_slip(expr)`                         | Route through a sequence of endpoints determined at runtime                                                                                               |
| Filter                      | `.filter(predicate)`                          | Forward exchange only when predicate is true                                                                                                              |
| Load Balancer               | `.load_balance()`                             | Distribute across endpoints with RoundRobin/Random/Weighted/Failover                                                                                      |
| Loop                        | `.loop_count(n)` / `loop:`                    | Iterate a sub-pipeline N times or while a predicate holds true                                                                                            |
| Marshal / Unmarshal         | `.marshal(fmt)` / `.unmarshal(fmt)`           | Serialize/deserialize bodies using pluggable data formats (JSON, XML, ZIP, CSV)                                                                           |
| Multicast                   | `.multicast()`                                | Send the same exchange to multiple endpoints                                                                                                              |
| RecipientList               | `.recipient_list(config)`                     | Dynamically resolve endpoint URIs from an expression at runtime                                                                                           |
| Splitter                    | `.split(config)`                              | Split one exchange into multiple fragments (body lines, ZIP entries, streaming)                                                                           |
| Stream Cache                | `.stream_cache(n)` / `stream_cache:`          | Materialize `Body::Stream` into `Body::Bytes` with configurable threshold (128 KB default)                                                                |
| Throttler                   | `.throttle(n, duration)`                      | Rate limiting with Delay/Reject/Drop strategies                                                                                                           |
| WireTap                     | `.wire_tap(uri)`                              | Fire-and-forget copy to a tap endpoint                                                                                                                    |

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
 cargo run -p do-try
 cargo run -p dynamic-router
 cargo run -p recipientlist
 cargo run -p routing-slip
 cargo run -p error-handling
 cargo run -p file-pipeline
 cargo run -p file-polling
 cargo run -p hello-world
 cargo run -p http-client
 cargo run -p http-server
 cargo run -p http-streaming
 cargo run -p kafka-example
 cargo run -p jms-example
 cargo run -p opensearch-example
 cargo run -p language-rhai
 cargo run -p language-simple
 cargo run -p lazy-route
 cargo run -p load-balancer
 cargo run -p loop-example
 cargo run -p log-eip
 cargo run -p marshal-unmarshal
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
    .to("http://api.example.com?allowInternal=false")
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

let ctx = CamelContext::builder().build().await?
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

Platform SPI support for Kubernetes:

- Leader election via Kubernetes Leases
- Readiness gate via pod status conditions
- Pod identity auto-detection from Downward API

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

## Type Converters

The pipeline automatically coerces the exchange body to the type a component endpoint declares via `body_contract()`. No manual casting needed in `.process()` closures.

```rust
// Declare what your endpoint expects (component author):
fn body_contract(&self) -> Option<BodyType> {
    Some(BodyType::Text)  // pipeline coerces to Text before calling producer
}

// Deserialize body into any type (route author):
use camel_api::impl_from_body_via_serde;
use serde::Deserialize;

#[derive(Deserialize)]
struct Order { id: u64, amount: f64 }
impl_from_body_via_serde!(Order);

RouteBuilder::from("direct:orders")
    .process(|ex| async move {
        let order: Order = ex.body_as::<Order>()?;
        // ...
        Ok(ex)
    })
    .to("sql:insert into orders ...")
    .build()?;
```

Built-in `FromBody` impls: `String`, `Vec<u8>`, `Bytes`, `serde_json::Value`.

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

### RouteBuilder shorthand

```rust
RouteBuilder::from("direct:input")
    .route_id("shorthand")
    .dead_letter_channel("log:dlc")
    .on_exception(|e| matches!(e, CamelError::Io(_)))
        .retry(3)
        .handled_by("log:io-errors")
    .end_on_exception()
    .to("mock:result")
    .build()?;
```

### YAML Configuration

```yaml
routes:
  - id: "retry-example"
    from: "timer:tick"
    error_handler:
      dead_letter_channel: "log:dlc"
      retry:
        max_attempts: 3
        initial_delay_ms: 100
        multiplier: 2.0
        max_delay_ms: 10000
        jitter_factor: 0.2
      on_exceptions:
        - kind: "ProcessorError"
          message_contains: "validation"
          retry:
            max_attempts: 1
    steps:
      - to: "direct:processor"
```

### Exception Disposition — Propagate, Handled, Continued

Every `on_exception` clause has a **disposition** that controls what happens after the error handler runs:

| Disposition | Behavior                                                                                                    |
| ----------- | ----------------------------------------------------------------------------------------------------------- |
| `Propagate` | Error is re-thrown to upstream after DLC/handler runs (default)                                             |
| `Handled`   | Error is absorbed as `Ok(Exchange)`. Pipeline stops — subsequent steps do NOT run                           |
| `Continued` | Error is cleared from the Exchange. Pipeline **continues** to the next step — subsequent steps run normally |

In the builder API, use `ErrorHandlerConfig` with `.continued(true)` or `.handled(true)`:

```rust
use camel_api::error_handler::ErrorHandlerConfig;

let eh = ErrorHandlerConfig::dead_letter_channel("log:dlc")
    .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
    .continued(true)    // ← clear error, pipeline continues
    .retry(1)
    .build();

let route = RouteBuilder::from("direct:input")
    .route_id("continued-example")
    .error_handler(eh)
    .to("log:result")       // ← runs even after the error
    .build()?;
```

In YAML, use the `continued: true` field:

```yaml
error_handler:
  dead_letter_channel: "log:errors"
  on_exceptions:
    - kind: ProcessorError
      continued: true
```

The `continued` and `handled` fields are mutually exclusive — setting both to `true` is a compile error.

See `examples/error-handling` for complete examples, including Route 10 which demonstrates `continued=true`.

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
allow_internal = false

[default.components.kafka]
brokers = "localhost:9092"
group_id = "camel"

[default.components.redis]
host = "localhost"
port = 6379

[default.components.opensearch]
host = "localhost"
port = 9200

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

- **`[components.http]`**: `connect_timeout_ms`, `response_timeout_ms`, `max_connections`, `max_body_size`, `max_request_body`, `allow_internal`
- **`[components.kafka]`**: `brokers`, `group_id`, `session_timeout_ms`, `request_timeout_ms`, `auto_offset_reset`, `security_protocol`, `partition_assignment_strategy`. Named clusters under `[components.kafka.brokers_named.<name>]` each with `brokers`, optional `security_protocol`, `sasl_auth_type`, `client_id`, and `rdkafka_config` escape hatch. Reference via `?brokerName=<name>` in the endpoint URI.
- **`[components.redis]`**: `host`, `port`
- **`[components.sql]`**: `max_connections`, `min_connections`, `idle_timeout_secs`, `max_lifetime_secs`, `ssl_mode`, `ssl_root_cert`, `ssl_cert`, `ssl_key`
- **`[components.jms]`**: `default_broker`, `max_bridges`, `bridge_cache_dir`, `bridge_start_timeout_ms`, `broker_reconnect_interval_ms`. Brokers are declared as named entries under `[components.jms.brokers.<name>]`, each with `broker_url`, `broker_type` (`activemq`|`artemis`), and optional `username`/`password`. URI schemes `activemq:` and `artemis:` lock the broker type automatically and support shorthand destinations (e.g. `activemq:orders` → queue). Use the `broker=<name>` URI query param to select a specific broker from the pool.
- **`[components.file]`**: `delay_ms`, `initial_delay_ms`, `read_timeout_ms`, `write_timeout_ms`
- **`[components.container]`**: `docker_host`
- **`[components.ws]`**: `max_connections`, `max_message_size`, `heartbeat_interval_ms`, `idle_timeout_ms`

Each optional component (http, ws, kafka, redis, sql, jms, file, container) implements `ComponentBundle` — it owns its config key, deserializes its own TOML block, and registers one or more schemes. See `examples/custom-component-bundle` for a full walkthrough of implementing your own bundle.

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
let mut ctx = CamelContext::builder().build().await?;
ctx.register_component(TimerComponent::new());
ctx.register_component(LogComponent::new());

// Discover and load routes from config patterns
let routes = discover_routes(&config.routes)
    .map_err(|e| CamelError::Config(e.to_string()))?;
for route in routes {
    ctx.add_route_definition(route).await?;
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
allow_internal = false

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

## JSON Schema

The full DSL AST has a published JSON Schema:

- **URL:** `https://raw.githubusercontent.com/kennycallado/rust-camel/main/schemas/dsl/route-schema.json`
- **Local:** `schemas/dsl/route-schema.json` (regenerate with `cargo xtask schema`).

### Using `$schema`

Add the `$schema` key to your JSON route files for editor autocomplete and SDK validation:

```json
{
  "$schema": "https://raw.githubusercontent.com/kennycallado/rust-camel/main/schemas/dsl/route-schema.json",
  "routes": [...]
}
```

The parser silently ignores the key. See [`schemas/dsl/README.md`](./schemas/dsl/README.md) for scope and versioning.

## License

Apache-2.0
