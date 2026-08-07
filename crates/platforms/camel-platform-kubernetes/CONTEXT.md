## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(c) system-broken** (`platform_service.rs` L301-302): leader election loop terminated without cancellation. This is a lifecycle anomaly. The site keeps `error!` with `// log-policy: system-broken`. The `error!` event is the operator signal, so this site does not emit a metric.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.

## Dependency boundary

`KubernetesLeadershipService::new` and `KubernetesReadinessGate::new` intentionally accept `kube::Client`. This platform-integration crate uses the native client as its public injection point. A breaking `kube` upgrade can therefore require a breaking release of this crate.

The implementation uses `kube` types in `platform_service.rs` and `readiness_gate.rs`. It maps Kubernetes failures to `PlatformError` or `CamelError` before they cross the platform contracts.

ADR-0020 makes a different choice for the beta `siumai` dependency. It confines that dependency because its API churn and provider-specific types would otherwise spread through a general component. A project-owned wrapper around `kube::Client` here would duplicate the platform client's API without creating a second implementation boundary.
