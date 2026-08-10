# Tracing

rust-camel integrates with OpenTelemetry for distributed tracing,
metrics, and log export. `OtelService` owns the lifecycle of the
global OTel providers.

## OtelService

`OtelService` is a `Lifecycle` implementation. It installs and owns
the process-global OpenTelemetry tracer, meter, and logger providers.
Keep one active instance per process.

Configure the OTLP endpoint and service identity with `OtelConfig`:

```rust,ignore
{{#include ../../../examples/otel-demo/src/main.rs:otel-config}}
```

> **Note:** Service registration is Rust API only. YAML routes compile to
> the same `RouteDefinition`. The service wiring stays in application
> code.

Create the service and register it with the context:

```rust,ignore
{{#include ../../../examples/otel-demo/src/main.rs:otel-service-setup}}
```

> **Note:** Service registration is Rust API only. YAML routes compile to
> the same `RouteDefinition`. The service wiring stays in application
> code.

`with_lifecycle()` auto-registers the metrics collector from
`OtelService::as_metrics_collector()`. The runtime records route-level
metrics automatically.

### Operational invariants

- **One global provider set.** `OtelService::start()` installs providers through `opentelemetry::global`. OpenTelemetry global setters replace the active provider and provide no reset API. Starting a second service can detach the first from global lookups. Its exporter tasks stay alive until you stop it. Keep one active `OtelService` per process.
- **Start before metric use.** `OtelMetrics` resolves its `Meter` on first use and caches it. Recording before `OtelService::start()` binds that instance to the no-op provider. Later startup does not replace the cached meter. Start the service before you register or record metrics.

See [camel-otel CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/services/camel-otel/CONTEXT.md) for the full operational invariants.

## Propagation helpers

The crate provides helpers that bridge W3C trace context between transport headers and an `Exchange`:

| Helper | Action |
|--------|--------|
| `extract_context` | extract trace context from transport headers |
| `inject_context` | inject trace context into transport headers |
| `extract_into_exchange` | extract trace context into an Exchange |
| `inject_from_exchange` | inject trace context from an Exchange |

## Related decisions

- [ADR-0007](../adr/0007-route-supervised-consumer-failure.md) motivates graceful provider shutdown. OpenTelemetry global ownership makes shutdown order operationally significant.
- [ADR-0012](../adr/0012-log-level-convention-handler-contract-boundaries.md) defines the log-policy classification for service start failures.

**Reference**: [camel-otel crate](https://github.com/kennycallado/rust-camel/blob/main/crates/services/camel-otel/CONTEXT.md)
