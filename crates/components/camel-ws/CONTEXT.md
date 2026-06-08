## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(g) outside-contract** (lib.rs L212, L234):
  - L212 = TLS bind failure in `spawn_server`. Calls `runtime.health().force_unhealthy_for_route(route_id, "g:ws:bind-tls", &e.to_string())` BEFORE the `error!`. The health pin is the operator signal; `error!` provides loud log visibility.
  - L234 = plain bind failure in `spawn_server`. Calls `runtime.health().force_unhealthy_for_route(route_id, "g:ws:bind-plain", &e.to_string())` BEFORE the `error!`. Same pattern.
  Both sites keep `error!` with `// log-policy: outside-contract`.

- **(e) outside-contract** (lib.rs L393): per-request policy evaluation error during WS upgrade in `dispatch_handler`. Calls `runtime.metrics().increment_errors(route_id, "e:ws:policy-eval")` BEFORE the `error!`. The metric is the operator signal; `error!` provides loud log visibility. Site keeps `error!` with `// log-policy: outside-contract`.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.
