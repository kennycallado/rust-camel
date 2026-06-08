## Log-level policy

Per ADR-0012:

- **(b′) outside-contract** (consumer.rs L~272): response-marshalling failure after handler returns (post-handler side-effect). `runtime.metrics().increment_errors(route_id, "b-prime:cxf:response-marshalling")` BEFORE `error!`. Keeps `error!`.

- **(a) handler-owned** (consumer.rs L~285): route handler invocation returned Err to CXF consumer. Downgraded to `warn!`. No metric — route ErrorHandler owns ERROR.

- **(c) system-broken** (pool.rs L~441): pool restart max-attempts exhausted → "staying degraded" (lifecycle termination). Keeps `error!` with `system-broken` annotation. No metric — operator action required (fix CXF service config + restart).
