# camel-prometheus

Prometheus metrics integration for rust-camel.

## Overview

Implements `MetricsCollector` trait to export rust-camel metrics in Prometheus format.

## Quick Start

```rust
use camel_prometheus::{PrometheusMetrics, MetricsServer};
use camel_core::context::CamelContext;
use std::sync::Arc;
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    // Create Prometheus metrics collector
    let prometheus = Arc::new(PrometheusMetrics::new());
    
    // Configure context with metrics
    let ctx = CamelContext::with_metrics(Arc::clone(&prometheus) as Arc<dyn MetricsCollector>);
    
    // Start metrics server on port 9090
    let addr: SocketAddr = "0.0.0.0:9090".parse().unwrap();
    MetricsServer::run(addr, Arc::clone(&prometheus)).await;
}
```

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
