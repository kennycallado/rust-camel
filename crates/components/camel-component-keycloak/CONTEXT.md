## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(b′) outside-contract** (keycloak_consumer.rs L178): channel-closed `context.send()` failure in `process_event_batch`. Calls `runtime.metrics().increment_errors(context.route_id(), "b-prime:keycloak:response-body")` BEFORE the `error!`. The metric is the operator signal; `error!` provides loud log visibility. Both stay. Runtime passed as parameter from `start()`.

- **(e) outside-contract** (keycloak_consumer.rs L260): transient auth-material acquisition failure during retry loop in `start()`. Calls `self.runtime.metrics().increment_errors(context.route_id(), "e:keycloak:auth-material")` BEFORE the `error!`. The metric is the operator signal; `error!` provides loud log visibility. Both stay.

- **(c) system-broken** (keycloak_consumer.rs L326): max-consecutive-auth-errors exhausted → consumer lifecycle termination. Keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via `error!` is the signal).

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.
