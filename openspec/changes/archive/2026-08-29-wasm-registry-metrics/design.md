# Design: wasm-registry-metrics

## Approach

One surgical wiring fix, mirroring the Task 4.1
`ControllerComponentContext` seam pattern from dashboard-observability.

Wasm-surface observability topology (three handles, honesty first):
(a) route-compiled producers — `ControllerComponentContext`, LIVE;
(b) guest-initiated producers — hardcoded `NoOpComponentContext`
(host_functions.rs:112-113), dead, OUT OF SCOPE here (tracked as a
separate bd issue during implementation); (c)
`RegistryComponentContext` itself — LATENT dead facade (zero consumers
today), the target of this change.

1. `RegistryComponentContext`
   (crates/camel-core/src/shared/components/domain/registry.rs) gains
   two fields:
   - `metrics: Arc<dyn MetricsCollector>` — resolved once at
     construction (`metrics.unwrap_or_else(|| Arc::new(NoOpMetrics))`);
   - `components_enabled: bool` — lever snapshot.
2. `new(registry, metrics: Option<Arc<dyn MetricsCollector>>,
  components_enabled: bool)` replaces `new(registry)`. Every call site
   becomes explicit — no constructor path silently reintroduces
   rc-66he for future callers.
   Rejected alternatives:
   - keep `new(registry)` plus a builder `with_metrics(...)`: the
     default path still wires NoOp silently; explicitness is the fix.
   - a separate `RuntimeObservability` impl: unnecessary — the blanket
     `impl<T: ComponentContext> RuntimeObservability for T`
     (camel-component-api runtime_observability.rs) already derives
     `component_metrics()` from `metrics()` +
     `component_metrics_enabled()`; overriding the two
     `ComponentContext` methods is sufficient.
3. camel-cli `run.rs`, both sites: thread `Some(ctx.metrics())` and the
   lever via the `ComponentContext` trait method on `CamelContext`
   (`component_metrics_enabled()` reads the `metrics_levers` snapshot
   taken at `set_tracer_config`).
   - Site 1 (WasmBean load, ~:235) runs after
     `configure_context_with_beans` (:135) and `ctx.registry_arc()`
     (:170), so `ctx` is in scope. The bean consumes no metrics today
     (bean.rs uses the registry solely for `resolve_component`);
     threading the handle is uniformity for future consumers (bean
     drain-latency hooks) — no bean-emission test is authored.
   - Site 2 (WasmBundle register, ~:490) already holds `ctx`.
   Late-binding note: `CamelContext::metrics()` (context.rs:658)
   returns the `MetricsHandle` — ArcSwap-backed and itself a
   `MetricsCollector` — so storing that Arc is NOT a backend snapshot;
   collectors registered by later `with_lifecycle` calls still reach
   registry-resolved components (ADR-0066, registration order
   irrelevant).
4. camel-cli `security.rs:412`
   (`build_security_compile_context_from_config`): `None, false` with
   a comment — compile-time policy scan, no route runtime, no wired
   collector exists there. Behavior identical to today.
5. Tests live in the registry.rs test module: recording-collector
   double asserting identity (same Arc) and lever propagation; NoOp
   fallback for `None`; `resolve_component` unaffected regression.

## Affected crates

- camel-core: `RegistryComponentContext` struct fields, constructor
  signature, two trait-method overrides, unit tests. No other camel-core
  surface touched.
- camel-cli: three construction sites (run.rs ×2 wired, security.rs ×1
  explicit `None`) under the existing `#[cfg(feature = "wasm")]`
  gates.

## Architecture boundaries

Registry stays a domain adapter inside camel-core. The adapter crosses
the camel-component-api `ComponentContext` port with an injected
collector — the `Arc<dyn MetricsCollector>` port comes from camel-api,
so no service-layer dependency enters camel-core. camel-cli is the
composition root that owns the wiring decision, same as the controller
path is wired in camel-core lifecycle adapters from context state. No
DSL, Services, Languages, or Functions impact. Context citations:
ADR-0066 (collector binding/lifetime), ADR-0012 (error family
non-disableable).

<!-- Single-phase change: no Phases section here, no `## Phase N`
     headings in tasks.md. -->
