# Tasks: metrics-handle-late-binding

## camel-api

### Task 1.1: MetricsHandle + CompositeMetricsCollector types

**Files:**
- `crates/camel-api/Cargo.toml` (modified — add `arc-swap = { workspace = true }`)
- `crates/camel-api/src/metrics.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified — root re-export parity for the new public types)

**Steps:**
1. Add `arc-swap = { workspace = true }` to camel-api dependencies (workspace root already pins `arc-swap = "1"`).
2. In `metrics.rs` add `pub struct MetricsHandle { inner: ArcSwap<dyn MetricsCollector> }`:
   - `MetricsHandle::new()` seeds `NoOpMetrics`.
   - `MetricsHandle::register(&self, collector: Arc<dyn MetricsCollector>)`: first dedupe — the handle keeps a parallel membership list `members: Mutex<Vec<Arc<dyn MetricsCollector>>>` (push on every accepted register); reject the new collector if `Arc::ptr_eq` matches ANY member (same-inner-Arc double registration is a real call-site pattern: crates/services/camel-prometheus/tests/lifecycle_test.rs:150-160 registers `.metrics(collector)` AND `.with_lifecycle(prometheus)` with the same inner Arc; without dedupe compose double-counts — introspection of the stored `ArcSwap<dyn>` is impossible, hence the membership list). If present, return without storing; also push accepted collectors to `members`. Otherwise `self.inner.store(Arc::new(CompositeMetricsCollector::new(vec![prev, collector])));` — second registration composes over the first; a third composes over the composite.
   - `impl MetricsCollector for MetricsHandle`: all 7 methods delegate through `self.inner.load()` (no `load_full`, no Arc clone on the hot path).
3. Add `pub struct CompositeMetricsCollector { collectors: Vec<Arc<dyn MetricsCollector>> }` with `new(collectors)`; `impl MetricsCollector` iterates and calls each collector in order for all 7 methods.
4. Both types: rustdoc comments stating the contract (late binding, composition-not-replacement, delegation cost = one atomic load).

**Tests:** (in `metrics.rs` `#[cfg(test)] mod tests`, alongside the existing TestMetrics stub pattern)
- `handle_delegates_to_stored_collector`: MetricsHandle::new + register(Arc<TestMetrics>) → call `record_exchange_duration("r", 1ms)` on the handle → assert TestMetrics recorded `("r", 1ms)`. Command: `cargo test -p camel-api --lib handle_delegates`. Expected: pass after step 2-3.
- `second_registration_composes_both_observe`: two distinct TestMetrics instances, register(a) then register(b) → call `increment_errors("r", "x")` → assert BOTH instances recorded. Command: `cargo test -p camel-api --lib second_registration`. Expected: pass.
- `register_same_arc_is_idempotent`: one TestMetrics Arc, register(a) twice (the `.metrics(a)` + `.with_lifecycle(service-exposing-a)` pattern) → call `increment_exchanges("r")` → assert the instance recorded exactly ONE entry. Command: `cargo test -p camel-api --lib register_same_arc`. Expected: pass.
- `handle_defaults_to_noop`: fresh MetricsHandle::new() → call all 7 methods → no panic, no output (delegates to NoOpMetrics). Command: `cargo test -p camel-api --lib handle_defaults`. Expected: pass.

**Acceptance:**
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean.

- [x] 1.1

## camel-core

### Task 1.2: Wire the shared handle; delete snapshot injection; rc-hrm1.3 regression test

**Files:**
- `crates/camel-core/src/context.rs` (modified — field :49/:77 → `Arc<MetricsHandle>`, `with_lifecycle` :508-510 stores via `register`, delete injection :473-479, `metrics()` :629 returns handle clone as `Arc<dyn MetricsCollector>`)
- `crates/camel-core/src/context_builder.rs` (modified — build() creates the handle once; seeds bus + controller; `with_lifecycle` :135-140 unchanged extraction, consumed at build-time register)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified — new `set_tracer_metrics` setter; `set_tracer_config` :412-416 drops the `metrics_collector` line; fallback sites :460-462/:633-635 unchanged but now unreachable in production)
- `crates/camel-core/src/shared/observability/domain/config.rs` (modified — delete `metrics_collector` field :27 and its redacting `Debug` arm :37-38)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified — regression test)

