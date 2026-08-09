# Error handling

Errors in rust-camel split into two mechanisms. **Disposition** decides what
happens to an exchange after a catch matches it. The **route-level error
handler** is the safety net that retries failed steps and routes exhausted
exchanges to a dead-letter endpoint. The two compose. They do not overlap.
Block-level `do_try` adds a third, local scope for a small group of steps.

## Error model

Every failure in the data plane is a `CamelError`. The enum lives in
`crates/camel-api/src/error.rs`.

| Variant | Cause |
|---|---|
| `ProcessorError(String)` | A pipeline step failed. |
| `ProcessorErrorWithSource(String, ..)` | A step failed and the source error chain is kept. |
| `Io(String)` | An I/O operation failed. |
| `RouteError(String)` | A route lifecycle error. |
| `CircuitOpen(String)` | A circuit breaker rejected the exchange. |
| `HttpOperationFailed { .. }` | An HTTP call returned an error status. |
| `ValidationError(String)` | Exchange validation failed. |
| `Unauthorized(String)` | Authorization denied the exchange. |
| `ConfigValidation(ConfigValidationError)` | Route or context configuration is invalid. |
| `ConsumerStopping` | The consumer is shutting down. This is a control signal, not a processing failure. |

`CamelError` is `#[non_exhaustive]`. New variants can arrive in any release
without a breaking change. Match with a `_` arm in downstream code. See
`crates/camel-api/CONTEXT.md`.

`Stop` is not a `CamelError`. A `stop` step ends the route as successful
control flow. The pipeline reports it as `PipelineOutcome::Stopped`, never as
`Err` ([ADR-0024](../adr/0024-pipeline-outcome-replaces-camel-error-stopped.md)).

## How errors propagate

A pipeline runs its compiled steps in sequence. See
[routes and pipelines](routes-pipelines.md). Each step returns
`Result<Exchange, CamelError>` to the executor. On `Err`, the executor calls
the route error handler before the loop continues.

The handler returns a disposition. The executor translates that disposition
into a `PipelineOutcome`. The full outcome algebra, and the adapter that maps
it back to `Result` at the Tower boundary, live on the
[routes and pipelines](routes-pipelines.md) page. This page covers only the
error path.

## Error disposition

Disposition is the per-catch decision. After a catch clause matches an error,
the handler picks one of three dispositions. The `ExceptionDisposition` enum
in `crates/camel-api/src/error_handler.rs` defines them.

| Disposition | Effect on the exchange | Effect on the pipeline |
|---|---|---|
| `Propagate` | Keep the error. | Abort. The outcome is `Failed`. |
| `Handled` | Clear the error. | Stop. The outcome is `Completed`. |
| `Continued` | Clear the error. | Advance to the next step. |

Disposition runs inside the pipeline loop. It is not the retry policy and it
is not the dead-letter channel. Those belong to the route-level error handler,
which runs first. When the handler exhausts its retries and (optionally) sends
the exchange to the dead-letter channel, it then applies the disposition. See
[ADR-0019](../adr/0019-error-disposition-pipeline-recovery.md).

The default disposition for a route-level `onException` clause is `Propagate`.
The default for a `do_try` catch clause is `Handled`.

## Route-level error handler

The route-level `error_handler` wraps every step in a route. It is the safety
net. It does three jobs, in this order:

1. **Retry** the failed step with a redelivery policy.
2. **Route** the exchange to a dead-letter endpoint or a custom `handled_by`
   endpoint.
3. **Apply** the disposition from the matched `onException` clause.

Configure it with `ErrorHandlerConfig`. A global handler on `CamelContext`
covers routes that have no per-route handler.

### Dead Letter Channel

```rust
{{#include ../../../examples/error-handling/src/main.rs:basic-dlc}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: "basic-dlc"
    from: "timer:route1?period=2000&repeatCount=1"
    error_handler:
      dead_letter_channel: "log:route1-dlc?showHeaders=true&showBody=true&showCorrelationId=true"
    steps:
      - set_header:
          key: "example"
          value: "basic-dlc"
      # The always_fail Rust closure maps to a registered bean in YAML.
      - bean:
          name: "always-fail"
          method: "process"
```

</details>

When a step fails and retries are absent or exhausted, the handler sends the
exchange to the DLC endpoint. The exchange keeps its error state and its
original message. The DLC is a fallback sink. It does not by itself stop the
route. The disposition still decides whether the pipeline aborts, stops, or
advances.

### Retry with backoff

```rust
{{#include ../../../examples/error-handling/src/main.rs:retry-backoff}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: "retry-backoff"
    from: "timer:route2?period=2000&repeatCount=1"
    error_handler:
      dead_letter_channel: "log:route2-dlc?showHeaders=true&showBody=true&showCorrelationId=true"
      on_exceptions:
        - retry:
            max_attempts: 3
            initial_delay_ms: 50
            multiplier: 2.0
            max_delay_ms: 1000
    steps:
      - set_header:
          key: "example"
          value: "retry-backoff"
      # The fail_n_times Rust closure maps to a registered bean in YAML.
      - bean:
          name: "fail-twice-then-succeed"
          method: "process"
```

</details>

The `RedeliveryPolicy` drives the retry loop:

