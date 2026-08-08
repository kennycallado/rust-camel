# Design: audit-fix-otel-lifecycle

## Approach

Two independent lifecycle-correctness fixes in `camel-otel`, unified by crate.

### Fix 1 (rc-z0y3): start-gated meter resolution

`OtelMetrics` currently resolves its `Meter` and `MetricInstruments` into
`OnceLock`s on first use (`metrics.rs:69-72, 98-128, 133-139`). If first use
precedes `OtelService::start()` → `global::set_meter_provider()`
(`service.rs:335`), the `OnceLock`s bind permanently to the no-op provider.

The fix gates ALL meter/instrument resolution on a `started: AtomicBool`,
keeping the existing `OnceLock`s (no per-call allocation):

- Add `started: std::sync::atomic::AtomicBool` to `OtelMetrics` (default `false`).
- Add `pub(crate) fn mark_started(&self)` that does
  `self.started.store(true, Ordering::Release)`.
- `OtelService::start()` calls
  `if let Some(m) = &self.metrics { m.mark_started(); }` immediately after
  `global::set_meter_provider(meter_provider.clone())` succeeds
  (`service.rs:335`). (The `metrics` field is `Option<Arc<OtelMetrics>>`, always
  `Some` after `new()`; the lazy `if let` form needs no escape hatch and avoids
  the `lint-unwrap` gate that `.expect()` would trip.)
- `fn instruments(&self)` changes return type to `Option<&MetricInstruments>`:
  returns `None` when `started` is false (BEFORE touching the `OnceLock`); otherwise
  `get_or_init` from the global meter (now the real provider). Its `get_or_init`
  closure calls a private non-gated helper `fn meter_inner(&self) -> &Meter`
  (the old `meter()` body), NOT `meter()` (which now returns `Option`).
- Add private `fn meter_inner(&self) -> &Meter` holding the old
  `get_or_init(|| { global::meter_with_scope(scope) })` body — the un-gated
  resolution used by `instruments()`.
- `fn meter(&self)` changes return type to `Option<&Meter>`: returns `None` when
  not started (BEFORE delegating); otherwise `Some(self.meter_inner())`. Keeping
  the `OnceLock` (inside `meter_inner`) avoids rebuilding `InstrumentationScope`
  and resolving the global on every dynamic call (the allocation concern).
- Every `MetricsCollector` record method (and the dynamic-counter/histogram
  helpers at `metrics.rs:237-245, 274-281`) gates BEFORE touching its `DashMap`:
  the dynamic helpers return early when `self.meter()` is `None`, so no entry is
  cached before start.

Net behavior: before `start()`, metric recording is a silent no-op (as it was
when bound to no-op — no functional regression). After `start()`, the
`OnceLock`s populate from the real provider and bind permanently correct. The
permanent-no-op-binding bug is gone.

**Single-start contract.** `OtelService` is single-start: restarting a stopped
service is UNSUPPORTED. The OTel global has no reset API, and cached instruments
remain bound to the (shut-down) first provider on restart. This matches the
camel-otel CONTEXT.md invariant ("Keep one active `OtelService` per process and
stop it before replacement"). A restart attempt does not corrupt state but is
not guaranteed to re-bind — the CONTEXT.md contract already forbids it.

**Hot-path cost:** one `AtomicBool::load(Acquire)` + one `Option` check per
record call. No mutex, no allocation, no scope rebuild after the first start-gated
resolution.

### Fix 2 (rc-3ixr): Drop guard for surviving providers

`OtelService::stop()` (`service.rs:362-403`) takes providers out of their
`Option`s and shuts them down. But drop-without-stop leaves them `Some`,
leaking batch-exporter tasks.

Add `impl Drop for OtelService`:

```rust
impl Drop for OtelService {
    fn drop(&mut self) {
        if self.tracer_provider.is_some()
            || self.meter_provider.is_some()
            || self.logger_provider.is_some() {
            warn!(service_name = %self.config.service_name,
                  "OtelService dropped without stop(); shutting down providers best-effort");
            // log-policy: system-broken
            // Explicit typed blocks (the three SDK provider types share no trait
            // bound for force_flush/shutdown). Each takes() the Option first so a
            // later stop()/drop cannot double-shutdown.
            if let Some(provider) = self.tracer_provider.take() {
                let _ = provider.force_flush();
                let _ = provider.shutdown();
            }
            if let Some(provider) = self.meter_provider.take() {
                let _ = provider.force_flush();
                let _ = provider.shutdown();
            }
            if let Some(provider) = self.logger_provider.take() {
                let _ = provider.force_flush();
                let _ = provider.shutdown();
            }
        }
    }
}
```

Each provider's `force_flush`/`shutdown` are synchronous inherent methods in
opentelemetry_sdk 0.32. The `take()` guarantees no double-shutdown with `stop()`
(which already `take()`s). `Drop` must not panic, so errors are discarded (the
`warn!` is the operator signal). The `log-policy: system-broken` annotation
follows ADR-0012.

## Affected crates

- `camel-otel` (`crates/services/camel-otel/`):
  - `src/metrics.rs`: add `started` field + `mark_started`; gate `instruments()`,
    `meter()`, and the dynamic-instrument helpers; update record methods.
  - `src/service.rs`: call `mark_started()` in `start()`; add `impl Drop`;
    `Drop` mirrors `stop()` using three explicit typed `take()` blocks (one per
    provider type), since the SDK provider types share no shutdown trait bound.

## Architecture boundaries

Both fixes are internal to the `camel-otel` `Lifecycle` service. No change to
the `Lifecycle` trait, the `MetricsCollector` trait, `OtelConfig`, or any
public type signature except the two private resolver methods (`instruments`/
`meter`) which become `Option`-returning — they are private (`fn`, not `pub`).
The OTel global-provider-overwrite constraint (one active provider per process,
documented in camel-otel CONTEXT.md) is respected, not changed: `Drop` mirrors
`stop()`'s shutdown; it does not reset the global.

## Alternatives considered

- **Re-resolve meter on every call without a start gate.** Rejected: the
  `OnceLock<MetricInstruments>` would still cache no-op instruments built before
  start; the start gate is required to prevent that permanent cache.
- **Detect no-op meter and re-init.** Rejected: opentelemetry 0.32 does not
  expose "is this a no-op meter". The start gate is the deterministic signal.
- **Make `stop()` return providers to a pool / reset global.** Rejected: the
  OTel SDK has no global reset API (CONTEXT.md). `Drop` best-effort shutdown is
  the achievable mitigation.
- **Panic in `Drop` if not stopped.** Rejected: `Drop` must not panic. Best-effort
  shutdown + `warn!` is the safe posture.
