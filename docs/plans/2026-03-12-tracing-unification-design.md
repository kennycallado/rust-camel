# Tracing Unification Design

**Date:** 2026-03-12
**Status:** ✅ Implemented

---

## Overview

The current tracing architecture lost cohesion when `OtelService` was introduced as a `Lifecycle` service. The original design intended OTel as an additional output layer on top of the `tracing` subscriber, but the implementation made the two paths mutually exclusive: when OTel is active, the local structured JSON output disappears entirely, and the subscriber is owned by `OtelService.start()` rather than by the system that knows how to configure it (`configure_context`).

This document defines the corrected architecture: a single unified subscriber, always assembled by `configure_context`, with OTel as an additive layer via `tracing-opentelemetry`. `OtelService` becomes purely a provider manager — it no longer installs any subscriber.

---

## Section 1: Root Cause Analysis

### How it broke

The `opentelemetry-design.md` explicitly rejected `tracing-opentelemetry` and used the OTel API directly in `TracingProcessor`. This created two parallel emission channels:

1. `opentelemetry::global::tracer("camel-core")` → OTel native span (exported to OTLP if provider installed)
2. `tracing::info_span!(target: "camel_tracer", ...)` → tracing span (captured by subscriber if installed)

Because `OtelService` sets a global `TracerProvider` via `global::set_tracer_provider()`, it also needed to install a subscriber (to bridge `tracing` log events to OTLP). This subscriber had no `camel_tracer`-filtered JSON layer. `configure_context` worked around this by making the two paths mutually exclusive (`if otel_enabled { skip init_tracing_subscriber }`).

Result: enabling OTel silently drops all local structured trace output.

### Why `exchange.otel_context` must stay `opentelemetry::Context`

The W3C propagation API (`inject_from_exchange`, `extract_into_exchange`) in `camel-otel/src/propagation.rs` is typed on `opentelemetry::Context`. HTTP and Kafka components call these functions directly. Changing the field type would require rewriting all propagation logic and every test that touches it. The field stays as-is.

### Why dual-channel TracingProcessor is acceptable

Given that `exchange.otel_context` must be `opentelemetry::Context`, `TracingProcessor` legitimately needs both channels:
- The `tracing::info_span!` channel feeds the unified subscriber (stdout JSON, file, and `tracing-opentelemetry` bridge)
- The `opentelemetry::global::tracer()` channel handles the parent-child context stored in `exchange.otel_context` for W3C propagation

The problem is not the dual channel — it is that the subscriber that should capture both is not always installed correctly.

---

## Section 2: Target Architecture

### Subscriber ownership (single location)

`configure_context` in `camel-config/src/context_ext.rs` is the **only place** that installs a tracing subscriber. It always does so, unconditionally. `OtelService.start()` never touches the subscriber.

```
configure_context(config)
  │
  ├─ always: init_tracing_subscriber(tracer_config, log_level, otel_active)
  │     Layer 1: EnvFilter(log_level, suppress h2/hyper/tonic/reqwest/tower)
  │     Layer 2: fmt::layer()           — plaintext, all events (stdout)
  │     Layer 3: fmt::layer().json()    — FmtSpan::CLOSE, target=camel_tracer
  │              OR fmt::layer()        — if OutputFormat::Plain
  │              → to stdout or file per TracerConfig.outputs
  │     Layer 4: tracing_opentelemetry::layer()  ← only if otel feature + otel enabled
  │              filtered to target=camel_tracer
  │
  └─ if otel_enabled: ctx.with_lifecycle(OtelService::new(otel_config))
        OtelService.start():
          global::set_tracer_provider(SdkTracerProvider)   ← provider only, no subscriber
          global::set_meter_provider(SdkMeterProvider)
        OtelService.stop():
          sdk.shutdown()
          [routes stop before services — ordering guaranteed by context.rs]
```

### Flow with and without OTel

**Without `camel-otel`:**
```
TracingProcessor
  tracing::info_span!(target: "camel_tracer")
    → Layer 3 captures FmtSpan::CLOSE → JSON to stdout (or plaintext if Plain)
  global::tracer("camel-core") → noop provider → spans silently discarded
  exchange.otel_context = Context::new() (no valid span, W3C propagation inactive)
```

**With `camel-otel`, OTel enabled:**
```
TracingProcessor
  tracing::info_span!(target: "camel_tracer")
    → Layer 3 captures FmtSpan::CLOSE → JSON to stdout  (local output preserved)
    → Layer 4 tracing-opentelemetry bridge → real OTel span → OTLP export
  global::tracer("camel-core") → SdkTracerProvider → real span for exchange.otel_context
  exchange.otel_context = child span context (W3C propagation active)
```

Both modes produce correct local output. OTel adds OTLP export without replacing anything.

---

## Section 3: Changes Required

### 3.1 New dependency: `tracing-opentelemetry`

```toml
# workspace Cargo.toml
tracing-opentelemetry = { version = "0.29" }  # compatible with opentelemetry 0.31

# camel-config Cargo.toml
tracing-opentelemetry = { workspace = true, optional = true }

# camel-config feature
[features]
otel = ["dep:tracing-opentelemetry", "dep:camel-otel"]
```

