# Log-level policy

Per ADR-0012, this component's `error!` / `warn!` sites are categorized as:

## Client (bridge reconnect)

- **(e) outside-contract** (client.rs L331): failed to re-seed stylesheets after bridge reconnect
  (transient recovery, NOT handler invocation). Calls
  `metrics().increment_errors(route_id, "e:xslt:reconnect-reseed")` BEFORE the `error!`.
  The metric is the operator signal; `error!` provides loud log visibility. Both stay.

## Producer (transform pipeline)

- **(a) handler-owned** (producer.rs L154): stylesheet compilation failed inside the pipeline (`warn!`).
  Route ErrorHandler owns the ERROR. Downgraded to `warn!` with
  `// log-policy: handler-owned`. No metric call.

- **(a) handler-owned** (producer.rs L179): XSLT transform failed inside the pipeline (`warn!`).
  Route ErrorHandler owns the ERROR. Downgraded to `warn!` with
  `// log-policy: handler-owned`. No metric call.