- `initial_delay` sets the wait before the first retry.
- `multiplier` scales the wait after each attempt.
- `max_delay` caps the wait.
- `jitter_factor` randomizes the wait to avoid thundering-herd retries.

Defaults are a 100 ms initial delay, a 2x multiplier, a 10 s cap, and no
jitter. See `crates/camel-api/src/error_handler.rs`. A retry that recovers the
exchange clears the error and the pipeline advances. A retry that exhausts
falls through to the DLC and then to the disposition.

### OnException clauses

`onException` matches errors by variant or predicate. The first matching
clause wins. Each clause can carry its own retry, its own `handled_by`
endpoint, and its own disposition. Put broad clauses before specific clauses.
A broad clause first shadows the rest.

### Global error handler

Set a default handler on `CamelContext`. Routes without a per-route handler
use it.

```rust
ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel(
    "log:global-dlc",
))
.await;
```

This is Rust API only. YAML routes compile to the same RouteDefinition but cannot express registration logic. A global handler has no YAML field. Set it on `CamelContext` in Rust.

### Continued disposition in practice

The `continued` example shows the two mechanisms composing. The `onException`
clause sets the disposition. The error handler clears the error and the
pipeline advances to the next step. The DLC still receives the exchange for
auditing.

```rust
{{#include ../../../examples/error-handling/src/main.rs:continued-disposition}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: "continued-disposition"
    from: "timer:route10?period=2000&repeatCount=1"
    error_handler:
      dead_letter_channel: "log:route10-dlc?showHeaders=true&showBody=true&showCorrelationId=true"
      on_exceptions:
        - kind: "ProcessorError"
          continued: true
          retry:
            max_attempts: 1
    steps:
      - set_header:
          key: "example"
          value: "continued-disposition"
      # The always_fail Rust closure maps to a registered bean in YAML.
      - bean:
          name: "always-fail"
          method: "process"
      - to: "log:route10-continued?showHeaders=true&showBody=true&showCorrelationId=true"
```

</details>

Use `Continued` when a step is non-critical and the route should keep moving
after it fails.

## doTry blocks

`do_try` is a local error-handling scope. It wraps a group of steps in a try
block with catch clauses and an optional finally clause. A handled catch does
not trigger the route-level error handler. The block stays a local island.
Unhandled errors bubble up to the route.

```rust
{{#include ../../../examples/do-try/src/main.rs:do-try-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: "catch-by-variant"
    from: "direct:catch-by-variant"
    steps:
      - do_try:
          steps:
            # The always_fail Rust closure maps to a registered bean in YAML.
            - bean:
                name: "always-fail"
                method: "process"
          catch:
            - exception:
                - "ProcessorError"
              disposition: "handled"
              steps:
                - bean:
                    name: "log-marker"
                    method: "process"
```

</details>

See the [Do Try pattern page](../eip/do-try.md) for catch-by-variant,
catch-by-predicate, and finally clauses. The processor contract is in
`crates/camel-processor/CONTEXT.md`.

## doTry vs route-level error_handler

| Situation | Use |
|---|---|
| One step may fail and you want to repair it locally. | `do_try` with a catch clause. |
| All steps in the route share one error policy. | Route-level `error_handler`. |
| You need cleanup that runs on success and failure. | `do_try` with a `finally` clause. |
| You want to retry a failed step. | Route-level `error_handler` with `retry`. |
| You want to advance after a non-critical step fails. | Route-level `error_handler` with `Continued`. |

## Step lifecycle and drain

Stateful steps (aggregators, idempotent repositories, resequencers) own
background work that outlives a single `process()` call. They implement the
`StepLifecycle` trait.

When a route stops, the runtime drains stateful steps in this order:

1. Cancel consumer intake.
2. Force-complete aggregator buckets.
3. Cancel the pipeline token.
4. Join the consumer and pipeline tasks.
5. Call `shutdown(RouteStop)` on each stateful step.
6. Reset the cancellation tokens.

Shutdown errors are best-effort. A failing step does not block the rest from
draining. See [ADR-0022](../adr/0022-steplifecycle-trait-and-drain.md).

## Consumer failure supervision

A consumer that cannot continue returns an error from its task. The
RuntimeBus records the route as failed. An optional supervision policy then
restarts the whole route with backoff. Consumers retry transient external
failures inside their normal receive loop. They do not restart their own task
after a task-level failure. That decision belongs to the route control plane.
See [ADR-0007](../adr/0007-route-supervised-consumer-failure.md).

The `SupervisingRouteController` watches crashed routes and restarts them.
Configure it with `SupervisionConfig`:

```rust
let ctx = CamelContext::builder()
    .supervision(SupervisionConfig {
        max_attempts: None,
        initial_delay: Duration::from_millis(500),
        backoff_multiplier: 2.0,
        max_delay: Duration::from_secs(4),
    })
    .build()
    .await?;
```

This is Rust API only. YAML routes compile to the same RouteDefinition but cannot express registration logic. Supervision is a `CamelContext` concern, not a route field.

See `examples/auto-restart/` for a complete example.

## Examples

- `examples/error-handling/` covers the dead-letter channel, retry,
  `onException`, the continued disposition, the global handler, and the
  shorthand builder API.
- `examples/do-try/` covers catch by variant, catch by predicate, and finally
  cleanup.
- `examples/auto-restart/` covers the `SupervisingRouteController` with
  exponential backoff.