Note: `tracing-opentelemetry` 0.29 is the version compatible with `opentelemetry` 0.31 (the version pinned in the workspace).

### 3.2 `camel-config/src/context_ext.rs`

**Remove** the `if otel_enabled { ... } else { init_tracing_subscriber(...) }` branch.

**Replace with:**
```rust
// Always install the unified subscriber
init_tracing_subscriber(&tracer_config, &config.log_level, otel_enabled)?;

// OtelService manages providers — subscriber already installed above
if otel_enabled {
    ctx = ctx.with_lifecycle(otel_service);
}
ctx.set_tracer_config(tracer_config);
```

`init_tracing_subscriber` gains a third parameter `otel_active: bool`:
- Layers 1–3: always assembled (same as today, plus OutputFormat::Plain implemented)
- Layer 4: assembled only if `otel_active && cfg!(feature = "otel")`:
  ```rust
  #[cfg(feature = "otel")]
  if otel_active {
      let otel_layer = tracing_opentelemetry::layer()
          .with_filter(filter_fn(|meta| meta.target() == "camel_tracer"));
      registry = registry.with(otel_layer);
  }
  ```

### 3.3 `camel-config/src/context_ext.rs` — implement `OutputFormat::Plain`

Layer 3 branches on `OutputFormat`:
```rust
match tracer_config.outputs.stdout.format {
    OutputFormat::Json => {
        let layer = fmt::layer()
            .json()
            .with_span_events(FmtSpan::CLOSE)
            .with_filter(camel_tracer_filter());
        registry = registry.with(layer);
    }
    OutputFormat::Plain => {
        let layer = fmt::layer()
            .with_span_events(FmtSpan::CLOSE)
            .with_filter(camel_tracer_filter());
        registry = registry.with(layer);
    }
}
```
Same branching for the file output layer.

### 3.4 `camel-services/camel-otel/src/service.rs`

**Remove** `init_subscriber()` method entirely.

**Remove** `logs_enabled` / `SdkLoggerProvider` setup from `start()` — the `OpenTelemetryTracingBridge` log export was only needed because OtelService owned the subscriber. With `tracing-opentelemetry` bridging spans directly, log export is handled differently (see §3.5).

**`start()` becomes:**
```rust
async fn start(&mut self) -> Result<(), CamelError> {
    // Guard: prevent double-init
    if self.tracer_provider.is_some() { return Ok(()); }

    let tracer_provider = build_tracer_provider(&self.config)?;
    global::set_tracer_provider(tracer_provider.clone());
    self.tracer_provider = Some(tracer_provider);

    let meter_provider = build_meter_provider(&self.config)?;
    global::set_meter_provider(meter_provider.clone());
    self.meter_provider = Some(meter_provider);

    Ok(())
}
```

**`stop()` shutdown order** — shutdown tracer provider first (flushes pending spans), then meter provider.

### 3.5 Log export via OTel

With `init_subscriber()` removed from `OtelService`, the `OpenTelemetryTracingBridge` (OTLP log export) is lost. This is intentional for now: the `tracing-opentelemetry` layer handles span export, and `tracing` log events (non-span) go to stdout via Layer 2. OTLP log export can be re-added later as a configurable layer in `init_tracing_subscriber` without coupling it to `OtelService`.

This is a deliberate scope reduction. The `OTel: opentelemetry-appender-tracing bridge (log export)` item in TODO.md remains pending.

### 3.6 `camel-core/src/config.rs` — remove dead config

Remove `OpenTelemetryOutput` from `TracerConfig.outputs`:
```rust
// REMOVE:
pub struct TracerOutputs {
    pub stdout: StdoutOutput,
    pub file: Option<FileOutput>,
    pub opentelemetry: Option<OpenTelemetryOutput>,  // ← dead, remove
}

pub struct OpenTelemetryOutput { ... }  // ← dead, remove entirely
```

OTel is configured exclusively via `[observability.otel]` in `Camel.toml`. There is no `tracer.outputs.opentelemetry`.

### 3.7 `crates/camel-core/src/context.rs` — stop ordering

Services must stop in **reverse insertion order**, after all routes are stopped. Currently they stop in forward order. Fix: reverse the services vec before stopping, or use a `VecDeque` and pop from the back.

This guarantees `OtelService.stop()` (which shuts down the OTLP exporter) is called after all routes finish emitting spans.

---

## Section 4: What Does NOT Change

- `exchange.otel_context: opentelemetry::Context` — field type unchanged
- `camel-otel/src/propagation.rs` — inject/extract API unchanged
- HTTP and Kafka propagation (`#[cfg(feature = "otel")]` blocks) — unchanged
- `TracingProcessor` dual-channel emission — unchanged (correct as-is)
- `compose_traced_pipeline` signature — unchanged
- `set_tracer_config` in `route_controller.rs` — unchanged (only needs enabled + detail_level)
- All existing tests in `camel-otel/tests/integration.rs` — no breakage expected
- `OtelMetrics` / Prometheus — unaffected

