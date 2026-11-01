# Metrics

rust-camel records route and exchange metrics through the
`MetricsCollector` trait. `PrometheusMetrics` is the production
implementation. You can supply your own.

## Metric families

The Prometheus exporter adds the `camel_` prefix to every collector name.
The collector surface records the names without the prefix.

| Metric | Type | Labels | Values | Disableable |
|---|---|---|---|---|
| `camel_language_eval_failures_total` | counter | `language`, `route`, `verb`, `class` | `language` is the expression language; `route` is the route id; `verb` is the DSL step verb; `class` is the typed `ExpressionErrorClass` name (`runtime`, `arithmetic`, `type-mismatch`, `function-not-found`, `limit`, `timeout`, `conversion`, `parse`). | No |
| `camel_errors_total` | counter | `route`, `error_type` | `route` is the route id; `error_type` is the closed error class. | No |
| `camel_component_operations_total` | counter | `component`, `operation`, `outcome` | `component` is the component identity; `operation` is the component operation name; `outcome` is `success` or `failure`. | Yes (`components`, `component_overrides`; success/failure series only — errors are never gated) |

Label values come from operator-authored route configuration, from the
typed error class, and from closed code-owned vocabularies. Exchange
data is never a label value. The cause text, the conversion detail, the
position, the body, the headers, the properties, the request URL, and
the host are not label values.

`camel_language_eval_failures_total` increments once per top-level
evaluation failure that the route step loop surfaces. A failure that a
route error handler recovers still counts, because the evaluation failed.
Retries inside the handler do not increment the counter again. An
`ExpressionFailed` nested inside another error's cause chain is not
counted, and neither is one raised by the handler itself.

The language counter is not gated by a metric-family lever. It matches the
non-disableable error family.

## Component operations

Components opt in to the `camel_component_operations_total` family. The
family is off by default. Each component records a small, closed set of
operations:

| Component | Operations |
|---|---|
| kafka | `consume`, `produce` |
| redis | `command` (producer only) |
| http | `request` (producer), `consume` (consumer) |

Component-specific pages list the operations of other components. A
failure also increments `camel_errors_total` as
`e:{component}:{operation}`. The error family is never gated, so a
switched-off component still reports failures.

### HTTP consumer completion

The HTTP consumer records `http:consume` once per handled request on a
`from: http:` route. The outcome is decided at response completion or
abort, not when the exchange enters the route channel:

- The outcome is `success` when the reply is not classified as failed
  and the response body is exhausted under the HTTP body contract. A
  body that is already end-of-stream, or a response whose body the
  protocol suppresses (`HEAD`, `204`, `304`, `1xx`), counts as exhausted.
- The outcome is `failure` when the consumer classifies the reply as
  failed (pipeline error, authentication denial, cancellation reply,
  unconsumable response body), the body returns an error, the body is
  dropped before exhaustion, the reply headers cannot build a valid
  response, the dispatcher replaces an oversized response, or the
  request never completes a route round-trip (request body over the
  limit, in-flight limit reached, closed route or reply channel, handler
  cancellation including the dispatch timeout).
- A classified failed reply keeps its failure through body
  finalization. Exhausting or dropping its error-rendering body does not
  turn it into a success.
- A status code selected by the route, including 4xx and 5xx, is not a
  failure by itself. The route completed and produced a well-formed
  response.
- A transport write error after the body is exhausted is outside this
  metric. Static mounts served by the `http-static` endpoint are outside
  it too.

### Per-component overrides

`[observability.metrics] components` sets the default for every
component. `[observability.metrics.component_overrides]` changes it for
named components:

```toml
[observability.metrics]
enabled = true
components = false

[observability.metrics.component_overrides]
http = true
kafka = false
```

The effective switch for a component is:

```text
enabled && component_overrides.get(component).unwrap_or(components)
```

An absent override inherits `components`. An explicit override wins over
`components` in both directions. `enabled = false` suppresses every
component series regardless of overrides. An override for a component
that is not registered has no effect. Override keys must be non-empty
ASCII identities matching `[a-z0-9][a-z0-9_-]*`; an invalid key fails
configuration load. Profiles and includes merge the table key by key, so
a profile overrides only the components it names.

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

> **Note:** YAML routes cannot declare services. Wire a custom collector
> through the Rust API. The `[observability.prometheus]` block configures
> the built-in Prometheus service.

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

> **Note:** YAML routes cannot declare services. Wire `PrometheusService`
> through the Rust API or the `[observability.prometheus]` block in
> `Camel.toml`.

The server exposes these endpoints:

| Endpoint | Purpose |
|----------|---------|
| `/metrics` | Prometheus scrape |
| `/healthz` | liveness probe (200 while alive) |
| `/readyz` | readiness probe (200 healthy, 503 not) |
| `/health` | detailed JSON health report |

### Exposure posture

Diagnostic endpoints follow the Prometheus scrape convention: unauthenticated by default, with TLS and auth as opt-in hooks. The service binds exactly the address the caller supplies; there is no loopback default. Binding to any non-loopback address emits a startup warning.

See [ADR-0052](../adr/0052-diagnostic-endpoint-exposure-posture.md) for the full posture.

### Cardinality contract

Metric label values must come from a closed or bounded set. Never pass raw Exchange body, header, property, or correlation-key data as a label value. Each distinct label combination creates a new Prometheus series. Dynamic collector names are capped at 1024 by default. `max_dynamic_collectors` raises or lowers the cap. Observations beyond the cap are dropped with a log. Label combinations per collector remain unbounded.

See [ADR-0032](../adr/0032-exchange-data-trust-boundary.md) for the exchange-data trust boundary.

**Reference**: [camel-prometheus crate](https://github.com/kennycallado/rust-camel/blob/main/crates/services/camel-prometheus/CONTEXT.md)
