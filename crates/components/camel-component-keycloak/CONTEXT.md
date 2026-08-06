## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **outside-contract** (`fn process_event_batch`, channel-closed arm): a
  channel-closed `context.send()` failure. Calls
  `runtime.metrics().increment_errors(context.route_id(), "b-prime:keycloak:response-body")`
  before the `error!`. The metric is the operator signal. The `error!` provides
  loud log visibility. Both stay. The caller passes the Runtime from `start()`.

- **outside-contract** (`Consumer::start`, auth-material arm): a transient
  auth-material acquisition failure during the retry loop. Calls
  `self.runtime.metrics().increment_errors(context.route_id(), "e:keycloak:auth-material")`
  before the `error!`. The metric is the operator signal. The `error!` provides
  loud log visibility. Both stay.

- **system-broken** (`Consumer::start`, max-auth-errors arm): exhausted
  consecutive authentication retries terminate the Consumer lifecycle. Keeps
  `error!` with `// log-policy: system-broken`. No metric call exists because
  the `error!` is the operator signal.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.
