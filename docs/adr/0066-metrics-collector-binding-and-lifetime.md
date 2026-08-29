# ADR-0066: Metrics Collector Binding and Lifetime Contract

**Date:** 2026-08-28
**Status:** Accepted
**Origin:** bd rc-hrm1.8 (epic rc-hrm1), expert ruling N7
**Amends:** ADR-0012
**Cross-refs:** ADR-0012, ADR-0013, ADR-0041, ADR-0044, ADR-0045, ADR-0052

## Context

The 2026-08-26 metrics audit (epic rc-hrm1) found two structural defects
in collector wiring. First, `with_tracer_config` snapshotted a collector
before backend lifecycle registration. A prometheus-only context captured
`NoOpMetrics` and exported nothing (rc-cizb). Second, the first-registered
OTel collector won the route path. A later Prometheus registration
overwrote a `CamelContext.metrics` slot that no route or component read.

The fix landed as metrics-handle-late-binding (8c876d54): one
`MetricsHandle` per context, backed by `ArcSwap`, resolved after backend
registration. This ADR pins that contract and the lifetime rules that
follow from it. It also records the error-accounting amendments to
ADR-0012 that the dashboard-observability change introduces.

## Decision

### Decision 1: one late-bound collector slot per context

Each `CamelContext` owns exactly one `MetricsHandle`
(`crates/camel-api/src/metrics.rs:110`). The handle is an
`ArcSwap<CollectorSlot>` that seeds itself with `NoOpMetrics`. Consumers
may hold the handle before any real collector exists. Calls before
registration are safe no-ops. The handle is resolved after backend
registration. This supersedes the collector snapshot inside
`set_tracer_config` that produced rc-cizb.

### Decision 2: registration composes; order is irrelevant

`MetricsHandle::register` composes the new collector over the stored one
via `CompositeMetricsCollector` (`crates/camel-api/src/metrics.rs:132`,
`:240`). Registration never replaces. The same `Arc` registered twice is
a no-op (`Arc::ptr_eq` dedupe), so a call site that wires one collector
through two builder paths does not double-count. Composition is
order-independent, so registration order is irrelevant.

### Decision 3: multi-backend fan-out composes

OTel and Prometheus registered simultaneously both receive every
emission. No collector silently wins the route path. The composite
delegates every trait method to each member, including the
error-semantics methods added by dashboard-observability
(`increment_retry_attempt`, `increment_circuit_breaker_rejection`).

### Decision 4: tracer.enabled gates spans ONLY

`[observability.tracer] enabled` controls span creation only. It does
not gate metric emission. A prometheus-only context (tracer off) still
collects route and component metrics. Prometheus enablement implies the
tracer pipeline, so the non-disableable error family is always exported
(`effective_tracer_config`, `crates/camel-config/src/context_ext.rs:962`).

### Decision 5: the error family is the only non-disableable family

The ADR-0012 error family (`MetricsCollector::increment_errors`,
`crates/camel-api/src/metrics.rs:10`) is always emitted. No
`[observability.metrics]` lever can disable it. The exchange counter,
duration histogram, and component-operations families are gateable
independently. The pipeline tracer never gates `increment_errors`
(`crates/camel-core/src/shared/observability/adapters/tracer.rs:159`).

### Decision 6: retry accounting (amends ADR-0012)

One `increment_errors` per exhausted `NetworkRetryPolicy` sequence,
executed by the policy helpers themselves (`retry_async`,
`crates/components/camel-component-api/src/network_retry.rs:245`;
`retry_async_cancelable`, `:349`). Call sites do not increment on their
own `Err` arm for attempts the helper retries. Cancellation is not
failure: a cancelled sequence emits no error. Per-attempt telemetry
lives on `camel_retry_attempts_total{scheme,operation}`.

### Decision 7: breaker rejections are not errors (amends ADR-0012)

Open-breaker fast-fails (`CamelError::CircuitOpen`, classified
`"circuit_open"`, `crates/camel-api/src/error.rs:174`) count on
`camel_circuit_breaker_rejections_total{route}`. The pipeline tracer
skips `increment_errors` for `error_class == CIRCUIT_OPEN`
(`crates/camel-core/src/shared/observability/adapters/tracer.rs:178-179`).
Callers still receive `CamelError::CircuitOpen` unchanged. Only metric
routing changes.

### Decision 8: rejection-counter unit is readiness probes

The rejection counter counts readiness probes, not logical sends. A
parked caller retries `poll_ready` on backoff, so one open breaker
produces about one rejection per backoff interval per parked send. The
unit is pinned: `camel_circuit_breaker_rejections_total` measures probe
pressure, not dropped work. Alerting must scale thresholds by the probe
rate, not by the send rate.

### Decision 9: helper-owned exhaustion errors use the operation label

Helper-owned exhaustion errors place the OPERATION in the first-arg
(route) label position of `camel_errors_total`. The helper has no route
scope, so the shape is `increment_errors(operation,
"e:{scheme}:{operation}")`. Dashboards keying on route must expect
component-operation pseudo-routes there, for example
`e:container:events-connect`.

### Decision 10: registration-order contract for gauges