**Steps:**
1. **RED first (rc-hrm1.3)**: add `controller_path_receives_registered_collector` to `route_controller_tests.rs`. `tracer_metrics` is `pub(super)` and the concrete controller is moved into `spawn_controller_actor` post-build — there is NO accessor to read; the probe mechanism is a **capture component**: a minimal test component whose activation captures the collector it receives at resolution time. Build a context via `CamelContextBuilder::new().with_lifecycle(RecordingLifecycle)` where `RecordingLifecycle` embeds an `Arc<Mutex<Vec<String>>>` `RecordingCollector` (implements all 7 `MetricsCollector` methods) and `as_metrics_collector` returns it; register a route whose `to:` targets the capture component; the capture component stores the `Arc<dyn MetricsCollector>` from its `ControllerComponentContext`-supplied `RuntimeObservability` into a shared slot. Run the route once, then call `increment_exchanges("probe")` through the CAPTURED collector and assert the recording list grew — and separately assert the captured collector is NOT NoOp (NoOp silently accepts; the recorded-entry assertion is the real gate). Run `cargo test -p camel-core --lib controller_path` — assert it FAILS before wiring (the captured collector writes into the void / recording list stays empty).
2. In `CamelContextBuilder::build()`: create `let metrics_handle = Arc::new(MetricsHandle::new());` once; if `self.metrics` is `Some(collector)` (pre-registered via builder `with_lifecycle` or the `.metrics()` setter), call `metrics_handle.register(collector)`. Pass `metrics_handle.clone()` (coerced `Arc<dyn MetricsCollector>`) to `build_runtime(...)` instead of the raw `metrics` value.
3. After each `DefaultRouteController` construction branch in `build()` (supervision and non-supervision), call `controller_impl.set_tracer_metrics(metrics_handle.clone())` — new `pub fn set_tracer_metrics(&mut self, metrics: Arc<dyn MetricsCollector>)` in route_controller.rs.
4. `CamelContext`: change `metrics` field to `Arc<MetricsHandle>`; `with_lifecycle` (:508-510) calls `self.metrics.register(collector)`; delete the reverse-injection block (:473-479 — the `if config.metrics_collector.is_none()` branch); `metrics()` getter returns `Arc::clone(&self.metrics) as Arc<dyn MetricsCollector>`; update `From`/`parts` plumbing (:101) accordingly.
5. Builder `with_lifecycle` (:135-140): keep extracting `as_metrics_collector` but defer to build-time register (store `Option<Arc<dyn MetricsCollector>>` as today; step 2 consumes it).
6. `config.rs`: delete `metrics_collector` field and the redacting Debug branch (plain `derive(Debug)` suffices once the Arc field is gone).
7. Fix all compile errors from the field deletion (`rg "metrics_collector" crates/` — remaining hits are camel-config call sites handled in Task 1.3 or unrelated local test stubs; leave non-TracerConfig hits alone).
8. Run `cargo test -p camel-core --lib controller_path` — now GREEN.

**Tests:**
- `controller_path_receives_registered_collector`: (step 1 spec — red before wiring, green after; the capture component's stored collector, called with `increment_exchanges("probe")`, grows the RecordingCollector's list; the recording list must also contain the real exchange emissions `increment_exchanges`/`record_exchange_duration` after one exchange). Command: `cargo test -p camel-core --lib controller_path_receives`.
- `late_registration_after_build_observed`: `CamelContextBuilder::new().build()` → `ctx.with_lifecycle(RecordingLifecycle)` (on the BUILT context) → process one exchange through a trivial route → assert the recording service observed `increment_exchanges`/`record_exchange_duration`. Command: `cargo test -p camel-core --lib late_registration`. Expected: pass after wiring (red before).
- `tracer_config_carries_no_collector_field`: compile-level — `TracerConfig { enabled: true, detail_level: Default::default(), .. }` constructs without any collector reference; `format!("{tracer_config:?}")` requires no redaction (plain Debug). Command: `cargo test -p camel-core --lib tracer_config`. Expected: pass.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0 (full lib suite).
- `rg "metrics_collector" crates/camel-core/src/shared/observability/domain/config.rs` returns zero hits.
- `rg "unwrap_or_else\(.*NoOpMetrics" crates/camel-core/src/lifecycle/adapters/route_controller*.rs` hits remain ONLY at the (now unreachable) fallback sites — no new fallbacks introduced.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.

