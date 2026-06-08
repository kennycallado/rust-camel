# camel-otel

OpenTelemetry service implementation. Manages lifecycle of OTel providers (TracerProvider, MeterProvider, LoggerProvider).

## ADR-0012 log-policy annotations

| File | Line | Category | Reason |
|------|------|----------|--------|
| `src/service.rs` | 288 | `system-broken` | Lifecycle service start failure — config validation failed |
