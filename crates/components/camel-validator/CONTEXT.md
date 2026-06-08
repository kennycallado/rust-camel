# Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

## Bridge reconnect (xsd_bridge)

- **(e) outside-contract** (xsd_bridge.rs L385): re-seed schema failed after bridge reconnect
  (transient recovery, NOT validation failure). Calls
  `metrics().increment_errors(route_id, "e:validator:reconnect-reseed")` BEFORE the `error!`.
  The metric is the operator signal; `error!` provides loud log visibility. Both stay.
