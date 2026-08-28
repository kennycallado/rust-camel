## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(c) system-broken** (`platform_service.rs` L381-387): leader election loop terminated without cancellation. This is a lifecycle anomaly. The site keeps `error!` with `// log-policy: system-broken`. The `error!` event is the operator signal, so this site does not emit a metric.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.

## Self-fencing

The leadership loop self-fences on the renewal budget. The budget is
`renew_deadline` counted from the last successful renewal
(`leadership_fsm::remaining_budget`). Every reconcile attempt is bounded by
the remaining budget (`leadership_fsm::bound_attempt`); a hanging attempt
fails at the boundary. At budget exhaustion the holder steps down — clears
`is_leader`, emits `StoppedLeading` — without observing the Lease. Within
the budget, `Failed` and `Conflict` outcomes keep leadership and retry at a
jittered `retry_period`, with the sleep clamped to the remaining budget.
Config validation rejects `renew_deadline >= lease_duration`, so the holder
fences itself before the lease can legally expire for peers (modulo clock
skew on Lease timestamps). While leading, the stored epoch is clamped
monotonic (ADR-0035); an observed regression (deleted/recreated Lease) is
ignored and logged. The clamp bounds only the LOCAL pin: after a
fleet-wide term reset the local epoch stays pinned at the old maximum
until this pod next enters leadership fresh (`BecomeLeader` on
step-down-and-reacquire, or restart). GLOBAL fencing recovers only when
the server term exceeds the pre-reset maximum — until then a stale
pre-reset epoch can outrank the current leader's writes.

## Dependency boundary

`KubernetesLeadershipService::new` and `KubernetesReadinessGate::new` intentionally accept `kube::Client`. This platform-integration crate uses the native client as its public injection point. A breaking `kube` upgrade can therefore require a breaking release of this crate.

The implementation uses `kube` types in `platform_service.rs` and `readiness_gate.rs`. It maps Kubernetes failures to `PlatformError` or `CamelError` before they cross the platform contracts.

ADR-0020 makes a different choice for the beta `siumai` dependency. It confines that dependency because its API churn and provider-specific types would otherwise spread through a general component. A project-owned wrapper around `kube::Client` here would duplicate the platform client's API without creating a second implementation boundary.
