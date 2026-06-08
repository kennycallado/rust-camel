# Log-level policy

Per ADR-0012, this component's `error!` / `warn!` sites are categorized as:

## Producer

- **(a) handler-owned** (producer.rs L228): WASM transform execution inside the pipeline (`warn!`). Route
  ErrorHandler owns the ERROR. Downgraded to `warn!` with
  `// log-policy: handler-owned`. No metric call.
