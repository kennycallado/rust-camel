# otel-lifecycle Specification

## Purpose
TBD - created by archiving change audit-fix-otel-lifecycle. Update Purpose after archive.
## Requirements
### Requirement: OtelMetrics resolution is gated on service start

The system SHALL NOT cache OpenTelemetry meters or instruments before the
`OtelService` has installed the global `MeterProvider`. Metric recording before
`start()` SHALL be a silent no-op that does not populate any cached instrument
state. After `start()`, metric recording SHALL resolve meters and instruments
from the real global provider, so the binding is permanently correct.

`OtelService` is single-start: restarting a stopped service is unsupported (the
OTel global provides no reset API; cached instruments stay bound to the shut-down
first provider). This matches the camel-otel CONTEXT.md one-active-service
invariant.

#### Scenario: pre-start recording does not bind no-op instruments (unit-testable)

- **GIVEN** a fresh `OtelMetrics` whose `started` flag is false (before
  `OtelService::start()`)
- **WHEN** a metric is recorded (e.g. `increment_exchanges("route-1")`)
- **THEN** the `instruments` cache remains unpopulated (`instruments.get()` is
  `None`) and no no-op instrument is cached; the call returns without recording

#### Scenario: dynamic instruments are start-gated (unit-testable)

- **GIVEN** a fresh `OtelMetrics` not yet started
- **WHEN** a dynamic counter or histogram is created
- **THEN** no entry is cached in the `dyn_counters`/`dyn_histograms` DashMaps
  until `mark_started()` has run

#### Scenario: post-start binding resolves the real provider (integration test)

- **GIVEN** an `OtelMetrics` that has been marked started after a global
  `MeterProvider` backed by an in-memory test exporter is installed
- **WHEN** a metric is recorded and the provider is collected
- **THEN** the metric is present in the exporter (the binding resolved to the
  real provider, not a no-op)

This scenario is verified by an in-process integration test in
`crates/services/camel-otel/tests/` (a separate binary per test file, so the
process-global OTel state is clean at start). The test installs a global
`SdkMeterProvider` backed by `InMemoryMetricExporter` + `PeriodicReader`,
collects synchronously via `provider.force_flush()` (the workspace does not
enable the `experimental_metrics_custom_reader` feature, so `ManualReader` is
not available). The pre-start record is made while the global is still the
no-op default (before `set_meter_provider`), then the real provider is
installed and `mark_started()` is called, then a post-start record is made and
the metric is asserted present in the exporter. This directly proves the
rc-z0y3 fix: the binding is the real provider, not a cached no-op.

### Requirement: OtelService drops safely without explicit stop

The system SHALL shut down surviving OpenTelemetry providers when an
`OtelService` is dropped without `stop()` having been called. The shutdown SHALL
be best-effort (flush then shut down each provider) and SHALL emit one diagnostic
warning, so batch-exporter background tasks do not leak.

#### Scenario: drop without stop shuts down surviving providers

- **GIVEN** an `OtelService` that holds at least one provider (tracer, meter, or
  logger) and has NOT had `stop()` called
- **WHEN** the `OtelService` is dropped
- **THEN** each surviving provider is force-flushed and shut down (best-effort)
  and exactly one warning is logged

#### Scenario: drop after stop is a no-op

- **GIVEN** an `OtelService` whose `stop()` has already been called (all
  providers taken out and shut down)
- **WHEN** the `OtelService` is dropped
- **THEN** no additional shutdown is attempted and no warning is logged

### Requirement: TracingProcessor preserves the tower readiness contract

The `TracingProcessor` SHALL invoke the SAME inner processor instance it readied
in `poll_ready` when `call` executes. It SHALL NOT clone the inner processor and
re-drive readiness on the clone, because stateful producers hold reservations
acquired during `poll_ready` that only the readied instance can consume.

#### Scenario: direct InOut hop with tracing enabled completes and repeats

- **GIVEN** a route whose step is `to: "direct:echo"` with tracing enabled (via
  a process-local no-op or in-memory OTel provider — no OTLP network endpoint
  is required, the defect is exporter-independent) and a registered
  `direct:echo` consumer route
- **WHEN** an InOut exchange traverses the entry pipeline
- **THEN** the exchange SHALL complete with the consumer route's effects within
  the test timeout
