## Log-level policy

Per ADR-0012:

- **(c) system-broken** (component.rs L463): bridge restart max-attempts exhausted → "staying degraded". Component-layer lifecycle termination. Keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via error!).

- **(b′) outside-contract** (consumer.rs L316): normal-data ctx.send to pipeline failure. `runtime.metrics().increment_errors(route_id, "b-prime:jms:consumer-send")` BEFORE `error!`. Keeps `error!`. Source read confirmed no deliberate error-handoff — `build_exchange(&jms_msg, map_headers)` constructs Exchange from real JMS message, no `set_error()` or `bridge_error_handler` wrapping.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.