Identity gauges (build info, uptime) fire pre-registration at build
time. They are re-published on registration, so a late-registered
collector still sees them. Route-state is transition-driven and does NOT
replay to collectors registered after transitions fired. The canonical
path registers pre-start (`configure_context`) and is e2e-proven.
Post-start registration shows uptime without route inventory until the
next transition. Queue-depth self-heals via 250ms samplers.

### Decision 11: sampling cadence

| Series | Cadence |
|---|---|
| `camel_uptime_seconds` | 60s refresh task | `context_builder.rs` `spawn_uptime_refresh` (`from_secs(60)`) |
| SEDA queue depth | 250ms sampler |
| Aggregator queue depth | 250ms sampler |
| Aggregator TTL sweep | `ttl / 2`, minimum 50ms |
| Resequencer queue depth | post-accept only |

The resequencer publishes after each accept. Timeout-release staleness
is a known ceiling: a batch released by gap timeout is not re-sampled
until the next accept.

### Decision 12: double-count contract

Facade failures land on `e:{component}:{operation}`. Retained
component-specific error labels MUST never equal that string. A true
double-count is one series incremented twice for one failure. Dashboards
summing the whole `camel_errors_total` family count each component
failure twice (uniform series plus specific series). This is intended
per D5 and stated here so the 4.2 audit table records it.

### Decision 13: helper and facade label collision

`retry_async` with `metrics = Some` emits `e:{scheme}:{operation}`,
byte-identical to the facade label when `scheme == component` (the
opensearch shape). Today no boundary wires BOTH the helper
(`metrics = Some`) AND the facade: the `Some` helper sites (container
`events-connect`/`logs-connect`, sql pool-init) do not use the facade,
and the facade sites (opensearch, and the rest of the Phase-4 sweep)
pass the helper `None`. A component that adopts the facade at an
operation boundary where `retry_async` runs with `metrics = Some` would
same-series double-count. Wiring BOTH at one boundary is FORBIDDEN.
Choose one error owner per boundary. A component that wants per-attempt
telemetry AND facade outcome semantics must use distinct operation
labels — the shared series cannot carry both.

### Decision 13a: third-backend registration

Registration composes (D2) and fan-out is order-independent, so a third
backend needs no code change: register it and every emission site fans
out to it. Delegation cost grows linearly with member count — one dyn
dispatch per member per call — which is the accepted price of the
multi-backend contract.

### Decision 14: vocabulary asymmetry

| Component | Operations |
|---|---|
| kafka | consume, produce |
| redis | command (producer only) |

Kafka emits both consume and produce. Redis emits command (producer)
only. The redis consumer stays outside the family vocabulary. D5 names
the producer operation only.

### Decision 15: recording-double debt

About ten recording doubles exist across crates, growing per trait
method. Consolidation is bd-tracked (rc-4dvi). This ADR records the
intent: the doubles are known, counted, and scheduled for consolidation,
not silent drift.

## Rejected alternatives

### Per-backend collector slots

Rejected: a slot per backend re-introduces the winner-takes-all bug.
One composed slot is the contract.

### Snapshot at config time

Rejected: this is the rc-cizb failure mode. Late binding is the fix.

## Consequences

### Alert thresholds recalibrate

`camel_errors_total` counts drop by the retry factor and by breaker
rejection rate. Alert thresholds calibrated to inflated counts fire
less. The merge commit states this.

### Trait growth is additive

Five new default methods on `MetricsCollector` keep out-of-tree
implementors compiling. The composite delegates all of them.

### Cardinality is bounded

`camel_component_operations_total` is bounded by closed label sets.
Components opt in with the `components` lever, default off.

## Load-bearing citations

| File:line | Element |
|---|---|
| `crates/camel-api/src/metrics.rs:110` | `MetricsHandle` (ArcSwap slot) |
| `crates/camel-api/src/metrics.rs:132` | `register` composes |
| `crates/camel-api/src/metrics.rs:240` | `CompositeMetricsCollector` |
| `crates/camel-core/src/context.rs:53` | `metrics: Arc<MetricsHandle>` |
| `crates/camel-config/src/context_ext.rs:962` | `effective_tracer_config` |
| `crates/camel-core/src/shared/observability/adapters/tracer.rs:178-179` | circuit_open exclusion |
| `crates/camel-api/src/error.rs:174` | `CIRCUIT_OPEN` |
| `crates/components/camel-component-api/src/network_retry.rs:245` | `retry_async` |
| `crates/components/camel-component-api/src/network_retry.rs:349` | `retry_async_cancelable` |
| `crates/components/camel-container/src/lib.rs:1545-1562` | helper-owned exhaustion |
| `crates/components/camel-kafka/src/consumer.rs:591` | `e:kafka:recv-exhaustion` |
| `crates/components/camel-kafka/src/consumer.rs:386,410` | `("kafka","consume")` |
| `crates/components/camel-kafka/src/producer.rs:282` | `("kafka","produce")` |
| `crates/components/camel-redis/src/producer.rs:21-23` | `("redis","command")` |
| `crates/camel-processor/src/aggregator.rs:371` | TTL sweep `ttl / 2`, min 50ms |
| `crates/camel-processor/src/aggregator.rs:31` | 250ms queue-depth sampler |
| `crates/components/camel-component-seda/src/lib.rs:43` | 250ms queue-depth sampler |
| `crates/camel-processor/src/resequencer/mod.rs:202-211` | post-accept publish |