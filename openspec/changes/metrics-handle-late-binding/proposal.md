# Proposal: metrics-handle-late-binding

## Why

Three independent `Arc<dyn MetricsCollector>` snapshots are taken at three
different times, and the one components actually read is taken too early:

- `CamelContext.metrics` (`crates/camel-core/src/context.rs`) — set by
  `with_lifecycle` when a service registers.
- `DefaultRouteController.tracer_metrics`
  (`crates/camel-core/src/lifecycle/adapters/route_controller.rs:81`, seeded
  from `config.metrics_collector` at `:414`) — this is the ONLY path
  components receive (`ControllerComponentContext` → `RuntimeObservability`,
  consumed at route_controller.rs:460/633 and
  route_controller_trait.rs:306/761 via `unwrap_or_else(NoOpMetrics)`).
- The `RuntimeBus` collector (`crates/camel-core/src/context_builder.rs:136`)
  — permanently `NoOp` in the `configure_context` path because the context is
  built before any service registers.

In `configure_context` (`crates/camel-config/src/context_ext.rs:612`), the
tracer config snapshot is taken at line 612 — BEFORE prometheus registers at
`:615-636`. Consequences (expert ruling N1, e_opus, supersedes rc-cizb):

- **prometheus-only mode**: dashboard-blind for pipeline AND every component
  error metric (kafka 7 sites, redis 6, http 4 — all write into `NoOp`).
- **otel+prometheus**: otel wins the route path (registers first, snapshotted
  first); prometheus overwrites a slot nothing reads — `/metrics` registry
  empty (dead write, not precedence).
- A second registration REPLACES the first instead of composing.

Additionally (rc-685y, P1, same epic): `effective_tracer_config`
(`context_ext.rs:944-948`) only forces `tracer_config.enabled = true` when
`otel_enabled`. In prometheus-only mode the tracer pipeline stays disabled, so
disposition counters (exchanges/errors/duration) are never emitted even to a
correctly wired collector. This gate is a hard dependency of the prom-only
acceptance test and is folded into this change.

## What Changes

- ADD `MetricsHandle { inner: ArcSwap<dyn MetricsCollector> }` to
  `camel-api/src/metrics.rs`; it implements `MetricsCollector` by delegating
  via `inner.load()` (NOT `load_full` — no Arc clone on the hot path).
- Build the handle ONCE in `CamelContextBuilder::build()`; hand the SAME Arc
  to all three consumers (context slot, controller `tracer_metrics`, RuntimeBus
  collector). Seed `tracer_metrics` at build so the
  `unwrap_or_else(NoOpMetrics)` fallbacks become unreachable in production.
- `with_lifecycle` registers into the handle (compose; same-Arc registration
  is idempotent) instead of field reassign.
- Second registration composes
  `CompositeMetricsCollector(Vec<Arc<dyn MetricsCollector>>)` instead of
  replacing (~30 lines once the handle exists). Each impl tolerates its own
  failures; no error aggregation (trait returns `()`).
- DELETE `TracerConfig.metrics_collector` and its injection
  (removes snapshot semantics; also kills the redacting-Debug need at
  `crates/camel-core/src/shared/observability/domain/config.rs:27-38`).
- `effective_tracer_config` gains a `prometheus_enabled: bool` parameter;
  when prometheus is enabled and tracing is not explicitly disabled, the
  tracer pipeline is enabled (rc-685y).
- `arc_swap` is already a workspace dependency — it must be ADDED to
  camel-api's `Cargo.toml` (`arc-swap = { workspace = true }`); no new
  external dep. RwLock rejected (lock on hot path); `OnceLock` rejected
  (first-wins, cannot compose).

**Included:** rc-hrm1.2 (P0 fix), rc-hrm1.3 (controller-path regression test,
gates this merge), rc-685y (prometheus-only pipeline gate).
**Excluded:** rc-hrm1.4 (retry-sequence error semantics), rc-hrm1.1
(Camel.toml levers), rc-hrm1.6/1.7 (gauges), rc-hrm1.8 (ADR) — later epic
children; rc-q25t component-emission sweep.

## Acceptance criteria

- Prometheus-only, otel-only, and prometheus+otel integration tests each
  assert non-empty metric output from BOTH the pipeline path and a component
  error path.
- A test registers a lifecycle metrics service AFTER routes are added and
  still observes emission (late binding works).
- A mirrored assertion covers the `ControllerComponentContext` →
  `RuntimeObservability` path a component actually receives (pairs with, does
  not replace, the existing `CamelContext` slot test).
- Two registered collectors both receive emissions (composition, not
  replacement).
- `TracerConfig.metrics_collector` no longer exists; no `unwrap_or_else(NoOp)`
  fallback is reachable in production paths.
- Full quality gates green (fmt, clippy ×3, lints, schema, hex boundaries).

## Risk budget

- Acceptable: mechanical churn at ~6 construction sites of `RuntimeBus` /
  controller structs; trait-object `ArcSwap` adds one atomic load per metric
  call (negligible vs I/O); adding `arc-swap` to camel-api's Cargo.toml from
  the workspace dep set.
- **Intentional behavior change**: an explicit `[observability.tracer]
  enabled = false` now wins over otel enablement (today otel force-enables
  over it). Covered by the config unit-test matrix.
- Out of bounds: changing the `MetricsCollector` trait shape (5 required + 2
  defaulted methods stay as-is); any behavior change to otel tracer
  registration order; new external dependencies.
