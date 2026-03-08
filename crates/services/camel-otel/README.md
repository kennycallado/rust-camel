# camel-otel

OpenTelemetry integration for rust-camel.

## Overview

Implements `Lifecycle` and `MetricsCollector` traits to export traces, metrics, and logs to any OTLP-compatible backend (e.g., Grafana LGTM, Jaeger, Tempo, Prometheus via OTLP).

`OtelService` initializes the global `TracerProvider` and `MeterProvider` on start and shuts them down gracefully on stop. W3C `traceparent` / `tracestate` headers are propagated automatically in HTTP and Kafka components via optional feature flags.

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
// Traces and metrics exported to OTLP backend

ctx.stop().await?;
// Providers flushed and shut down
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

- Initializes global `TracerProvider` and `MeterProvider` when `CamelContext.start()` is called
- Flushes and shuts down providers when `CamelContext.stop()` is called
- Exposes `status()` reflecting `Stopped` / `Started` / `Failed` states

## Metrics Exported

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `camel.exchanges.total` | Counter | `route.id` | Total exchanges processed |
| `camel.errors.total` | Counter | `route.id`, `error.type` | Total errors |
| `camel.exchange.duration.seconds` | Histogram | `route.id` | Exchange processing duration |
| `camel.queue.depth` | UpDownCounter | `route.id` | Current queue depth |
| `camel.circuit.breaker.state` | UpDownCounter | `route.id` | Circuit breaker state (0=closed, 1=open, 2=half_open) |

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
    ├── Initializes global TracerProvider (OTLP span exporter, batch)
    ├── Initializes global MeterProvider (OTLP metric exporter, 60s periodic reader)
    └── propagation.rs (W3C inject/extract used by camel-http and camel-kafka)
```

**Note:** Uses `Lifecycle` trait (not `Service`) to avoid confusion with `tower::Service`.

## Local Dev Backend

```bash
docker run -p 3000:3000 -p 4317:4317 -p 4318:4318 grafana/otel-lgtm
```

Then open `http://localhost:3000` to view traces and metrics in Grafana.

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
