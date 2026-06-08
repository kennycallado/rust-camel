## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(b′) outside-contract** (consumer.rs L148, L157, L364, L378, L456, L466): channel-closed ctx.send failures in poll / commit paths. Each site calls `runtime.metrics().increment_errors(route_id, "b-prime:kafka:<site>")` BEFORE the `error!`. The metric is the operator signal; `error!` provides loud log visibility. Both stay.

- **(c) system-broken** (bundle.rs L30, consumer.rs L430):
  - bundle.rs L30 = startup-time bundle init failure in `register_all`. Lifecycle failure — no route_id, no runtime accessor. Annotation-only.
  - consumer.rs L430 = "exhausted reconnect attempts" → consumer `break` ends the route lifecycle. Annotation-only.
  Each site keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via error! is the signal).

- **(a) handler-owned** (producer.rs L263): producer `call()` returns Err → pipeline catches → route ErrorHandler owns ERROR. Downgraded to `warn!` with `// log-policy: handler-owned`. No metric call.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.
