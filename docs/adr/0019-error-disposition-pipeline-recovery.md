# ADR-0019: Error Disposition — In-Pipeline Recovery via RouteErrorHandler

**Date:** 2026-06-13
**Status:** Accepted
**Amends:** ADR-0012
**Amended by:** [ADR-0024](./0024-pipeline-outcome-replaces-camel-error-stopped.md)

> **Amended by [ADR-0024](./0024-pipeline-outcome-replaces-camel-error-stopped.md):** the route pipeline executor (`run_steps`) now returns `PipelineOutcome` (Completed | Stopped | Failed) instead of `Result<Exchange, CamelError>`. The in-pipeline disposition table below is unchanged; see ADR-0024 for the `PipelineOutcome` semantics and reply-channel adapter.
>
> **Phase 4 amendment (2026-06-22, ADR-0025):** `retry_step` retries the failed **compiled step** via `RetryableStep` (generalised from `&mut BoxProcessor`). `RetryOutcome::Stopped` exits before the disposition phase — Stop is successful control flow, not exhausted error handling. The `StepDisposition` model itself is unchanged.

## Decision

Error handling decisions are made INSIDE the pipeline step loop, not by an outer Tower middleware layer. A `RouteErrorHandler` trait is injected into `SequentialPipeline` / `TracedPipeline`, and after each step failure the handler decides the disposition:

- **`Propagate`** — return the error upstream; the route aborts (default).
- **`Handled`** — absorb the error, send to DLC, and return `Ok(exchange)` immediately; the route terminates normally.
- **`Continued`** — clear the error, send to DLC, and continue to the next step in the pipeline.

A new `RouteChannelService` wraps the pipeline with explicit Security and CircuitBreaker gates. Boundary errors (Security denials, CB rejections) flow through the same `RouteErrorHandler::handle_boundary` method, ensuring they reach the DLC without double-counting or provenance hacks.

The previous `ErrorHandlerLayer` / `ErrorHandlerService` Tower middleware is deprecated (since 0.16.0) and remains only as a backward-compatibility shell for routes that have no `errorHandler` configured.

## Context

The old architecture wrapped the entire `SequentialPipeline` with an `ErrorHandlerLayer` Tower middleware. When a step failed, the pipeline loop aborted immediately and returned `Err` to the outer layer. The handler could absorb the error (`handled: true`) and return `Ok(exchange)`, but it could not instruct the pipeline to continue to the next step — the loop had already exited. This made Camel's `continued=true` semantics impossible to implement.

Additionally, the old layer-based composition made CircuitBreaker and SecurityPolicy interact with error handling opaquely: errors flowed through Tower layers in stack order, and the handler could not distinguish whether an error originated from a pipeline step, a CB rejection, or a Security denial.

The pipeline needs to be "recovery-aware": after a step fails, the handler must be able to say "clear the error and continue to the next step" without re-entering the failed step.

## Comparison with Apache Camel 4.x

Apache Camel models error handling with an outer `ErrorHandler` wrapping the route processor. After a step throws, the handler applies `onException` policies: `handled=true` absorbs the error and breaks out of the original route (optionally routing to a sub-route); `continued=true` clears the error and resumes the original route. Redeliveries retry from the point of failure. When all retries are exhausted the exchange is sent to the Dead Letter Channel. These semantics are faithfully mirrored by `ExceptionDisposition` (`Propagate`/`Handled`/`Continued`), `retry_step`, and the DLC catch-all policy.

The architectural divergence is forced by Tower. Camel's JVM runtime has no readiness concept — every call is synchronous from the error handler's perspective. Tower splits `poll_ready` (readiness) from `call` (execution), and treats a readiness `Err` as a permanently broken service. This is incompatible with Camel's model where all errors are routable, retryable events. rust-camel resolves the mismatch by having `poll_ready` return `Ready(Ok(()))` unconditionally (multicast, error handler) and routing readiness errors through the same in-pipeline `RouteErrorHandler`. The consequence is that `continued=true` is implemented inside the pipeline loop rather than by an outer layer — a structural necessity, not a stylistic choice.

### Enumeration of processors bound by the `Ready(Ok(()))` readiness contract

Processors whose semantics are incompatible with Tower's "readiness `Err` =
permanently broken service" assumption MUST NOT propagate readiness errors from
`poll_ready`. They MUST return `Ready(Ok(()))` unconditionally, or preserve only
`Pending` backpressure while mapping readiness `Err` to `Ready(Ok(()))`, and move
per-endpoint or per-fragment readiness checks into `call()` where the route error
handler can apply retry, handled, continued, failover, or stop-on-exception
semantics.

