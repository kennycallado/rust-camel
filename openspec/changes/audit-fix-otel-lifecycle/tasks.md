# Tasks: audit-fix-otel-lifecycle

## camel-otel

### Task 1.1: Start-gate meter/instrument resolution in OtelMetrics

**Files:**
- `crates/services/camel-otel/src/metrics.rs` (modified)
- `crates/services/camel-otel/src/service.rs` (modified)

**Steps:**
1. In `metrics.rs`, add a field `started: std::sync::atomic::AtomicBool` to
   `OtelMetrics` (after the `meter` field, ~line 72). Initialize it to
   `AtomicBool::new(false)` in `OtelMetrics::new()` (~line 88).
2. Add a method `pub(crate) fn mark_started(&self)` to `impl OtelMetrics` that
   does `self.started.store(true, std::sync::atomic::Ordering::Release)`.
3. Add a private non-gated helper `fn meter_inner(&self) -> &Meter` to
   `impl OtelMetrics` holding the OLD `meter()` body
   (`self.meter.get_or_init(|| { global::meter_with_scope(scope) })`). This
   helper is the un-gated resolution used by `instruments()`.
4. Change `fn instruments(&self) -> &MetricInstruments` (~line 97) to
   `fn instruments(&self) -> Option<&MetricInstruments>`: if
   `!self.started.load(Ordering::Acquire)` return `None`; otherwise return
   `self.instruments.get_or_init(|| { ... }).into()`. In the `get_or_init`
   closure, change the `let meter = self.meter();` line (~line 99) to
   `let meter = self.meter_inner();` (NOT `self.meter()`, which now returns
   `Option`). Keep the rest of the closure body unchanged.
5. Change `fn meter(&self) -> &Meter` (~line 133) to
   `fn meter(&self) -> Option<&Meter>`: if `!self.started.load(Ordering::Acquire)`
   return `None`; otherwise `Some(self.meter_inner())`. (The body delegates to the
   private helper added in step 3 — do NOT duplicate the `get_or_init` closure.)
6. Update the 5 fixed-instrument record methods
   (`record_exchange_duration`, `increment_errors`, `increment_exchanges`,
   `set_queue_depth`, `record_circuit_breaker_change`) to gate the OTel call:
   change `self.instruments().<field>.<op>(...)` to
   `if let Some(inst) = self.instruments() { inst.<field>.<op>(...) }`.
   NOTE: in `set_queue_depth` and `record_circuit_breaker_change`, the
   Mutex bookkeeping (queue_depths/cb_states delta calc) BEFORE the instrument
   call stays unchanged — only the `self.instruments()` call is gated, so local
   state tracking remains accurate.
7. Update the 2 dynamic methods (`record_counter` ~line 213, `record_histogram`
   ~line 252) to gate FIRST, as a silent pre-start no-op: at the very top of each
   method body, before any validation/normalization/warning/allocation/DashMap
   access, add `let Some(meter) = self.meter() else { return; };`. Then in the
   `Entry::Vacant` branch, use that `meter` (not `self.meter()` again) to build
   the instrument: `let counter = meter.u64_counter(normalized.clone()).build();`
   (and `meter.f64_histogram(...)` for the histogram). When not started,
   `self.meter()` returns `None` and the method returns immediately — no
   validation, no warning, no DashMap entry cached (matching the spec's silent
   pre-start no-op contract).
8. Update existing tests in `#[cfg(test)] mod tests` (~line 290+) that call
   `metrics.instruments()` expecting `&MetricInstruments`: those that do
   `let _ = metrics.instruments();` compile fine (Option), but any test that
   accesses `.instruments().<field>` or `.meter().u64_counter(...)` must be
   adapted to the `Option` return (call `metrics.mark_started()` first, then
   `if let Some(...) = metrics.instruments()`). The dynamic-instrument tests
   (~line 440-546) that build counters/histograms must call `mark_started()`
   before, since `meter()` now returns `None` until started.