- [x] 1.2

## camel-config

### Task 1.3: effective_tracer_config gains prometheus + explicit-set params (rc-685y)

**Files:**
- `crates/camel-config/src/context_ext.rs` (modified — `effective_tracer_config` :944-948, call site :612)
- `crates/camel-core/src/shared/observability/domain/config.rs` (modified — `TracerConfig` gains `#[serde(skip)] tracing_enabled_explicit: bool`, set at the serde boundary)

**Steps:**
1. Change signature to `fn effective_tracer_config(tracer_config: TracerConfig, otel_enabled: bool, prometheus_enabled: bool, tracing_explicitly_set: bool) -> TracerConfig`: if `tracing_explicitly_set` → return unchanged (explicit value wins, both directions); else if `otel_enabled || prometheus_enabled` → `enabled = true`; else unchanged.
2. **Serde-boundary detection** (no production raw-table loader exists in camel-config — only `#[cfg(test)]` smoke deserializers): `TracerConfig.enabled` is `#[serde(default)] bool`, indistinguishable after deserialization. On `TracerConfig` (camel-core config.rs) add `#[serde(skip)] pub tracing_enabled_explicit: bool` (default false) and a custom `Deserialize` for the struct (or `#[serde(with)]` wrapper) that deserializes into an intermediate `Option<bool>` for `enabled`: `enabled = opt.unwrap_or(false)`, `tracing_enabled_explicit = opt.is_some()`. This covers every load path including the CLI. Do NOT touch `enabled`'s public bool shape.
3. Update the call site (:612): pass `otel_enabled`, `prometheus_enabled = config.observability.prometheus.as_ref().is_some_and(|p| p.enabled)`, and `tracer_config.tracing_enabled_explicit`.

**Tests:** (unit, next to the existing `configure_context_smoke_tests`)
- `effective_tracer_config_truth_table`: table-driven over (otel, prometheus, explicit_set, input_enabled) → expected output: `(true,_,false,_)→true`, `(false,true,false,_)→true`, `(false,false,false,false)→false`, `(*,*,true,X)→X` — MUST include the `(true,*,true,false)→false` row (old behavior forced true; intentional change). Command: `cargo test -p camel-config --lib truth_table`.
- `serde_boundary_sets_explicit_flag`: deserialize a TOML fragment with `[observability.tracer] enabled = false` → `tracing_enabled_explicit == true && enabled == false`; same fragment without the `enabled` key → `tracing_enabled_explicit == false && enabled == false`. Command: `cargo test -p camel-config --lib serde_boundary` (place the test where TracerConfig deserialization is exercised — camel-core or camel-config, wherever the Deserialize impl lands).

**Acceptance:**
- `cargo test -p camel-config --lib` exits 0.
- The `(otel=true, explicit-set=true, enabled=false) → false` row (step-1 tuple `(true,*,true,false)→false`) exists and passes.
- `cargo clippy -p camel-config --all-targets -- -D warnings` exits 0.

- [x] 1.3

## camel-test

### Task 1.4: Integration tests (3 service modes, late registration, composition) + CONTEXT.md

**Files:**
- `crates/camel-test/tests/metrics_wiring_test.rs` (new)
- `crates/camel-test/Cargo.toml` (verified — `reqwest` already in `[dev-dependencies]`; no change expected, listed so the worker does not hunt)
- `crates/camel-api/CONTEXT.md` (modified)

