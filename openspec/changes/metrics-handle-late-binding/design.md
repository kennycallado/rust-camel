# Design: metrics-handle-late-binding

## Approach

Single mechanism: one shared, late-bound cell replaces three eager snapshots.

`MetricsHandle` wraps `ArcSwap<dyn MetricsCollector>`. It is created ONCE in
`CamelContextBuilder::build()` seeded with `NoOpMetrics`, and the SAME `Arc`
is handed to:

1. the `CamelContext` slot (replaces `self.metrics` field),
2. `DefaultRouteController.tracer_metrics` (seeded at build — the
   `unwrap_or_else(NoOpMetrics)` fallbacks in route_controller.rs:460/633 and
   route_controller_trait.rs:306/761 become unreachable because the Option is
   always `Some(handle)`),
3. the `RuntimeBus` collector slot (context_builder.rs:136).

`MetricsHandle` itself implements `MetricsCollector`; every method delegates
with `self.inner.load()` — a lock-free atomic load, no `Arc` clone, no
`load_full` (which would pin the old collector across swaps). Reads are
sequentially consistent per `arc_swap` defaults; a swap is only visible to
calls that start after it, which is exactly the intended semantics for
service registration.

Registration (`with_lifecycle` and the builder's `as_metrics_collector`
branch) becomes `handle.register(new)` which builds
`CompositeMetricsCollector(vec![prev, new])` on a second registration. Same-Arc
registration is idempotent (parallel membership list on the handle, `Arc::ptr_eq`
against every accepted member) — the `.metrics(a)` + `.with_lifecycle(exposing-a)` double-register
pattern exists in tests today.
`CompositeMetricsCollector` is `#[doc(hidden)]`-grade internal API in
camel-api: iterate collectors, call each. No panic containment is attempted
(trait methods cannot fail — each impl already tolerates its own failures
internally: prometheus tombstones, otel started-gate).

Why not the alternatives (per ruling N1):

- **RwLock**: read lock on the metric hot path; contention under concurrent
  exchanges.
- **OnceLock**: first-wins — cannot compose a second collector.
- **Field reassign + rebuild**: what exists today; snapshot staleness is the
  bug.
- **`load_full()`**: clones the `Arc` every call; also holds a pinned old
  collector if swapped mid-flight.

`TracerConfig.metrics_collector` is DELETED (field + injection at
context.rs:474-479 + redacting Debug at config.rs:27-38). Direct
programmatic injection migrates to the handle: builder `.metrics()` setter
now stores into the handle at build time. This removes the third injection
path that created the snapshot semantics in the first place.

rc-685y: `effective_tracer_config(tracer_config, otel_enabled)` gains two
parameters: `prometheus_enabled: bool` and
`tracing_explicitly_set: bool`. `TracerConfig.enabled` is `#[serde(default)]
bool`, so "absent" and "explicit false" deserialize identically — explicit-set
MUST be detected from raw table presence (the `[observability.tracer]`
table with an `enabled` key exists in the source config) in `configure_context`
and passed in; `TracerConfig` keeps its `bool` shape. New truth table:

| otel | prometheus | tracing explicitly set? | pipeline `enabled` |
|------|------------|-------------------------|--------------------|
| any  | any        | yes (either value)      | explicit value wins |
| on   | any        | no                      | forced true        |
| off  | on         | no                      | forced true (rc-685y) |
| off  | off        | no                      | config default (false) |

**Intentional behavior change:** today otel force-enables the pipeline even
over an explicit `enabled = false` (context_ext.rs:943-946). After this
change the explicit value wins in both directions. The config unit-test
matrix MUST include `otel × explicit-false` (old: true, new: false).

"Explicitly set" = the `[observability.tracer]` table present in the config
source with an `enabled` key.

## Affected crates

- **camel-api**: `metrics.rs` — add `MetricsHandle`, `CompositeMetricsCollector`;
  re-export from `lib.rs`.
- **camel-core**: `context.rs` (field type + `with_lifecycle` store),
  `context_builder.rs` (build once, seed controller + bus, compose on second
  register), `lifecycle/adapters/route_controller.rs` (`tracer_metrics` type
  stays `Option<Arc<dyn MetricsCollector>>`, seeded `Some(handle as Arc<dyn
  MetricsCollector>)` at both `build()` construction branches — the
  beans/no-beans constructor variants converge into one `controller_impl` per
  branch, and the inner `tracer_metrics: None` init sites are test-only; the
  `set_tracer_config` metrics line is DELETED with the field — the
  `unwrap_or_else(NoOp)` fallbacks become unreachable), `lifecycle/adapters/route_controller_trait.rs` (fallbacks
  unchanged, now unreachable), `shared/observability/domain/config.rs`
  (delete `metrics_collector` field + redacting Debug). NOT touched:
  `route_compiler_ext.rs` (consumes the coerced `Arc<dyn MetricsCollector>`,
  zero churn).
- **camel-config**: `context_ext.rs` — `effective_tracer_config` signature +
  call site passes prometheus flag.
- **camel-prometheus / camel-otel**: no source change (they register via
  `with_lifecycle` / service trait, which now composes).
- **camel-test**: integration tests (prom-only / otel-only / both / late
  registration / composition) + the rc-hrm1.3 mirrored controller-path
  assertion.

## Architecture boundaries

- Hexagonal: `MetricsHandle` is a domain-level infrastructure port adapter in
  camel-api (same layer as `MetricsCollector` itself — no upward dependency).
  camel-config orchestrates; camel-core consumes; components unchanged (they
  already receive `RuntimeObservability`).
- Public-type discipline: `MetricsHandle` exposes only `new()` (seeded NoOp) and
  `register()` as mutation. New symbols documented in camel-api CONTEXT.md
glossary.
- No DSL change; no component API change; config schema gains no new keys
  (rc-hrm1.1 levers are out of scope).

## Phases (optional)

Single-phase: the mechanism (handle + seeding + composition) is one atomic
slice — a phased split would leave an intermediate state with two wiring
disciplines coexisting. Task order inside the phase still runs
test-first (rc-hrm1.3 red test lands with the wiring task that turns it
green).

## Test strategy

- **Unit (camel-api)**: handle delegates to stored collector; second store
  composes (both collectors observe); `CompositeMetricsCollector` fan-out.
- **Regression (camel-core)**: rc-hrm1.3 —
  `ControllerComponentContext → RuntimeObservability` returns a non-NoOp
  collector when a lifecycle service registered one (pairs with existing
  context slot test at context_tests.rs:1271-1278).
- **Config unit (camel-config)**: `effective_tracer_config` truth table
  (4 rows × explicit/unset).
- **Integration (camel-test)**: three service-mode tests asserting non-empty
  output from BOTH pipeline path and component error path; late-registration
  test (service added AFTER routes); composition test (two collectors, both
  observe). Stub-collector strategy per established pattern (record calls into
  `Arc<Mutex<Vec>>`, assert content, no real network).