9. In `service.rs`, in `impl Lifecycle for OtelService::start()`
   (~line 280-360), immediately after the
   `global::set_meter_provider(meter_provider.clone());` line (~line 335) and
   BEFORE `self.meter_provider = Some(meter_provider);`, add:
   `if let Some(m) = &self.metrics { m.mark_started(); }` (the lazy form avoids a
   `.expect()` that would trip the `lint-unwrap` gate; `metrics` is always `Some`
   after `new()` but the if-let needs no escape hatch).
10. Run `cargo fmt -p camel-otel`, `cargo check -p camel-otel`,
   `cargo clippy -p camel-otel -- -D warnings`, `cargo test -p camel-otel --lib`.

**Tests:** (executable spec)

- `pre_start_recording_does_not_cache_instruments`:
  - setup: `let metrics = OtelMetrics::new("test-svc");` (started == false).
  - action: `metrics.increment_exchanges("route-1");`
    `metrics.record_counter("camel.exec.total", 1.0, &[]);`
    `metrics.record_histogram("camel.dur.hist", 1.0, &[]);`
  - assert: `metrics.instruments.get().is_none()` (the OnceLock was not
    populated) AND `metrics.dyn_counters.is_empty()` AND
    `metrics.dyn_histograms.is_empty()` (no dynamic entry cached on either
    dynamic path).
- `post_start_populates_instruments`:
  - setup: `let metrics = OtelMetrics::new("test-svc");`
  - action: `metrics.mark_started();` then `metrics.increment_exchanges("route-1");`
  - assert: `metrics.instruments.get().is_some()` (the OnceLock populated after
    mark_started).
- `meter_returns_none_before_start`:
  - setup: `let metrics = OtelMetrics::new("test-svc");`
  - action: `let m = metrics.meter();`
  - assert: `assert!(m.is_none());`
- `meter_returns_some_after_start`:
  - setup: `let metrics = OtelMetrics::new("test-svc");`
  - action: `metrics.mark_started(); let m = metrics.meter();`
  - assert: `assert!(m.is_some());`
- command: `cargo test -p camel-otel --lib`
- expected: the four new tests pass after the gate lands. The pre-start test
  would FAIL before the change (the old code populated the OnceLock on first
  call regardless of start state).

**Acceptance:**
- `cargo check -p camel-otel` and `cargo clippy -p camel-otel -- -D warnings`
  exit 0.
- `cargo test -p camel-otel --lib` passes including the four new tests.
- `cargo fmt --check` exits 0.
- Spec coverage: pre-start no-cache + dynamic-gate scenarios (unit-testable);
  post-start real-export is the CI-owned integration scenario.

- [x] 1.1

### Task 1.2: Drop guard for surviving OTel providers

**Files:**
- `crates/services/camel-otel/src/service.rs` (modified)

**Steps:**
1. In `service.rs`, after the `impl Lifecycle for OtelService` block and after
   `impl Default for OtelService`, add `impl Drop for OtelService`. The body:
   - Check if any of `self.tracer_provider`, `self.meter_provider`,
     `self.logger_provider` is `Some`.
   - If any is `Some`, emit one
     `warn!(service_name = %self.config.service_name, "OtelService dropped without stop(); shutting down providers best-effort");`
     with `// log-policy: system-broken` annotation.
   - Then for each surviving provider, `take()` it and best-effort
     `force_flush()` + `shutdown()` (discard errors with `let _ =`): three
     explicit typed blocks (one for `SdkTracerProvider`, one for `SdkMeterProvider`,
     one for the logger provider type) — do NOT use a generic helper (the types
     share no shutdown trait bound).
   - If all three are `None` (stop() already ran), the Drop is a no-op (no warn).
2. Run `cargo fmt -p camel-otel`, `cargo check -p camel-otel`,
   `cargo clippy -p camel-otel -- -D warnings`, `cargo test -p camel-otel --lib`.

**Tests:** (executable spec)

- `drop_without_stop_does_not_panic`:
  - setup: construct an `OtelService` and install a provider directly via the
    private field from the same-crate test module
    (`service.tracer_provider = Some(SdkTracerProvider::builder().build());` —
    same-crate `mod tests` can access private fields; an existing test already
    does this at service.rs:643).
  - action: drop the service (let it go out of scope) WITHOUT calling `stop()`.
  - assert: the test passing (Drop returns without panicking) verifies the
    best-effort shutdown ran. Verifying the actual take is out of unit-test
    reach since the value is consumed; the no-panic guard is the unit assertion.
