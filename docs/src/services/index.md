# Services

Services are cross-cutting infrastructure that registers into the
CamelContext lifecycle. They observe, manage, and secure the runtime.
Components produce and consume Exchanges. Services do not.

## Lifecycle trait

The `Lifecycle` trait coordinates service start and stop with
CamelContext. Register with `with_lifecycle(...)`. Services start in
registration order, stop in reverse order, and report `Stopped`,
`Started`, or `Failed` through `Lifecycle::status()`.

## Available services

| Service | Crate | Description |
|---------|-------|-------------|
| [Prometheus](metrics.md) | `camel-prometheus` | Route metrics at `/metrics` plus health endpoints |
| [OpenTelemetry](tracing.md) | `camel-otel` | Traces, metrics, and logs over OTLP |
| [Auth](auth.md) | `camel-auth` | Token validation, claim mapping, permission checks |
| [Function runtime](function.md) | `camel-function` | Out-of-process user code invoked as `function:` pipeline steps |
| [Bridge](bridge.md) | `camel-bridge` | Spawns and supervises Java bridge processes for JVM-only components (JMS, XML, CXF) |

**Reference**: [Services crate](https://github.com/kennycallado/rust-camel/blob/main/crates/services/CONTEXT.md). See also [Health endpoints](../operations/health.md).
