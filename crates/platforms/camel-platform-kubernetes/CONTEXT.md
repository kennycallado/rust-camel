## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(c) system-broken** (platform_service.rs L261): leader election loop terminated without cancellation — lifecycle anomaly. Keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via `error!` is the signal).

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.
