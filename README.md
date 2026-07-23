# rust-camel

[![CI](https://github.com/kennycallado/rust-camel/actions/workflows/ci.yml/badge.svg)](https://github.com/kennycallado/rust-camel/actions/workflows/ci.yml)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://www.apache.org/licenses/LICENSE-2.0)
[![Rust](https://img.shields.io/badge/rust-1.89%2B-orange.svg)](https://www.rust-lang.org)
[![Status](https://img.shields.io/badge/status-pre--release-orange.svg)](#status--roadmap)

Build integration routes in Rust or YAML using Tower-native pipelines.

**rust-camel** connects timers, HTTP APIs, files, Kafka, Redis, SQL, and other
systems through composable message routes. Use it when you need Apache
[Camel](https://camel.apache.org/)-style routing and Enterprise Integration
Patterns (EIP) without a JVM — every processor and producer is a
`Service<Exchange>`, so EIP patterns compose as Tower middleware.

> **Status:** Pre-release. APIs may change.
> Requires **Rust 1.89** or newer (stable).

---

## Table of Contents

- [What is rust-camel?](#what-is-rust-camel)
- [Quick Start](#quick-start)
- [Core Concepts](#core-concepts)
- [Components & EIP Patterns](#components--eip-patterns)
- [REST DSL & OpenAPI](#rest-dsl--openapi)
- [Configuration](#configuration)
- [Error Handling](#error-handling)
- [Observability](#observability)
- [Security](#security)
- [Docker](#docker)
- [Examples](#examples)
- [Architecture](#architecture)
- [Building & Testing](#building--testing)
- [Status & Roadmap](#status--roadmap)
- [Contributing](#contributing)
- [License](#license)

## What is rust-camel?

A Rust-native integration framework that routes messages between components
using a fluent builder API or a declarative YAML DSL. The **data plane**
(processing, EIP patterns, middleware) is Tower-native; the **control plane**
(components, endpoints, consumers, route lifecycle, supervision) uses its own
trait hierarchy.

### Who is it for?

- **Integration, platform, and DevOps engineers** — design and run routes with
  `camel-cli` and YAML, with configuration profiles, health endpoints,
  Prometheus/OpenTelemetry, and container images — often **without writing
  Rust**.
- **Backend, data, and API teams** — connect APIs, brokers, databases, files,
  and event streams (HTTP, WebSocket, Kafka, MQTT, JMS, gRPC, SOAP, SQL, XML,
  LLMs) using reusable routing and transformation patterns, and expose them
  through the REST DSL with schema validation and OpenAPI generation.
- **Rust and Tower developers** — embed routes with `RouteBuilder`, add custom
  processors or components, and compose pipelines as Tower `Service<Exchange>`
  services.
- **Apache Camel users and JVM migration teams** — evaluate Camel-inspired
  routing in native Rust, with single-binary deployment and scratch
  `amd64`/`arm64` images, without assuming drop-in compatibility.

From glue scripts to AI pipelines, edge deployments to legacy-system bridges —
if it involves routing messages between systems, rust-camel is built for the
shape of the problem.

### Why rust-camel?

- **Composable by construction** — EIP patterns are Tower layers; chain them
  the same way you chain `tower::Service`.
- **Single binary, scratch containers** — ship routes as one Rust binary or a
  minimal Docker image.
- **Declarative or code** — define routes in YAML with JSON-Schema validation
  and editor autocomplete, or in pure Rust.

### When not to use it

- You need a **stable, production-hardened** integration runtime — rust-camel
  is pre-release; APIs will change.
- You already live on the **JVM** and depend on Apache Camel's ecosystem.
- Your workload is pure request/response **without** routing, EIP patterns, or
  multi-system orchestration (a plain web framework is simpler).

## Quick Start

### Run an example

```bash
git clone https://github.com/kennycallado/rust-camel.git
cd rust-camel
cargo run -p hello-world
```

A route receives an `Exchange` from `timer:tick` and sends it to `log:info`.

### Your first route (Rust)

```rust
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulation};
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
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
```

### Your first route (YAML)

```yaml
# routes/hello.yaml
routes:
  - id: "hello-timer"
    from: "timer:tick?period=1000"
    steps:
      - to: "log:info"
```

Save it as `routes/hello.yaml` and run with `camel run`.

### Use the CLI

```bash
cargo install camel-cli
camel new my-integration   # scaffolds Camel.toml + routes/hello.yaml
cd my-integration
camel run
```

See [`crates/camel-cli/README.md`](crates/camel-cli/README.md) for all commands.

## Core Concepts

- **Route** — a pipeline from a source (`from:`) through a sequence of steps
  (`to:`, processors, EIP patterns) to one or more sinks.
- **Exchange** — the message envelope flowing through a route: headers, body,
  and a unique correlation id for tracing.
- **Component** — a connector (`timer`, `http`, `kafka`, ...) that creates
  **Endpoints** from URIs like `timer:tick?period=1000`.
- **EIP pattern** — a routing/transformation step (filter, split, choice,
  aggregate, ...) applied as Tower middleware on the data plane.

## Components & EIP Patterns

**Components:** `timer`, `cron`, `log`, `direct`, `seda`, `file`, `http`,
`ws`/`wss`, `kafka`, `mqtt`, `redis`, `sql`, `surrealdb`, `opensearch`, `jms`,
`grpc`, `cxf` (SOAP), `xslt`, `xj` (XML↔JSON), `validator`, `container`
(Docker), `controlbus`, `master`, `exec`, `keycloak`, `wasm`, `llm`. See
[`crates/components/`](crates/components/) for the full set of
implementations.

**Enterprise Integration Patterns** (all chainable as Tower layers):

| Pattern                    | Builder method                                |
| -------------------------- | --------------------------------------------- |
| Content-Based Router       | `.choice()` / `.when()`                       |
| Filter                     | `.filter(predicate)`                          |
| Splitter (incl. streaming) | `.split(config)`                              |
| Aggregator                 | `.aggregate(config)`                          |
| Multicast                  | `.multicast()`                                |
| Recipient List             | `.recipient_list(config)`                     |
| Routing Slip               | `.routing_slip(expr)`                         |
| Dynamic Router             | `.dynamic_router(expr)`                       |
| Wire Tap                   | `.wire_tap(uri)`                              |
| Load Balancer              | `.load_balance()`                             |
| Throttler                  | `.throttle(n, duration)`                      |
| Delayer                    | `.delay()`                                    |
| Loop                       | `.loop_count(n)`                              |
| Content Enricher           | `.enrich(uri)` / `.poll_enrich(uri, timeout)` |
| Marshal / Unmarshal        | `.marshal(fmt)` / `.unmarshal(fmt)`           |
| Stream Cache               | `.stream_cache(n)`                            |
| try / catch / finally      | `.do_try()` / `.do_catch()` / `.do_finally()` |

Marshal formats: JSON, XML, CSV, ZIP.

## REST DSL & OpenAPI

Define REST APIs declaratively in YAML with automatic JSON binding, path
templates, schema validation, and OpenAPI generation.

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
            name: { type: string }
            email: { type: string }
          required: [name, email]
```

The `rest:` block lowers to `http:` consumer routes with JSON unmarshalling,
schema validation (→ 400 on failure), and default status codes.

Generate an OpenAPI 3.0.3 document:

```bash
camel openapi generate routes.yaml --title "Users API" --version "1.0.0"
```

See [`examples/rest-crud/`](examples/rest-crud/) for a complete runnable CRUD
example with in-memory storage and static file co-hosting.

## Configuration

External configuration via `Camel.toml` (loaded by the `camel-config` crate):

```toml
[default]
routes = ["routes/**/*.yaml"]
log_level = "INFO"

# Component defaults — apply to all endpoints; URI params always override these
[default.components.http]
connect_timeout_ms = 5000
allow_internal = false

[default.components.kafka]
brokers = "localhost:9092"
group_id = "camel"

[default.components.redis]
host = "localhost"
port = 6379

[default.observability.prometheus]
enabled = true
port = 9090

[production]
log_level = "ERROR"

[production.components.kafka]
brokers = "prod-kafka:9092"
```

### Profiles, discovery, and overrides

- **Profiles** — multiple environments (`[default]`, `[production]`, ...) in
  one file, selected with `CAMEL_PROFILE=production`.
- **Route discovery** — `routes = ["routes/**/*.yaml"]` loads routes via glob.
- **Environment overrides** — any value via `CAMEL_*` env vars
  (`CAMEL_LOG_LEVEL=DEBUG`, `CAMEL_ROUTES_0="custom/*.yaml"`).
- **Deep merge** — nested configs merge; URI parameters always win.

Optional components (http, ws, kafka, redis, sql, jms, file, container)
implement `ComponentBundle`, owning their config key and schemes. See
[`examples/custom-component-bundle/`](examples/custom-component-bundle/) to
build your own.

## Error Handling

Retry with exponential backoff and jitter, plus a dead-letter channel:

```rust
use camel_api::error_handler::ErrorHandlerConfig;
use std::time::Duration;

let error_handler = ErrorHandlerConfig::dead_letter_channel("log:errors")
    .on_exception(|e| matches!(e, CamelError::Io(_)))
    .retry(3)
    .with_backoff(
        Duration::from_millis(100), // initial delay
        2.0,                        // multiplier
        Duration::from_secs(10),    // max delay
    )
    .with_jitter(0.2) // ±20% (recommended 0.1–0.3)
    .build();
```

The same route in YAML:

```yaml
# routes/retry.yaml
routes:
  - id: "retry-example"
    from: "direct:input"
    error_handler:
      dead_letter_channel: "log:errors"
      on_exceptions:
        - kind: "Io"
          retry:
            max_attempts: 3
            initial_delay_ms: 100
            multiplier: 2.0
            max_delay_ms: 10000
            jitter_factor: 0.2
    steps:
      - to: "log:info"
```

During retries these headers are set automatically: `CamelRedelivered`,
`CamelRedeliveryCounter`, `CamelRedeliveryMaxCounter`.

### Exception disposition

Each handler controls what happens after it runs:

| Disposition | Behavior                                             |
| ----------- | ---------------------------------------------------- |
| `Propagate` | Re-throw upstream after the handler runs (default)   |
| `Handled`   | Absorb as `Ok(Exchange)`; pipeline stops             |
| `Continued` | Clear the error; pipeline continues to the next step |

Configure the same in YAML via `error_handler:` with `retry:`, `jitter_factor:`,
`on_exceptions:`, and `continued: true` / `handled: true` (mutually exclusive).

See [`examples/error-handling/`](examples/error-handling/) for complete
examples.

## Observability

- **Correlation IDs** — every exchange carries a unique `correlation_id`.
- **Prometheus** — export via `PrometheusService` (implements the `Lifecycle`
  trait); metrics include `camel_exchanges_total`, `camel_errors_total`,
  `camel_exchange_duration_seconds`, `camel_queue_depth`,
  `camel_circuit_breaker_state`.
- **OpenTelemetry** — tracing and metrics export via `camel-otel`.
- **Health endpoints** — `/healthz` (liveness), `/readyz` (readiness),
  `/health` (detailed report), ready for Kubernetes probes.

```rust
use std::net::SocketAddr;
use camel_prometheus::PrometheusService;
use camel_core::context::CamelContext;

let prometheus = PrometheusService::new(SocketAddr::from(([0, 0, 0, 0], 9090)));

let mut ctx = CamelContext::builder()
    .build()
    .await?
    .with_lifecycle(prometheus)
    .with_tracing()
    .await;
ctx.start().await?;
// Prometheus server + /healthz, /readyz start automatically
```

## Security

rust-camel ships built-in security safeguards:

- **SSRF protection (HTTP)** — block private IPs by default
  (`allowInternal=false`) and custom blocklists (`blockedHosts=...`).
- **Path traversal protection (File)** — resolved paths are validated against
  the configured base directory; `../` and out-of-base absolute paths are
  rejected.
- **Timeouts** — configurable on file and HTTP I/O (file `readTimeout`/`writeTimeout`,
  HTTP `connectTimeout`/`responseTimeout`; defaults 30s).
- **Memory limits** — the aggregator caps `max_buckets` and `bucket_ttl`.

See [`SECURITY.md`](SECURITY.md) to report vulnerabilities.

## Docker

Pre-built images on [GHCR](https://github.com/kennycallado/rust-camel/pkgs/container/rust-camel)
and [Docker Hub](https://hub.docker.com/r/kennycallado/rust-camel). Both
variants support `linux/amd64` and `linux/arm64`.

| Tag suffix | Base        | Use case                                    |
| ---------- | ----------- | ------------------------------------------- |
| _(none)_   | scratch     | Minimal runtime image, small attack surface |
| `-alpine`  | alpine:3.21 | Debugging — includes a busybox shell        |

```bash
# Production
docker run -v $(pwd)/routes:/app/routes \
  ghcr.io/kennycallado/rust-camel:latest run

# Debugging shell
docker run -it -v $(pwd):/app \
  ghcr.io/kennycallado/rust-camel:latest-alpine sh
```

## Examples

The [`examples/`](examples/) directory contains 90+ runnable examples. A few
good starting points:

| Example                                                        | Shows                                  |
| -------------------------------------------------------------- | -------------------------------------- |
| [`hello-world`](examples/hello-world/)                         | Minimal `timer → log` route            |
| [`rest-crud`](examples/rest-crud/)                             | REST DSL + OpenAPI + static co-hosting |
| [`http-server`](examples/http-server/)                         | HTTP consumer with streaming           |
| [`kafka-example`](examples/kafka-example/)                     | Kafka producer/consumer                |
| [`error-handling`](examples/error-handling/)                   | Retries, dead-letter, dispositions     |
| [`custom-component-bundle`](examples/custom-component-bundle/) | Build your own component               |
| [`bean-demo`](examples/bean-demo/)                             | Bean registry / dependency injection   |

Run any of them with `cargo run -p <name>`.

## Architecture

rust-camel separates two planes:

- **Data plane** — Tower-native. Every processor and producer is a
  `Service<Exchange>`; EIP patterns compose as Tower middleware.
- **Control plane** — a trait hierarchy (`Component`, `Endpoint`, `Consumer`)
  for route lifecycle, supervision, and hot-reload.

```
┌──────────────────────────────────────────────────────┐
│                    Your Application                  │
│         RouteBuilder / YAML DSL / Camel.toml         │
└──────────────────────┬───────────────────────────────┘
                       │
┌──────────────────────▼───────────────────────────────┐
│                 camel-core                           │
│         CamelContext — composition root              │
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
│  └──────────────────────────────────────────────┘    │
└──────────┬──────────────────────────┬────────────────┘
           │                          │
┌──────────▼─────────┐    ┌───────────▼────────────────┐
│   camel-processor  │    │   Components               │
│   EIP patterns     │    │   timer, log, http, file,  │
│   (Tower layers)   │    │   kafka, redis, sql, ...,  │
└────────────────────┘    └────────────────────────────┘
```

Internals (domain model, event journaling, runtime state) are an
implementation detail — see [`crates/camel-core/README.md`](crates/camel-core/README.md)
and [`docs/ARCHITECT.md`](docs/ARCHITECT.md).

### JSON Schema for the DSL

The full YAML/JSON route DSL has a published JSON Schema for editor
autocomplete and SDK validation:

- **URL:** `https://raw.githubusercontent.com/kennycallado/rust-camel/main/schemas/dsl/route-schema.json`
- **Local:** `schemas/dsl/route-schema.json` (regenerate with `cargo xtask schema`)

```json
{
  "$schema": "https://raw.githubusercontent.com/kennycallado/rust-camel/main/schemas/dsl/route-schema.json",
  "routes": []
}
```

## Building & Testing

```sh
cargo build --workspace
cargo test  --workspace
cargo bench --workspace     # Criterion reports; camel-bench covers pipelines
scripts/coverage.sh         # needs cargo-llvm-cov; baseline enforced in coverage.toml
```

Local lint and schema tools: `cargo fmt --check`, `cargo clippy -- -D warnings`,
and the `cargo xtask` subcommands (`lint-unwrap`, `lint-secrets`,
`lint-log-levels`, `schema --check`). See [`AGENTS.md`](AGENTS.md) for the exact
CI gate set.

## Status & Roadmap

Pre-release — APIs will change. Active development covers
component coverage, the DSL surface, and observability.

## Contributing

Contributions are welcome. Read [`CONTRIBUTING.md`](CONTRIBUTING.md) for build
setup, coding standards, the commit convention, and the CI quality gates your
PR must pass. Open issues and pull requests at
[kennycallado/rust-camel](https://github.com/kennycallado/rust-camel).

## License

Apache-2.0.