---

## Section 5: Testing Strategy

### New tests required

1. **`camel-config` integration test: unified subscriber with OTel active**
   - Build `CamelConfig` with both `[observability.tracer] enabled = true` and `[observability.otel] enabled = true`
   - Call `configure_context`
   - Verify no `CamelError` is returned
   - Verify that `stdout` layer captures `camel_tracer` spans (use in-memory writer)
   - Verify that `tracing-opentelemetry` layer is present (check global provider is the SDK one)

2. **`OutputFormat::Plain` test**
   - Build config with `format = "plain"` for stdout
   - Run a route with tracing enabled
   - Capture stdout; verify output is NOT JSON (no leading `{`)

3. **`OutputFormat::Json` regression test** — existing behavior, ensure JSON output still works

4. **`OtelService.start()` — no subscriber side-effect**
   - Install a known subscriber before `OtelService.start()`
   - Call `start()`
   - Verify the pre-installed subscriber is still the active one (not replaced)

5. **`serial_test` for OTel unit tests** — convert `#[ignore]` tests in `service.rs` to `#[serial]`

### Existing tests that must continue passing

- `camel-otel/tests/integration.rs` — all span hierarchy and propagation tests
- `camel-test/tests/tracer_test.rs` — all three tests
- `camel-api` exchange tests
- HTTP/Kafka propagation unit tests

---

## Section 6: Camel.toml Configuration After Change

```toml
[observability.tracer]
enabled = true
detail_level = "full"   # minimal | medium | full

[observability.tracer.outputs.stdout]
enabled = true
format = "plain"        # json | plain  ← NOW IMPLEMENTED

[observability.tracer.outputs.file]
enabled = true
path = "traces.log"
format = "json"

# OTel is configured independently — no [observability.tracer.outputs.opentelemetry]
[observability.otel]
enabled = true
endpoint = "http://localhost:4317"
service_name = "my-service"
```

---

## Non-Goals

- OTLP log export (re-adding `opentelemetry-appender-tracing` as a configurable layer) — deferred
- Removing the native OTel channel from `TracingProcessor` — not needed, acceptable as-is
- Changing `exchange.otel_context` field type — not needed, correct as-is
- Any changes to Kafka or HTTP propagation logic — out of scope

---

## Postmortem (2026-03-12)

### Implementation Summary

All 7 tasks completed successfully in 6 commits:

| Commit | Description |
|--------|-------------|
| `089beb0` | Add `tracing-opentelemetry 0.32` to workspace |
| `c1ee212` | Fix stop ordering (reverse insertion order) |
| `e9185bb` | Remove `OpenTelemetryOutput` dead config |
| `ce6533c` | Remove subscriber installation from `OtelService` |
| `0e984b1` | Unified subscriber in `configure_context` + `OutputFormat::Plain` |
| `15ebd39` | Convert `#[ignore]` tests to `#[serial]` |

### Files Changed

| File | Changes |
|------|---------|
| `Cargo.toml` (workspace) | +1 dep (`tracing-opentelemetry`) |
| `crates/camel-config/Cargo.toml` | +optional dep + `otel` feature |
| `crates/camel-config/src/context_ext.rs` | Unified subscriber, 4 layers |
| `crates/camel-config/tests/subscriber_test.rs` | +3 tests (new file) |
| `crates/camel-core/src/config.rs` | -22 lines (dead config removed) |
| `crates/camel-core/src/context.rs` | +51 lines (reverse stop + test) |
| `crates/camel-core/src/lib.rs` | -8 lines (removed re-export) |
| `crates/camel-test/tests/tracer_test.rs` | -1 line |
| `crates/services/camel-otel/Cargo.toml` | +1 dev-dep (`serial_test`) |
| `crates/services/camel-otel/src/service.rs` | Simplified (removed subscriber) |
| `examples/otel-demo/src/main.rs` | -1 line |
| `examples/tracer-demo/src/main.rs` | -1 line |

### Verification

- **500+ tests pass** across workspace
- **33 camel-otel tests pass** (including formerly `#[ignore]` ones)
- **otel-demo builds** without errors
- **No warnings** from changed files

### Lessons Learned

1. **Plan accuracy matters**: The plan missed `lib.rs` re-export cleanup, but subagent caught it during review
2. **Parallel dispatch works**: Tasks 1 and 3 completed independently without conflicts
3. **Spec + Code review is essential**: Task 6 was complete but uncommitted - review caught it
4. **`serial_test` pattern**: Consistent with how `camel-test` resolved parallel test issues

### Deviations from Plan

None significant. Minor additions:
- Removed `build_log_exporter()` method (only used by removed `init_subscriber`)
- Removed stale FIXME comment in test (now resolved)
- Fixed examples that referenced removed `opentelemetry` field

### Deferred Items

- OTLP log export (`opentelemetry-appender-tracing` bridge) — remains in TODO.md
- `OtelService.as_metrics_collector()` auto-registro — remains in TODO.md
