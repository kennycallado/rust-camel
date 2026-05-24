# Services

Cross-cutting infrastructure services that hook into the Runtime lifecycle. Covers observability (OpenTelemetry, Prometheus) and platform integration (Kubernetes). Note: `camel-platform-kubernetes` lives in `crates/platforms/` but belongs to this context.

## Language

**Lifecycle**:
Contract for services that need coordinated start/stop with CamelContext. Implementations register via `with_lifecycle(...)` and are started/stopped automatically.
_Avoid_: plugin, hook, service (unqualified)

**MetricsCollector**:
Abstraction for recording route and exchange metrics. The Prometheus implementation is provided; custom implementations can be registered.
_Avoid_: metrics sink, reporter, exporter

**PlatformService**:
Abstraction over the deployment environment. Provides identity and cluster-aware behavior to the Runtime (e.g., Kubernetes pod metadata, readiness signaling).
_Avoid_: cloud provider, environment, infrastructure

**PlatformIdentity**:
The identity of the deployment: `node_id` (required), `namespace` (optional), and `labels`. Provided by the PlatformService implementation.
_Avoid_: pod identity, service identity, node identity

**ReadinessGate**:
Push-model contract for signaling deployment readiness to infrastructure. Implementations
proactively patch Kubernetes readiness conditions based on the result of `HealthSource::readiness()`.
Not the same as the HTTP `/readyz` endpoint — `ReadinessGate` pushes to the orchestrator,
`HealthSource` answers pull queries from HTTP clients. Both derive from the same underlying truth.
_Avoid_: health check, liveness probe, readiness probe, HealthSource (these are distinct)

**HealthSource**:
Pull-model contract that expresses the Runtime's health state on demand. `liveness()` checks only
Runtime lifecycle (no network I/O); `readiness()` aggregates all registered `AsyncHealthCheck`
results via `HealthCheckRegistry`. Implemented by `CamelContext` and consumed by `camel-health`
HTTP endpoints (`/healthz`, `/readyz`) and by `ReadinessGate` implementations.
_Avoid_: ReadinessGate (push vs pull are different), health checker (the old sync type)

**AsyncHealthCheck**:
Per-component probe that tests a live external connection and returns a `CheckResult`. Only
components with external connections implement this trait. Registered into `HealthCheckRegistry`
when a Consumer starts; unregistered when it stops.
_Avoid_: health monitor, liveness check (liveness is Runtime-only, no network)

**CheckResult**:
The outcome of a single `AsyncHealthCheck` — a name, a `HealthStatus`, and an optional message.
Distinct from `HealthReport`, which is the aggregated result across all checks.
_Avoid_: HealthReport (use CheckResult for per-component, HealthReport for aggregate)

**HealthReport**:
The aggregated health result returned by `HealthCheckRegistry.check_all()` and exposed on the
`/health` endpoint. Combines all `CheckResult` entries: any `Unhealthy` dominates; else any
`Degraded` dominates; else `Healthy`.
_Avoid_: CheckResult (use HealthReport only for the aggregate)

**Healthy**:
`HealthStatus` state indicating the component satisfies its contract with normal redundancy and
capacity. No action required by operators.
_Avoid_: ok, up, alive (use Healthy)

**Degraded**:
`HealthStatus` state indicating the component can process Exchanges but with reduced redundancy,
capacity, or safety margin measured by a component-specific objective threshold. The pod stays
`Ready` — Kubernetes keeps routing work — but operators should alert or inspect. Only use for
measurable partial-capability; never for vague conditions like "slow" or "suspicious".
_Avoid_: warning, partially unhealthy, NotReady (Degraded is readiness-positive)

**Unhealthy**:
`HealthStatus` state indicating the component cannot process matching Exchanges within the
health-check timeout, or shutdown makes success impossible. The pod goes `NotReady` — Kubernetes
stops routing new work. Use when connection is refused, pool is exhausted, server is not bound,
or any condition that makes Exchange processing impossible.
_Avoid_: Degraded (if exchanges can still be processed, it is Degraded, not Unhealthy)

**ForcedHealthFailure**:
A `HealthCheckRegistry` entry pinned to `Unhealthy` by the Runtime control plane when a Route
enters `Failed` state via `FailRoute`. Replaces the route's live `AsyncHealthCheck` entry without
requiring `Consumer::stop()` — which is never called on a crashed Consumer. Cleared only when
ConsumerRestart registers a new live check for the same `route_id`, or when the Route is removed.
Prevents the worst-case outcome: a dead Route whose health entry reports `Healthy` by omission.
_Avoid_: unregister (a ForcedHealthFailure is not removed — it stays Unhealthy until restart)


Contract for distributed leader election. Returns a `LeadershipHandle` with a channel for receiving leadership state changes.
_Avoid_: elector, leader lock, master election

## Example dialogue

> "How does the Runtime know it is running on Kubernetes?"
> "Register a KubernetesPlatformService as the PlatformService. It auto-detects PlatformIdentity from the downward API. When given a HealthSource, it polls readiness() and patches Kubernetes readiness conditions accordingly."
>
> "What is the difference between /readyz and a Kubernetes readiness condition?"
> "Both express the same truth — HealthSource::readiness() — but through different models. /readyz is pull: the kubelet HTTP probe queries it. Kubernetes readiness conditions are push: the KubernetesPlatformService patches them when readiness changes. Without the HealthSource integration, they diverge."
>
> "Do I need OTel and Prometheus both?"
> "No — each is a separate Lifecycle implementation. Register only what you need. If you register neither, the Runtime still works without traces or metrics exported."