**Steps:**
1. Create the integration test file with a `RecordingCollector` (`Arc<Mutex<Vec<String>>>`, implements all 7 `MetricsCollector` methods by pushing `"method:route"` strings) and a `RecordingLifecycle` (implements `Lifecycle`, `as_metrics_collector` returns the recording collector — mirrors how PrometheusService/OtelService register via `lifecycle.rs:52`).
2. **Prometheus port pre-allocation** (the service is constructed inside `configure_context`; bind `127.0.0.1:0` leaves the port undiscoverable): bind a `std::net::TcpListener` to `127.0.0.1:0`, read the port, drop the listener, write that port into the TOML `port =` field (bind/drop precedent: crates/services/camel-prometheus/src/server.rs; poll-GET precedent: crates/services/camel-function/tests/protocol.rs).
3. **prom-only mode test**: `CamelConfig` from TOML with only `[observability.prometheus]` enabled (pre-allocated port) and a trivial route whose error leg is `to:direct:missing` (proven error path, crates/components/camel-direct/src/lib.rs:337, no network — seda auto-creates queues and swallows the error); `configure_context`; `ctx.start()`; run one exchange through the failing route; poll `GET http://127.0.0.1:{port}/metrics` with reqwest until non-empty; assert BOTH present in the body: (a) an exchange-disposition family (pipeline path), (b) a component error family.
4. **otel-stand-in mode test**: same route/config but no prometheus; `configure_context` then `ctx.with_lifecycle(RecordingLifecycle)` POST-build (the faithful mirror — the real otel service registers post-build at context_ext.rs:608, not at builder time); run one exchange with an error; assert the recording collector observed `increment_exchanges` AND `record_exchange_duration` AND (component path) `increment_errors`.
5. **both-registered composition test**: prometheus enabled AND `RecordingLifecycle` registered after it (configure_context order); run one exchange; assert recording collector observed the exchange AND the prometheus `/metrics` body is non-empty (both collectors live).
6. **late registration test**: `configure_context` with no observability; add routes; `ctx.with_lifecycle(RecordingLifecycle)` on the BUILT context; run one exchange; assert recording observed.
7. `crates/camel-api/CONTEXT.md`: add glossary entries for `MetricsHandle` (late-bound ArcSwap cell, one per context, register composes, same-Arc registration is idempotent) and `CompositeMetricsCollector` (fan-out, composition-not-replacement); update the posture counts; re-run `cargo xtask lint-context-citations` until CITATIONS_PASS.

**Tests:**
- `prometheus_only_emits_pipeline_and_component_metrics`: TOML prom-only (pre-allocated port) + `to:direct:missing` failing route → reqwest GET `/metrics` body contains an exchange-disposition family AND a component error family. Command: `cargo test -p camel-test --test metrics_wiring_test prometheus_only`. Expected: pass (REQUIRES Task 1.3's gate fix — without it this test is the rc-685y red test).
- `otel_stand_in_emits_pipeline_and_component_metrics`: recording lifecycle → assert `increment_exchanges`, `record_exchange_duration`, `increment_errors` all observed. Command: `cargo test -p camel-test --test metrics_wiring_test otel_stand_in`.
- `both_registered_composes`: prometheus + recording (second) → recording observed AND prometheus output non-empty. Command: `cargo test -p camel-test --test metrics_wiring_test both_registered`.
- `late_registration_after_routes_observed`: service added after routes → exchange → recording observed. Command: `cargo test -p camel-test --test metrics_wiring_test late_registration`.

**Acceptance:**
- `cargo test -p camel-test --test metrics_wiring_test` exits 0 (all 4 tests).
- `cargo xtask lint-context-citations` passes for camel-api CONTEXT.md.
- `cargo xtask schema --check` exits 0.
- Full gate sweep from repo AGENTS.md QUALITY GATES green (fmt, clippy ×3, lints, hex boundaries).

- [x] 1.4
