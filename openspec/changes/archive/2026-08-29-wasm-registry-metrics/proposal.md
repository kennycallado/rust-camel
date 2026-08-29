# Proposal: wasm-registry-metrics

## Why

rc-66he — last open child of epic rc-hrm1. `RegistryComponentContext`
(camel-core, shared/components/domain/registry.rs) hard-codes
`NoOpMetrics` in `ComponentContext::metrics()` (a fresh allocation per
call) and never overrides `component_metrics_enabled()` (trait default
false).

Today this facade is LATENT: route-compiled wasm producers already emit
through the live `ControllerComponentContext` seam
(route_compiler_ext.rs), and `RegistryComponentContext::metrics()` has
zero current consumers. But the blanket
`impl<T: ComponentContext> RuntimeObservability for T` derives
`component_metrics()` from exactly these two methods, so any future
consumer resolved through the registry context (wasm bundle assembly,
bean paths) inherits a dead facade — the non-disableable ADR-0012
error family and the uniform component-operations family both
silently discard — plus a wasted `Arc::new(NoOpMetrics)` allocation on
every call.

## What Changes

- `RegistryComponentContext::new` takes explicit observability params:
  `metrics: Option<Arc<dyn MetricsCollector>>` and
  `components_enabled: bool`. The resolved collector Arc is stored once
  (removes today's per-call `Arc::new(NoOpMetrics)` allocation).
- `ComponentContext::metrics()` returns the stored collector (NoOp
  fallback only when constructed with `None`);
  `component_metrics_enabled()` returns the stored lever snapshot, so
  the blanket `RuntimeObservability` impl builds a correctly gated
  facade.
- camel-cli `run.rs` sites (WasmBean load + WasmBundle register) thread
  `Some(ctx.metrics())` — the late-bound MetricsHandle per ADR-0066,
  not a backend snapshot — plus the context's lever snapshot. The
  WasmBean site consumes no metrics today (bean.rs resolves components
  only); threading there is uniformity for future consumers (e.g. bean
  drain-latency hooks) — no bean-emission test is authored.
- camel-cli `security.rs` (compile-time policy scan, no route runtime)
  passes `None, false` — documented as intentional.
- Excluded: end-to-end wasm emission proof under a live wasm runtime
  (rc-xlo0, P3, CI infra); Registry lookup semantics; any trait-surface
  change. Also excluded — and tracked as a separate defect during
  implementation — the OTHER dead wasm path: guest-initiated producers
  hard-code `rt = NoOpComponentContext` (host_functions.rs:112-113),
  which this change does not touch.

## Acceptance criteria

- A `RegistryComponentContext` constructed with a collector returns
  that collector from `metrics()` and reports the stored lever from
  `component_metrics_enabled()` — unit-tested with a recording
  collector double.
- Constructed without a collector, behavior equals today's NoOp
  semantics (security path unchanged) — unit-tested.
- camel-cli compiles with all three construction sites explicit; no
  silent-NoOp constructor remains in the workspace.
- ADR-0066 late binding preserved: the threaded Arc is the
  MetricsHandle, so a collector registered by later `with_lifecycle`
  calls still reaches registry-resolved components.

## Risk budget

Small blast radius: one camel-core adapter struct, three camel-cli
call sites, zero trait changes, zero new dependencies, no hot-path cost
(one Arc clone per `metrics()` call, same class as every other
context). The breaking change to `RegistryComponentContext::new` is
workspace-internal (all callers swept in this change). Out of bounds:
Registry resolution internals, wasm runtime behavior, collector trait
surface, prometheus/otel backends.