| Processor | Reason | Status |
|---|---|---|
| `MulticastService` | Parallel/sequential fan-out must honour `stop_on_exception`; per-endpoint readiness belongs in `call()`. | migrated |
| `ErrorHandlerService` | Deprecated compatibility shell; retry/DLC handling happens in `call()`. | migrated |
| `AggregatorService` | Aggregation buckets and timeout work are call-time state; readiness has no external endpoint to validate. | migrated |
| `RecipientListService` | Recipients are dynamically resolved; readiness is per resolved recipient in `call()`. | migrated |
| `WireTapService` | Fire-and-forget tap failures MUST NOT block the main pipeline. | pending-fix |
| `LoadBalancerService` | Failover/selection strategies must skip broken endpoints in `call()`, not fail before selection runs. | pending-fix |
| `SplitterService` | Fragment sub-pipeline readiness is checked per fragment in `call()`; outer readiness MUST NOT bypass split error policy. | pending-fix |
| `StreamingSplitterService` | Streaming fragment readiness is checked per fragment in `call()`; outer readiness MUST NOT abort before stream policy runs. | pending-fix |

`SecurityPolicyService` is intentionally excluded. Route-level authorization is a
pre-pipeline boundary per [ADR-0010](./0010-security-policy-pre-pipeline-authorization.md):
authorization faults are system-boundary faults and MUST surface before normal
EIP processing runs.

| Concept | Apache Camel 4.x | rust-camel |
|---|---|---|
| Error handler placement | Outer layer wrapping route | In-pipeline `RouteErrorHandler` injection |
| `handled=true` | Break original route; optional sub-route | `Handled`: absorb → DLC → route terminates normally |
| `continued=true` | Resume original route after error | `Continued`: clear error → DLC → advance to next step |
| Redelivery | Retry from point of failure | `retry_step` retries the failed step |
| DLC default | `DeadLetterChannel` handler | Catch-all `Handled` policy when no `onException` |
| Readiness errors | N/A (no readiness concept) | Routed through `RouteErrorHandler` (not permanent) |
| CircuitBreaker + handled error | Separate EIP; open = `onCallNotPermitted` | `Handled` counts as CB success (handler absorbed it) |

## Considered Options

### Keep outer layer; add a "resume from step N" mechanism

Rejected. The pipeline would need to carry cursor state, and the outer layer would need to re-invoke the pipeline starting from step N+1. This splits the error-handling state machine across two components (the layer and the pipeline), making retry, DLC routing, and disposition logic hard to follow. It also re-enters the Tower `ready()` / `call()` protocol mid-pipeline, which is not designed for resumption.

### Move handler decision inside the pipeline loop (accepted)

The handler is injected as `Option<Arc<dyn RouteErrorHandler>>`. After each step, on `Err`, `run_steps` calls `match_policy` → `retry_step` → `handle_step`. The disposition returned by `handle_step` determines whether the loop continues (`Continued`), returns early (`Handled`), or propagates (`Propagate`). This keeps the entire error state machine in one place and makes the `Continued` variant trivial to implement — the loop simply clears the error and advances to the next step.

### Remove ErrorHandlerLayer entirely

Rejected. Routes with no `errorHandler` config still use the Tower layer path for backward compatibility. Removing it would be a breaking change for users who compose routes programmatically without configuring error handlers.

## Consequences

- `ExceptionDisposition` replaces `handled: bool` throughout `camel-api`. `ExceptionPolicy` carries a `disposition: ExceptionDisposition` field. The DSL gains a `continued: true` field on `onException`, mutually exclusive with `handled: true`.
- `RouteChannelService` (constructed only when an `errorHandler` is configured) chains Security → CB(`before_call`) → Pipeline(`run_steps`) → CB(`after_result`). Boundary errors from Security or CB gates go through `handle_boundary`, which routes them to the DLC.
- CircuitBreaker `after_result` receives the post-handler pipeline result. A `Handled` error counts as CB success — the handler absorbed it, so the CB does not trip. Users who want the CB to count absorbed errors must use `Propagate` disposition instead.
- When a DLC is configured with no explicit `onException` clauses, `resolve_error_handler` injects a catch-all `Handled` policy, preserving the old "DLC absorbs all errors" behaviour.
- `ErrorHandlerLayer` / `ErrorHandlerService` are deprecated. New routes should use the `RouteChannelService` path (automatic when `errorHandler` is configured). The old Tower layer path is used only when no `errorHandler` is present.
- `RouteChannelService` is `pub` but gated behind the `internal-adapters` feature flag; it is not part of the stable public API.
- `send_to_handler` always returns `Ok(exchange)` — the `Err` branches in `handle_step` / `handle_boundary` are dead code by construction (documented with comments).
