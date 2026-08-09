# Health

rust-camel exposes health endpoints through the `ObservabilityConfig.health`
block. When enabled, a HealthServer starts on the configured host and port. It
serves four HTTP endpoints:

- `/healthz`: liveness probe. Returns 200 while the runtime is alive.
- `/readyz`: readiness probe. Returns 200 when all routes are Healthy or Degraded. Returns 503 when any route is Unhealthy.
- `/startupz`: startup probe. Same response rules as `/readyz`.
- `/health`: detailed JSON report with per-route status. Always returns 200.

A probe handler that times out fails closed. The probe returns 503 with an
`Unhealthy` status.

## Configuration

The `ObservabilityConfig.health` field accepts an optional
`HealthCamelConfig`:

```rust,ignore
{{#include ../../../examples/health-demo/src/main.rs:health-config}}
```

> **Note:** Service registration is Rust API only. YAML routes compile to
> the same `RouteDefinition`. The service wiring stays in application
> code.

The `enabled` flag starts the HealthServer. `host` and `port` set the bind
address. `handler_timeout_ms` limits each probe handler. The default is 6
seconds (`DEFAULT_HANDLER_TIMEOUT`). `forced_ttl_ms` controls how long a
forced-unhealthy state persists before the registry re-evaluates the route.

## Route

A route registered with the context participates in health checks:

```rust,ignore
{{#include ../../../examples/health-demo/src/main.rs:health-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: health-demo-route
    from: timer:health?period=5000
    steps:
      - to: log:health?showBody=false
```

</details>

The HealthServer evaluates every registered route. A route whose probes return
Healthy contributes a healthy status. A route with a Degraded probe still passes
readiness (200 on `/readyz`). A route with an Unhealthy probe fails readiness
(503).

## Degraded vs Unhealthy

The health subsystem distinguishes two states below Healthy:

- **Degraded**: the component can still process Exchanges. The route passes readiness (200, pod Ready).
- **Unhealthy**: the component cannot process Exchanges. The route fails readiness (503, pod NotReady).

See the [glossary](../concepts/glossary.md) for the canonical definition of
these terms.

**Reference**: [Health crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-health/CONTEXT.md)
