## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(c) system-broken** (consumer.rs L200, L205, L393):
  - L200 = consumer task returned error during `stop()` shutdown lifecycle.
  - L205 = join error during `stop()` shutdown lifecycle.
  - L393 = retry-exhaustion: max transient-error attempts exceeded, consumer `return Err` ends route lifecycle.
  Each site keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via error! is the signal).

- **(b′) outside-contract** (consumer.rs L291, L377):
  - L291 = PubSub `ctx.send()` failure (channel closed). Calls `runtime.metrics().increment_errors(route_id, "b-prime:redis:pubsub-channel-closed")` BEFORE the `error!`.
  - L377 = BLPOP `ctx.send()` failure (channel closed). Calls `runtime.metrics().increment_errors(route_id, "b-prime:redis:blpop-channel-closed")` BEFORE the `error!`.
  The metric is the operator signal; `error!` provides loud log visibility. Both stay.

- **(e) outside-contract** (consumer.rs L416):
  - L416 = per-message non-transient Redis error. Calls `runtime.metrics().increment_errors(route_id, "e:redis:message-non-transient")` BEFORE the `error!`.
  `error!` at this site stays because the error is per-message and non-recoverable without user action.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.

## Crash health ownership

Per the project-wide contract (CONTEXT-MAP "Supervision / ConsumerRestart"):
the Redis consumer does NOT call `force_unhealthy_for_route` on task crash.
The Runtime pins health via `CrashNotification → RuntimeCommand::FailRoute →
commands.rs`.

The Redis producer holds a `runtime: Arc<dyn RuntimeObservability>` field
that is currently unused (producer errors are handler-owned per ADR-0012).
Retained for Phase A API consistency; the consumer's same-typed field is
metrics-only (used at L291/L377/L416 sites above).
