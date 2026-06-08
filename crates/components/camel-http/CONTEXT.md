## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(e) outside-contract** (lib.rs L748, L774):
  - L748 = accept-loop error in `run_axum_server`. Calls `runtime.metrics().increment_errors(route_id, "e:http:accept")` BEFORE the `error!`. The metric is the operator signal; `error!` provides loud log visibility.
  - L774 = server task exited unexpectedly in `monitor_axum_task`. Calls `runtime.metrics().increment_errors(route_id, "e:http:server-task-exited")` BEFORE the `error!`. Same pattern.
  Both sites keep `error!` with `// log-policy: outside-contract`.

- **(c) system-broken** (lib.rs L1108): `Body::Stream` already consumed before HTTP reply — programming-contract violation in `dispatch_handler`. Keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via error! is the signal).

- **(a) handler-owned** (lib.rs L1210): pipeline error processing HTTP request → 500 response in `dispatch_handler`. Route ErrorHandler owns the ERROR. Downgraded to `warn!` with `// log-policy: handler-owned`. No metric call.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.