- **AND** a SECOND exchange sent afterwards SHALL also complete within the test
  timeout (the permit-wedge failure mode would hang it)

#### Scenario: TracingProcessor instance is reusable across sequential cycles

- **GIVEN** a single `TracingProcessor` instance wrapping an inner processor
- **WHEN** `ready().await` then `call(exchange_a).await` completes, and the
  SAME instance is again driven `ready().await` then `call(exchange_b).await`
- **THEN** the second cycle SHALL also complete within the test timeout and
  return the inner's response for `exchange_b` (the wrapper must not become
  one-shot after the first call)

#### Scenario: inner service holding a poll-boundary permit is not re-readied on a clone

- **GIVEN** a mock inner service whose `poll_ready` acquires the sole permit of
  a shared `Semaphore::new(1)` into instance state and whose `Clone` shares the
  semaphore without the permit
- **WHEN** `TracingProcessor` wrapping that mock is driven `ready().await` then
  `call(exchange).await`
- **THEN** the call SHALL complete within the test timeout (no permanent
  `Pending` on the clone's acquire)
- **AND** the exchange result SHALL be the mock's response

#### Scenario: span lifecycle unchanged by the ownership restructure

- **GIVEN** a configured OTel in-memory provider and a `TracingProcessor`
  wrapping an inner processor
- **WHEN** an exchange completes successfully and a second one fails
- **THEN** each step SHALL produce exactly one span with status Ok / error
  respectively, as before the restructure

### Requirement: Bounded-stop stall repros share one harness

The bounded-stop regression repros in camel-otel (tests that pin `stop()`
boundedness when a provider's export path stalls) SHALL run their arrange /
act / assert bodies through one shared `bounded_repro` helper that owns the
dedicated-thread + current-thread-runtime + panic-channel + outer-timeout
scaffold. Repro thread names, panic diagnostics, and timeout constants SHALL
be preserved verbatim per repro.

#### Scenario: harness is single-source (unit-testable)

- **GIVEN** the camel-otel crate sources after the refactor
- **WHEN** searching for the harness scaffold pattern
  (`mpsc::channel::<Result<(), String>>` and
  `recv_timeout(Duration::from_secs(60))`)
- **THEN** each pattern occurs exactly once — inside the shared helper — and
  both stall repros (`test_stop_bounded_when_metric_export_stalls`,
  `test_stop_bounded_when_span_export_stalls`) invoke `bounded_repro`
  instead of embedding the scaffold

#### Scenario: metric-path repro is preserved (unit-testable)

- **GIVEN** the metric-export stall repro (`rc-q74u`)
- **WHEN** it runs through the shared harness
- **THEN** it spawns thread `q74u-repro`, keeps the 15s bounded-stop
  deadline assertion with its exact message, and reports a hang as
  "stop() hung (rc-q74u regression)"

#### Scenario: span-path repro is preserved (unit-testable)

- **GIVEN** the span-export stall repro (`rc-6ju71`)
- **WHEN** it runs through the shared harness
- **THEN** it spawns thread `q6ju71-repro`, keeps the 15s bounded-stop
  deadline assertion with its exact message, and reports a hang as
  "stop() hung (rc-6ju71 regression)"

### Requirement: OtelService unit tests live in a sibling module

The unit tests of `OtelService` SHALL live in
`crates/services/camel-otel/src/service_tests.rs`, wired from `service.rs`
as a `#[cfg(test)] #[path = "service_tests.rs"] mod tests;` declaration, so
test paths remain `camel_otel::service::tests::*` and `service.rs` carries
no inline test module. The crate's public API SHALL NOT change.

#### Scenario: test identity survives the move (unit-testable)

- **GIVEN** the baseline `cargo test -p camel-otel -- --list` output at
  commit 6b110708 (91 tests)
- **WHEN** the same listing runs after the move
- **THEN** the output is byte-identical: same test names, same count, same
  module paths, and the full suite passes

#### Scenario: crate context documents the layout (unit-testable)

- **GIVEN** `crates/services/camel-otel/CONTEXT.md`
- **WHEN** the refactor lands
- **THEN** it notes the sibling test layout (stall repros in
  `service_tests.rs` sharing `bounded_repro`), its ADR-0012 line anchors
  match the shifted `service.rs` lines, and
  `cargo xtask lint-context-citations` passes

