# camel-prometheus

Prometheus metrics integration for rust-camel.

## Overview

Implements `MetricsCollector` trait to export rust-camel metrics in Prometheus format.

**New in 0.3.0:** `PrometheusService` with `Lifecycle` trait for automatic server management.

## Quick Start

### Simple API (Recommended)

```rust
use camel_prometheus::PrometheusService;
use camel_core::context::CamelContext;

// Prometheus server starts/stops automatically with context
let ctx = CamelContext::new()
    .with_lifecycle(PrometheusService::new(9090))
    .with_tracing();

ctx.start().await?;
// Server running on http://0.0.0.0:9090/metrics

ctx.stop().await?;
// Server stopped automatically
```

### Manual Setup (Backward Compatible)

```rust
use camel_prometheus::{PrometheusMetrics, MetricsServer};
use camel_core::context::CamelContext;
use std::sync::Arc;
use std::net::SocketAddr;

let prometheus = Arc::new(PrometheusMetrics::new());
let ctx = CamelContext::with_metrics(Arc::clone(&prometheus));

// Start server manually
let addr: SocketAddr = "0.0.0.0:9090".parse().unwrap();
tokio::spawn(async move {
    MetricsServer::run(addr, prometheus).await;
});
```

## PrometheusService

`PrometheusService` implements the `Lifecycle` trait, which follows Apache Camel's Service pattern:

- Automatically starts HTTP server when `CamelContext.start()` is called
- Automatically stops when `CamelContext.stop()` is called
- Auto-registers `PrometheusMetrics` as `MetricsCollector`

## Metrics Exposed

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `camel_exchanges_total` | Counter | `route` | Total exchanges processed |
| `camel_errors_total` | Counter | `route`, `error_type` | Total errors |
| `camel_exchange_duration_seconds` | Histogram | `route` | Exchange processing duration |
| `camel_queue_depth` | Gauge | `route` | Current queue depth |
| `camel_circuit_breaker_state` | Gauge | `route` | Circuit breaker state (0=closed, 1=open, 2=half_open) |

## Endpoint

```
GET /metrics
```

Returns Prometheus text format metrics.

## Architecture

```
PrometheusService (Lifecycle trait)
    ↓
    ├── Manages HTTP server lifecycle (start/stop)
    ├── Auto-registers MetricsCollector
    └── MetricsServer (uses tower::Service internally via axum)
```

**Note:** Uses `Lifecycle` trait (not `Service`) to avoid confusion with `tower::Service`.

## Health Endpoints

Health endpoints are provided by the `camel-health` crate via `health_router()`. The `PrometheusService` accepts an optional `HealthChecker` and merges the health router into its HTTP server.

| Endpoint | Purpose | HTTP Status |
|----------|---------|-------------|
| `/healthz` | Liveness probe | 200 (always) |
| `/readyz` | Readiness probe | 200 if healthy, 503 if unhealthy |
| `/health` | Detailed health | 200 (always) |

### Kubernetes Integration

```yaml
livenessProbe:
  httpGet:
    path: /healthz
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 10

readinessProbe:
  httpGet:
    path: /readyz
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 5
```

### Health Report Format

```json
{
  "status": "Healthy",
  "services": [
    {
      "name": "prometheus",
      "status": "Started"
    }
  ]
}
```
