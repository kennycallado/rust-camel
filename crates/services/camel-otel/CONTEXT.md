# camel-otel

OpenTelemetry service implementation. Manages lifecycle of OTel providers (TracerProvider, MeterProvider, LoggerProvider).

## Language

**OtelService**:
`Lifecycle` implementation that installs and owns the process-global OpenTelemetry
tracer, meter, and logger providers. One active instance is supported per process.
_Avoid_: OTel provider, telemetry service

**OtelMetrics**:
`MetricsCollector` implementation returned by `OtelService::as_metrics_collector()`.
It resolves the global `Meter` on first instrument use and caches the meter and
instruments in `OnceLock` values.
_Avoid_: metrics exporter, OTel collector

**OtelConfig**:
Builder-style configuration for the OTLP endpoint, service identity, protocol,
sampler, resource attributes, logs, and metrics interval. Configuration is not
hot-reloadable. A change requires a process restart.

**Propagation helpers**:
`extract_context`, `inject_context`, `extract_into_exchange`, and
`inject_from_exchange` bridge W3C trace context between transport headers and an
`Exchange`.
_Avoid_: tracing middleware, header copier

## Operational invariants

- **One global provider set.** `OtelService::start()` installs providers through
  `opentelemetry::global`. OpenTelemetry global setters replace the active
  provider and provide no reset API. Starting a second service can detach the
  first service from global lookups and leave its exporter tasks alive until it
  is stopped. This is an OpenTelemetry SDK constraint, not a workspace-wide
  rust-camel policy. Keep one active `OtelService` per process and stop it before
  replacement.
- **Start before metric use.** `OtelMetrics` resolves its `Meter` on first use
  and caches it. Recording before `OtelService::start()` installs the global
  provider binds that instance to the no-op provider. Later startup does not
  replace the cached meter. Start the service before registering or recording
  metrics.

## `#[non_exhaustive]` posture

ADR-0049 does not place `camel-otel` in its mandatory contract-crate set. Its
Rule 3 framework therefore applies case by case, not as a blanket requirement.

| Public enum | Current posture | Rationale |
|---|---|---|
| `OtelProtocol` | Exhaustive | Public configuration enum with a deliberately small supported protocol set. Adding a variant is a breaking API change and requires an explicit pre-freeze decision. |
| `OtelSampler` | Exhaustive | Public configuration enum that exposes a selected sampler subset. Adding a variant is a breaking API change and requires an explicit pre-freeze decision. |

## Related decisions

- ADR-0012 defines the log-policy classification below.
- ADR-0049 supplies the case-by-case API framework but does not bind this crate.
- ADR-0007 motivates graceful provider shutdown. OpenTelemetry global ownership
  makes shutdown order operationally significant here.

## ADR-0012 log-policy annotations

| File | Line | Category | Reason |
|------|------|----------|--------|
| `src/service.rs` | 288 | `system-broken` | Lifecycle service start failure — config validation failed |
