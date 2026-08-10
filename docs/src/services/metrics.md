# Metrics

rust-camel records route and exchange metrics through the
`MetricsCollector` trait. `PrometheusMetrics` is the production
implementation. You can supply your own.

## Custom metrics collector

Implement `MetricsCollector` to receive route-level metrics. The
runtime calls your methods as exchanges flow, errors fire, and circuit
breakers change state. The trait covers exchange duration, error and
exchange counts, queue depth, and circuit-breaker transitions.

```rust,ignore
{{#include ../../../examples/metrics-demo/src/main.rs:metrics-collector-impl}}
```

Register the collector with `CamelContext::builder().metrics(...)`:

```rust,ignore
{{#include ../../../examples/metrics-demo/src/main.rs:metrics-context-builder}}
```

> **Note:** Service registration is Rust API only. YAML routes compile to
> the same `RouteDefinition`. The service wiring stays in application
> code.

<details>
<summary>YAML equivalent for the route</summary>

```yaml
routes:
  - id: metrics-demo
    from: timer:metrics?period=500&repeatCount=10
    steps:
      # The Rust route measures elapsed time and sets the body in an
      # inline process() closure. Move that logic into a bean step.
      - set_body:
          value:
            message: processed
      - to: log:output?showBody=true&showCorrelationId=true
```

</details>

**Reference**: [MetricsCollector contract](https://github.com/kennycallado/rust-camel/blob/main/crates/services/CONTEXT.md)

## Prometheus integration

`PrometheusService` is a `Lifecycle` implementation. It owns a
`PrometheusMetrics` collector and an HTTP server, exposes metrics at
`/metrics`, and merges the health routes from `camel-health`.

```rust,ignore
{{#include ../../../examples/prometheus-demo/src/main.rs:prometheus-service-setup}}
```

> **Note:** Service registration is Rust API only. YAML routes compile to
> the same `RouteDefinition`. The service wiring stays in application
> code.

The server exposes these endpoints:

| Endpoint | Purpose |
|----------|---------|
| `/metrics` | Prometheus scrape |
| `/healthz` | liveness probe (200 while alive) |
| `/readyz` | readiness probe (200 healthy, 503 not) |
| `/health` | detailed JSON health report |

### Exposure posture

Diagnostic endpoints follow the Prometheus scrape convention: unauthenticated by default, with TLS and auth as opt-in hooks. The default bind address prefers loopback. Binding to `0.0.0.0` emits a startup warning.

See [ADR-0052](../adr/0052-diagnostic-endpoint-exposure-posture.md) for the full posture.

### Cardinality contract

Metric label values must come from a closed or bounded set. Never pass raw Exchange body, header, property, or correlation-key data as a label value. Each distinct label combination creates a new Prometheus series. The registry has no cardinality cap or eviction.

See [ADR-0032](../adr/0032-exchange-data-trust-boundary.md) for the exchange-data trust boundary.

**Reference**: [camel-prometheus crate](https://github.com/kennycallado/rust-camel/blob/main/crates/services/camel-prometheus/CONTEXT.md)
