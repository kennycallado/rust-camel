# Proposal: audit-fix-otel-lifecycle

## Why

The camel-otel service has two OpenTelemetry lifecycle-correctness findings
(C4 in the execution strategy, both P2, same crate):

- **rc-z0y3 — OnceLock meter stale-binding (FC-LAZY-CACHE-STALE-BINDING).**
  `OtelMetrics` caches its `Meter` and built instruments in `OnceLock`s. If the
  first metric is recorded before `OtelService::start()` installs the global
  `MeterProvider`, the `OnceLock` permanently binds to the no-op provider. Later
  startup does NOT replace the cached meter/instruments — silent permanent
  metric loss. The camel-otel CONTEXT.md documents this as a known constraint;
  this change mitigates it in code so the binding self-heals after start.

- **rc-3ixr — no Drop / global-state-leak (FC-GLOBAL-STATE-LEAK).**
  `OtelService::stop()` shuts down the tracer/meter/logger providers, but if an
  `OtelService` is dropped WITHOUT `stop()` being called (early return, panic,
  test teardown), the batch-exporter background tasks and provider handles leak.
  OpenTelemetry global setters provide no reset API and silently overwrite. A
  `Drop` guard that best-effort shuts down surviving providers closes the leak.

## What Changes

- **rc-z0y3:** Add a `started: AtomicBool` to `OtelMetrics`. Gate meter/instrument
  resolution on it: before `start()`, metric recording is a no-op (returns early,
  does NOT cache no-op instruments); after `start()` calls
  `mark_started()`, the `OnceLock`s populate from the now-real global provider and
  bind correctly. `OtelService::start()` calls `self.metrics.mark_started()` after
  installing the `MeterProvider`.
- **rc-3ixr:** Add `impl Drop for OtelService` that, if providers are still
  `Some` (i.e. `stop()` was not called), logs a `warn!` and best-effort
  `force_flush` + `shutdown` on each surviving provider. This prevents
  batch-exporter task leaks on drop-without-stop.

Explicitly excluded: changing the OTel SDK global-provider-overwrite semantics
(an SDK constraint outside our control), hot-reload of OTel config (CONTEXT.md
says config is not hot-reloadable), and any change to `OtelConfig` or the
`MetricsCollector` trait.

## Acceptance criteria

- Recording a metric before `start()` does NOT populate the `OnceLock`s; after
  `mark_started()`, recording populates them from the real global provider (no
  permanent no-op binding).
- Dropping an `OtelService` that still holds providers (stop not called) shuts
  them down best-effort and emits one `warn!`, rather than leaking exporter tasks.
- Existing camel-otel tests pass; new regression tests cover both behaviors.
- `cargo fmt --check`, `cargo clippy -p camel-otel -- -D warnings`, and the
  workspace quality gates are green.

## Risk budget

Medium. The meter-resolution change touches the hot metrics-recording path
(every `record_exchange_duration` / `increment_*` call). The gate is a single
`AtomicBool::load(Acquire)` (cheap, no lock contention) plus one `Option` check
per call — negligible overhead. The `Drop` impl is off the hot path. Acceptable
risk: pre-start metrics are now skipped (they were already lost to no-op before —
no functional regression). Out of bounds: redesigning the global-provider model,
adding hot-reload, or changing public API surface beyond the two fixes.
