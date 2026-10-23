# Health

rust-camel exposes health endpoints through the `ObservabilityConfig.health`
block. When enabled, a HealthServer starts on the configured host and port. It
serves four HTTP endpoints. `/healthz`, `/readyz`, and `/startupz` are cluster
probes. `/health` is the diagnostic report endpoint.

- `/healthz`: liveness probe. Returns 200 while the runtime is alive.
- `/readyz`: readiness probe. Returns 200 when all routes are Healthy or Degraded. Returns 503 when any route is Unhealthy.
- `/startupz`: startup probe. Same response rules as `/readyz`.
- `/health`: detailed JSON report with per-service status. Always returns 200.

A probe handler that times out fails closed. The probe returns 503 with an
`Unhealthy` status.

## Probe bodies

Every response body is a `HealthReport` with `status`, `services`, and
`timestamp` fields. Each entry in `services` is a `ServiceHealth` object with
`name`, `status`, and `message` fields. Probe bodies use the same shape as the
`/health` report.

When the aggregate status is Degraded or Unhealthy, `services` names the
offending service or services and gives their messages. A failed or degraded
service never leaves `services` empty. The exception is a status-only source.
The trait-default report methods of such a source return an empty `services`
array for any aggregate status.

The Healthy shape passes through unchanged. A detail-capable source lists all
relevant services, even when the aggregate status is Healthy. `services` is
empty only when the evaluation produced no entries. An entry status (for
example `Stopped`) never changes the aggregate status on its own.

These endpoints are unauthenticated by convention (ADR-0052). Probe bodies
carry the same message content as `/health`, so they carry the same
redaction responsibility: components must not place credentials or hosts in
health check messages.

On the three probes, a handler timeout synthesizes one `probe-timeout` service
entry. Its message names the probe: `liveness`, `readiness`, or `startup`. An
ordinary failure never carries a `probe-timeout` entry. On `/health`, a timeout
returns an `Unhealthy` report with a `probe-timeout` entry, and the HTTP status
is still 200.

The status codes are unchanged: 200 for Healthy or Degraded, 503 for Unhealthy
on the three probes.

## Configuration

The `ObservabilityConfig.health` field accepts an optional
`HealthCamelConfig`:

```rust,ignore
{{#include ../../../examples/health-demo/src/main.rs:health-config}}
```

The `enabled` flag starts the HealthServer. `host` and `port` set the bind
address. The defaults are host `0.0.0.0` and port `8081`. On `camel run`,
`--health-port <PORT>` overrides the port and enables the standalone health
server. `handler_timeout_ms` limits each probe handler. The default is 6
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
  Lazily started resources (the xslt and jms bridges, the sql producer pool) report Degraded until first use, so a route without traffic still passes readiness.
- **Unhealthy**: the component cannot process Exchanges. The route fails readiness (503, pod NotReady).

For the JMS bridge, broker status follows the same split. A live bridge with unknown (not yet checked), skipped, or stale broker status is `Degraded` and passes readiness. A completed health RPC that reports the broker as verified down is `Unhealthy` 503. An RPC failure or timeout is also `Unhealthy` 503. Broker fields never trigger a process restart. They describe process liveness and broker reachability only.

The broker-latency isolation guarantee requires the updated Java health handler. No protobuf change is needed. An older blocking bridge cannot provide the guarantee.

See the [glossary](../concepts/glossary.md) for the canonical definition of
these terms.

**Reference**: [Health crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-health/CONTEXT.md)
