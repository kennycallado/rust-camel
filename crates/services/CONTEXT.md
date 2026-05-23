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
Contract for signaling deployment readiness state — ready, not-ready, or starting. Implementations report to the infrastructure (e.g., Kubernetes readiness conditions).
_Avoid_: health check, liveness probe, readiness probe

**LeadershipService**:
Contract for distributed leader election. Returns a `LeadershipHandle` with a channel for receiving leadership state changes.
_Avoid_: elector, leader lock, master election

## Example dialogue

> "How does the Runtime know it is running on Kubernetes?"
> "Register a KubernetesPlatformService as the PlatformService. It auto-detects PlatformIdentity from the downward API and signals readiness via a readiness gate when the Runtime is ready."
>
> "Do I need OTel and Prometheus both?"
> "No — each is a separate Lifecycle implementation. Register only what you need. If you register neither, the Runtime still works without traces or metrics exported."
