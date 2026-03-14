# camel-otel

OpenTelemetry integration for rust-camel.

## Overview

Implements `Lifecycle` and `MetricsCollector` traits to export **traces, metrics, and logs** to any OTLP-compatible backend (e.g., Grafana LGTM, Jaeger, Tempo, Prometheus via OTLP).

`OtelService` initializes the global `TracerProvider`, `MeterProvider`, and `LoggerProvider` on start and shuts them down gracefully on stop. W3C `traceparent` / `tracestate` headers are propagated automatically in HTTP and Kafka components via optional feature flags.

**Features:**
- **Traces**: Distributed tracing via OTLP span exporter
- **Metrics**: Route-level metrics (duration, exchanges, errors) automatically recorded
- **Logs**: Log bridge exports `tracing` logs via OTLP
- **Auto-registration**: `OtelService.as_metrics_collector()` enables automatic metrics collection

## Quick Start

```rust
use camel_otel::{OtelConfig, OtelService};
use camel_core::context::CamelContext;

// OTel providers start/stop automatically with context
let ctx = CamelContext::new()
    .with_lifecycle(OtelService::new(
        OtelConfig::new("http://localhost:4317", "my-service"),
    ));

ctx.start().await?;
// Traces, metrics, and logs exported to OTLP backend

ctx.stop().await?;
// All providers flushed and shut down
```

### Default Configuration

```rust
use camel_otel::OtelService;

// Connects to http://localhost:4317, service name "rust-camel"
let ctx = CamelContext::new()
    .with_lifecycle(OtelService::with_defaults());
```

### Custom Sampling and Resource Attributes

```rust
use camel_otel::{OtelConfig, OtelProtocol, OtelSampler, OtelService};

let config = OtelConfig::new("http://otel-collector:4317", "payments-service")
    .with_protocol(OtelProtocol::Grpc)
    .with_sampler(OtelSampler::TraceIdRatioBased(0.1))
    .with_resource_attr("deployment.environment", "production")
    .with_resource_attr("service.version", "2.3.1");

let ctx = CamelContext::new().with_lifecycle(OtelService::new(config));
```

## OtelService

`OtelService` implements the `Lifecycle` trait, which follows Apache Camel's Service pattern:

- Initializes global `TracerProvider`, `MeterProvider`, and `LoggerProvider` when `CamelContext.start()` is called
- Installs `OpenTelemetryTracingBridge` to export logs via OTLP
- Exposes `as_metrics_collector()` for automatic metrics registration
- Flushes and shuts down all providers when `CamelContext.stop()` is called
- Exposes `status()` reflecting `Stopped` / `Started` / `Failed` states

## Metrics Exported

Route-level metrics are recorded automatically by `TracingProcessor`:

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `camel.exchanges.total` | Counter | `route.id` | Total exchanges processed |
| `camel.errors.total` | Counter | `route.id`, `error.type` | Total errors |
| `camel.exchange.duration.seconds` | Histogram | `route.id` | Exchange processing duration |
| `camel.queue.depth` | UpDownCounter | `route.id` | Current queue depth |
| `camel.circuit.breaker.state` | UpDownCounter | `route.id` | Circuit breaker state (0=closed, 1=open, 2=half_open) |

## Log Bridge

The `OpenTelemetryTracingBridge` is installed automatically when `OtelService` starts. All `tracing` logs are exported via OTLP to your backend.

```rust
// These logs are automatically exported via OTLP
tracing::info!("Processing exchange");
tracing::warn!(route_id = "my-route", "Something unusual happened");
```

## W3C Propagation

W3C `traceparent` and `tracestate` headers are propagated automatically when the `otel` feature is enabled on the HTTP or Kafka components.

```toml
[dependencies]
camel-component-http = { workspace = true, features = ["otel"] }
camel-component-kafka = { workspace = true, features = ["otel"] }
```

- **HTTP**: `traceparent` is injected into outgoing request headers and extracted from incoming requests
- **Kafka**: `traceparent` is injected into message headers and extracted on consume

## Architecture

```
OtelService (Lifecycle trait)
    ↓
    ├── TracerProvider (OTLP span exporter, batch)
    ├── MeterProvider (OTLP metric exporter, 60s periodic reader)
    ├── LoggerProvider (OTLP log exporter, batch)
    │       ↓
    │   OpenTelemetryTracingBridge (log → OTLP)
    ├── OtelMetrics (implements MetricsCollector)
    │       ↓
    │   as_metrics_collector() → auto-registered with CamelContext
    └── propagation.rs (W3C inject/extract used by camel-http and camel-kafka)
```

**Note:** Uses `Lifecycle` trait (not `Service`) to avoid confusion with `tower::Service`.

## Hot-reload Compatibility

**OTel configuration is NOT hot-reloadable.** Changes to endpoint, service name, or sampler require process restart. This is standard practice in OpenTelemetry implementations.

Route hot-reload via `ArcSwap` continues to work independently.

## Local Dev Backend

```bash
docker run -p 3000:3000 -p 4317:4317 -p 4318:4318 grafana/otel-lgtm
```

Then open `http://localhost:3000` to view traces, metrics, and logs in Grafana.

Run the bundled demo:

```bash
cargo run -p otel-demo
```

## Documentation

- [API docs (docs.rs)](https://docs.rs/camel-otel)
- [OpenTelemetry Rust](https://github.com/open-telemetry/opentelemetry-rust)

## License

MIT — see [LICENSE](../../LICENSE) for details.

## Contributing

Contributions welcome. Please open an issue or pull request on [GitHub](https://github.com/rust-camel/rust-camel).
