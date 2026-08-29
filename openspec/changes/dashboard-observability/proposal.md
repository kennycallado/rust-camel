# Proposal: dashboard-observability

## Why

The 2026-08-26 metrics audit (epic rc-hrm1) found that even with collector
wiring fixed (metrics-handle-late-binding, merged 2026-08-27), dashboards
remain structurally blind:

1. **Error semantics inflate alerts** — ADR-0013 `NetworkRetryPolicy` sites
   call `increment_errors` per attempt, so one exhausted 5-attempt sequence
   counts 5 errors; open circuit breakers route full request rate into
   `camel_errors_total{error_type="circuit_open"}`, masking every real error
   (rulings N3/N4).
2. **No success-path or inventory signal exists** — every component-level
   `MetricsCollector` call in the workspace is `increment_errors` on failure
   paths; there is no way to tell a healthy route from a dead one, no route
   state gauge, no build/uptime info, and `set_queue_depth` has zero
   production callers (rc-6s6h, rc-q25t, rc-bfnw; rulings N5/N6).
3. **Metrics cannot be configured independently of tracing** — the only
   lever is `tracer.enabled`, which per ruling N7 gates spans ONLY.

## What Changes

Five delivery phases in one change:

- **Phase 1 — error semantics (rc-hrm1.4, rc-hrm1.5)**: exactly one
  `increment_errors` per exhausted retry sequence (never per attempt);
  per-attempt telemetry on new `camel_retry_attempts_total{scheme,operation}`;
  circuit-breaker rejections counted on new
  `camel_circuit_breaker_rejections_total{route}` and excluded from
  `camel_errors_total`.
- **Phase 2 — metrics levers (rc-hrm1.1)**: independent
  `[observability.metrics]` switch decoupled from `tracer.enabled`; opt-in
  `[observability.metrics.components]`; ADR-0012 error family remains the
  only non-disableable family.
- **Phase 3 — inventory and backpressure emissions (rc-hrm1.6,
  rc-hrm1.7)**: `camel_route_state{route,state}` gauge from
  `RouteStatusProjection`; `camel_build_info{version,git_sha}` and
  `camel_uptime_seconds`; `set_queue_depth` wired in SEDA, aggregator,
  resequencer (ADR-0044 backpressure signal).
- **Phase 4 — component emission sweep (rc-6s6h, rc-q25t, rc-bfnw)**:
  success-path counter family for components; dead-observability components
  (wasm, opensearch, cxf remainder, seda, surrealdb remainder per the
  re-verified 2026-08-27 audit) emit at least errors + success-path counts.
- **Phase 5 — closure (rc-hrm1.8, rc-hrm1.9)**: ADR for collector binding
  and lifetime contract (amends ADR-0012); xtask lint enforcing that metric
  label values are closed sets (literal or `OptionKind::Enum` metadata per
  ADR-0041).

## Impact

- **Capabilities**: three new spec capabilities — `component-error-semantics`,
  `metrics-configuration`, `component-metrics-emission` (delta specs below).
- **Crate surfaces**: camel-api (MetricsCollector trait methods:
  `increment_retry_attempt`, `increment_circuit_breaker_rejection`,
  `set_route_state`, `record_build_info`, `record_uptime`; NoOp +
  Composite delegation), camel-prometheus (new metric registration),
  camel-core (tracer adapter error classification, route-state feed),
  camel-config (levers) + docs/src/configuration/schema.md, components (kafka/redis/http/cxf retry
  sites; seda/aggregator/resequencer queue depth; dead-observability sweep),
  camel-processor (circuit_breaker.rs), scripts/xtask (label-domain lint),
  docs (ADR).
- **Risk**: trait addition is additive (default methods keep out-of-tree
  implementors compiling); `camel_errors_total` semantic change is the
  headline user-visible difference and must be called out in the merge
  commit; per-family opt-out must never disable the ADR-0012 error family.
