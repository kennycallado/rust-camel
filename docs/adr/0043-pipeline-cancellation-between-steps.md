# ADR-0043: Pipeline cancellation between steps

## Status
Accepted

## Context
`run_steps` is a linear `async fn` with no cooperative cancellation. When a route
stops/suspends or the context shuts down, in-flight Exchanges stuck in a slow step
`.await` have no observable cancel path. The `pipeline_cancel_token` exists in
`ManagedRoute` and is checked at idle in the pipeline task's `select!` loop, but
once an Exchange enters `pipeline.call(exchange).await`, cancellation cannot
interrupt it until the next idle cycle.

## Decision
Use a `tokio::task_local!` to propagate a per-start `CancellationToken` from the
pipeline task into `run_steps`. The pipeline task already creates a fresh child
token on each `start_route` call (`route_controller_trait.rs:118`). Before calling
`pipeline.call(exchange)`, the task scopes the token via `CANCEL_TOKEN.scope(...)`.
`run_steps` checks `CANCEL_TOKEN` at the top of each loop iteration — BETWEEN steps.

> **Why task-local, not compiled-in struct field (expert ruling):** A token
> compiled into the pipeline at registration is a child of
> `ManagedRoute.pipeline_cancel_token`. On stop, the parent is cancelled (killing
> the child), then `stop_route_internal` replaces the parent with a new token. On
> restart, the compiled pipeline still has the OLD cancelled child → every exchange
> fails immediately. A task-local is set per-start from the fresh child token,
> avoiding the lifecycle bug entirely.

### Cancellation outcome: `Failed(ConsumerStopping)` — justified

We return `PipelineOutcome::Failed(CamelError::ConsumerStopping)` rather than
`PipelineOutcome::Stopped(ex)` for two reasons:

1. **UoW hook semantics:** `Stopped` is a *successful* termination — UoW
   completion hooks fire, and the exchange is delivered to the reply channel
   as `Ok(ex)`. A cancelled exchange is NOT successfully processed — its data
   may be incomplete. `Failed(ConsumerStopping)` routes through UoW failure
   hooks, giving the operator visibility.

2. **Error handler behavior:** `Failed(ConsumerStopping)` reaches the error
   handler's `match_policy`. **Note:** the default `RouteErrorHandler` has NO
   special handling for `ConsumerStopping` — a handler with a catch-all policy
   MAY retry it. This is an accepted trade-off: the cancellation check happens
   between steps, so the exchange has already completed at least one step. A
   retry would re-invoke from the top of `run_steps`, hitting the cancel check
   immediately and returning `Failed(ConsumerStopping)` again — so retries are
   effectively self-limiting. Custom handlers that want to skip retry on
   `ConsumerStopping` can match the variant in their `match_policy`.

### Token lifecycle (per-start, not per-compile)
- `ManagedRoute.pipeline_cancel_token` — parent (created at registration).
- On `start_route`: `pipeline_cancel = managed.pipeline_cancel_token.child_token()`
  (route_controller_trait.rs:118). This child is FRESH on each start.
- Pipeline task wraps `pipeline.call(exchange)` in `CANCEL_TOKEN.scope(cancel.clone(), ...)`.
- `run_steps` reads `CANCEL_TOKEN` and checks `is_cancelled()` between steps.
- On `stop_route`: `managed.pipeline_cancel_token.cancel()` — propagates to the
  child in the pipeline task (which exits its select! loop). The pipeline struct
  itself is untouched — no lifecycle bug.

### Checkpoint granularity
- Check at the top of the for-loop, before destructuring each `CompiledStep`.
- If the task-local is not set (tests calling `run_steps` directly), skip the check.
- The in-flight step's `.await` completes naturally; the NEXT iteration exits.

## Consequences
- In-flight Exchanges exit cleanly on route stop (one more step boundary at most).
- UoW failure hooks see `Failed(ConsumerStopping)` — distinguishable from real errors.
- No lifecycle bug: token is per-start via task-local, not compiled-in.
- Restart regression test required (stop → start → exchanges process normally).
