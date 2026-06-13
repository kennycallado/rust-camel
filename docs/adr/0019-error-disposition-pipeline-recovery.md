# ADR-0019: Error Disposition — In-Pipeline Recovery via RouteErrorHandler

**Date:** 2026-06-13
**Status:** Accepted
**Amends:** ADR-0012

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