- `drop_after_stop_does_not_reshutdown`:
  - setup: construct an `OtelService` via `OtelService::new(OtelConfig::new(...))`
    — its providers start as `None` by construction, which is exactly the
    post-`stop()` state (stop takes the providers out). Do NOT call `start()`
    (avoids mutating process-global OTel state and the need for async/serial
    test machinery).
  - action: drop the service.
  - assert: no panic, no double-shutdown — Drop's guard sees all `None` and does
    nothing (no provider shut down, no warn). The "no warning logged" clause is
    thus satisfied by construction; out of unit-test reach to capture via log,
    tracked for CI (bd rc-lpog).
- command: `cargo test -p camel-otel --lib drop`
- expected: both tests pass. They guard against Drop panicking and against
  double-shutdown (stop+drop).

**Acceptance:**
- `cargo check -p camel-otel` and `cargo clippy -p camel-otel -- -D warnings`
  exit 0.
- `cargo test -p camel-otel --lib` passes including the two new drop tests.
- `cargo fmt --check` exits 0.
- Spec coverage: drop-without-stop + drop-after-stop scenarios.

- [x] 1.2

### Task 1.3: Integration test for post-start real-provider binding (rc-z0y3)

**Files:**
- `crates/services/camel-otel/src/metrics.rs` (modified) — widen `mark_started`
  from `pub(crate)` to `pub` so the integration test (external to the crate) can
  call it. Add the doc note (already drafted).
- `crates/services/camel-otel/tests/metrics_binding_test.rs` (new) — integration
  test binary (separate process → clean OTel global).

**Steps:**
1. In `metrics.rs`, change `pub(crate) fn mark_started` to `pub fn mark_started`
   (done above).
2. Create `crates/services/camel-otel/tests/metrics_binding_test.rs`. Use the
   existing dev-dependency `opentelemetry_sdk` with the `testing` feature (already
   in Cargo.toml line 33) for `InMemoryMetricExporter` and `ManualReader`.
3. Write a single `#[test] fn post_start_binds_real_provider()` that:
   - Builds an `InMemoryMetricExporter` (`InMemoryMetricExporter::default()`).
   - Builds a `SdkMeterProvider` with `ManualReader` wired to the exporter, sets
     it as the global via `opentelemetry::global::set_meter_provider(provider)`.
   - Creates `OtelMetrics::new("binding-test")` (NOT yet started).
   - Records a metric: `metrics.increment_exchanges("route-1")` → pre-start, must
     be a no-op.
   - Calls `exporter.collect()`/reads `get_finished_metrics()` → asserts NO
     `exchanges_total` metric present (pre-start recording did not bind/export).
   - Calls `metrics.mark_started()`.
   - Records again: `metrics.increment_exchanges("route-1")`.
   - Collects and asserts the `exchanges_total` metric IS present in the exporter
     (the post-start binding resolved to the real provider, not a no-op).
4. Run `cargo test -p camel-otel --test metrics_binding_test`,
   `cargo clippy -p camel-otel --all-targets -- -D warnings`,
   `cargo fmt --check`.

**Tests:** (executable spec)
- `post_start_binds_real_provider`:
  - setup: global `SdkMeterProvider` with `InMemoryMetricExporter` + `ManualReader`;
    fresh `OtelMetrics`.
  - action: record before `mark_started` → collect (assert absent); `mark_started`
    → record → collect (assert present).
  - assert: pre-start collect has no `exchanges_total`; post-start collect has it.
- command: `cargo test -p camel-otel --test metrics_binding_test`
- expected: passes after the rc-z0y3 fix. Would FAIL before the fix (the old code
  cached a no-op meter pre-start, so post-start would never bind the real one).

**Acceptance:**
- `cargo test -p camel-otel --test metrics_binding_test` passes.
- `cargo clippy -p camel-otel --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- `lint-non-exhaustive` and `lint-unwrap` unchanged (no new `pub enum`, no
  production `unwrap`).
- Spec coverage: closes the rc-lpog follow-up — the post-start real-provider
  binding scenario is now integration-tested in-process, not deferred to CI.

- [x] 1.3
