# ADR-0024: PipelineOutcome Replaces `CamelError::Stopped` for Pipeline Control Flow

**Date:** 2026-06-21
**Status:** Accepted
**Amends:** ADR-0019

## Decision

Introduce `PipelineOutcome` as the return type of the **route pipeline executor** (`run_steps`), NOT as the `Response` type of every `Service<Exchange>`. The Tower data plane (`BoxProcessor = BoxCloneService<Exchange, Exchange, CamelError>`) is unchanged.

```rust
/// Result of executing a full route pipeline (multiple steps).
/// Produced by `run_steps`; consumed by the route controller and the
/// consumer reply-channel adapter. Individual processors keep returning
/// `Result<Exchange, CamelError>` — `PipelineOutcome` lives one layer up.
pub enum PipelineOutcome {
    /// Normal end of pipeline (all steps completed, or handler returned Handled).
    Completed(Exchange),
    /// `Step::Stop` was hit. Exchange is the response state (NOT discarded).
    /// Stop is successful control flow, not an error.
    Stopped(Exchange),
    /// Unhandled error escaped the pipeline (handler returned Propagate, or
    /// no handler was configured and a step errored).
    Failed(CamelError),
}
```

`CompiledStep` gains a `Stop` variant that `run_steps` recognises and converts into `PipelineOutcome::Stopped(ex)` WITHOUT invoking a Tower service. The existing `StopService` (`crates/camel-processor/src/stop.rs:27`) that returns `Box::pin(async { Err(CamelError::Stopped) })` is removed.

`CamelError::Stopped` is **removed entirely** — no `#[deprecated]`, no `#[allow(deprecated)]`, no legacy alias retention. Project policy (user directive 2026-06-20): "no deprecamos xq no tenemos usuarios". The README confirms pre-release status ("APIs will change").

## Context

The Stop EIP (`<stop/>`) terminates route processing without error semantics. In Apache Camel it is a successful break — the exchange is returned to the consumer, not discarded. rust-camel implemented it as `Err(CamelError::Stopped)`, which leaked control flow into the error type. This forced every reply finaliser to treat "Stop" as a special case: HTTP's consumer (`lib.rs:1185-1188`) returned a hardcoded 204 No Content with an empty body when it received `Err(CamelError::Stopped)`, discarding the Exchange state that the Stop step had preserved.

The same `CamelError::Stopped` variant was also misused by JMS and OpenSearch producers (`camel-jms/producer.rs:140`, `camel-opensearch/producer.rs:539`) to signal "shutting down, don't retry" during `poll_ready` — a completely different concept from the Stop EIP. This conflation of "consumer stopping" with "route stopped" meant any fix for the EIP had to first disentangle the misuse before the variant could be removed.

Phase 3 fixes this by:
1. Introducing `PipelineOutcome` at the correct layer (one above Tower).
2. Making `Stop` a route-internal compiled step (not a Tower service returning an error).
3. Replacing `CamelError::Stopped` in JMS/OpenSearch with `CamelError::ConsumerStopping`.
4. Removing `CamelError::Stopped` entirely (no deprecation, no alias).

## Comparison with Apache Camel 4.x

Apache Camel models Stop as a control-flow processor that returns `true` from `process(Exchange, AsyncCallback)`, signalling "stop processing" to the pipeline. The exchange is passed through normally — all modifications (body, headers, properties) are preserved. The consumer builds its response from the exchange state, exactly as it would for normal completion. There is never an error or empty body involved.

| Concept | Apache Camel 4.x | rust-camel (after ADR-0024) |
|---|---|---|
| Stop EIP semantics | Successful break, exchange preserved | `PipelineOutcome::Stopped(ex)` → `Ok(ex)` at Tower boundary |
| Consumer response on Stop | Built from exchange state | Built from exchange state (same as Completed) |
| Error handler interaction | Stop bypasses error handler | `CompiledStep::Stop` bypasses the handler loop |
| Control-flow encoding | In-pipeline return value | `PipelineOutcome` (one layer above Tower) |
| "shutting down" signal | `ShutdownStrategy` / lifecycle | `CamelError::ConsumerStopping` (separate variant) |

## Considered Options

### Keep `CamelError::Stopped` as an error variant

Rejected. The invariant "every `Err` from `Service<Exchange>` is a real failure" is fundamental to Tower's design. `Stop` is semantically successful control flow — it should never be an error. Keeping it as `CamelError::Stopped` forces every consumer that processes pipeline results to know about Stop, duplicate the response-build logic, and risk discarding Exchange state (the actual Bug B — HTTP consumer returning empty 204 on Stop).

### Use `Exchange.stopped: bool` flag

Rejected by e_gpt (spec §3.1). Leaks control state into the data object; every processor and consumer would need to check the flag. A boolean flag is invisible in the type system — no compiler guarantees that any consumer actually checks it.

### Keep `StopService` as a Tower service but change its error variant

Rejected. `StopService::call` currently returns `Box::pin(async { Err(CamelError::Stopped) })`. Even if the error variant changed, it would still be an `Err` at the Tower layer, violating "errors are failures". The correct fix is to remove `StopService` entirely and recognise Stop at the `run_steps` layer, before the Tower boundary.

## Consequences

### `run_steps` return type changes

**Breaking change** (acceptable pre-release per README "APIs will change"). The return type of `run_steps` changes from `Result<Exchange, CamelError>` to `PipelineOutcome`. Every call site within `camel-core` that matches on the return of `run_steps` is updated. `RouteChannelService::call` and `SequentialPipeline::call` / `TracedPipeline::call` gain a trivial translation shim:

```rust
fn outcome_to_reply(outcome: PipelineOutcome) -> Result<Exchange, ReplyError> {
    match outcome {
        PipelineOutcome::Completed(ex) | PipelineOutcome::Stopped(ex) => Ok(ex),
        PipelineOutcome::Failed(err) => Err(ReplyError::Failed(err)),
    }
}
```

> Note: `ReplyError` shown here is the consumer-side wrapper; in production the actual translation site (`SequentialPipeline::call` / `TracedPipeline::call`) returns `Result<Exchange, CamelError>` directly — `Failed(err)` maps to `Err(err)`. The `ReplyError` shape is the spec's illustrative name; the implemented adapter (see Task 2) is `PipelineOutcome::into_tower_result(self) -> Result<Exchange, CamelError>`.

This adapter lives at exactly one site per pipeline — the body of `SequentialPipeline::call` / `TracedPipeline::call` in `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (see ADR-0018 for route lifecycle context) — which is the only place `PipelineOutcome` crosses back into `Result<Exchange, CamelError>`.

### `RouteErrorHandler` trait shape is UNCHANGED

The handler (`match_policy`, `retry_step`, `handle_step`, `handle_boundary`) keeps its existing return types (`StepDisposition`). Translation from `StepDisposition` to `PipelineOutcome` happens inside `run_steps`, per the mapping table below. This locking of the trait shape was an explicit e_gpt blessing condition.

### `CompiledStep` type alias becomes an enum

Every arm in `route_compiler.rs` that constructs `CompiledStep` is migrated. The compiler gains a `CompiledStep::Stop` variant. `CompiledStep::Processor(BoxProcessor)` remains for all other steps. Every arm that previously pushed `StopService` as a `BoxProcessor` now pushes `CompiledStep::Stop`.

### `StopService` is removed

The file `crates/camel-processor/src/stop.rs` is deleted (or gutted if other concerns remain). No trace of the Tower service that returned `Err(CamelError::Stopped)` survives.

### `CamelError::Stopped` is removed (hard removal, no deprecation)

**Removal path (updated 2026-06-22 per e_gpt Option E):** `CamelError::Stopped`
is removed only after (a) JMS/OpenSearch `poll_ready` migrate to
`ConsumerStopping` (Tasks 17-19), AND (b) nested structural sub-pipelines
propagate Stop without going through Tower Response (deferred to future epic).
Task 22's variant removal is contingent on both conditions. `StopService`
removal (Task 7) is similarly deferred.

### Mapping table: error handler ↔ `PipelineOutcome`

The following table governs how `run_steps` translates handler outputs into `PipelineOutcome`:

| Inside the step loop | Handler returns | `run_steps` produces |
|---|---|---|
| Step completes normally | (not called) | continue to next step |
| Step errors, handler matches and absorbs | `StepDisposition::Handled(ex)` | `PipelineOutcome::Completed(ex)` |
| Step errors, handler clears and continues | `StepDisposition::Continued(ex)` | continue loop with `ex` |
| Step errors, handler propagates | `StepDisposition::Propagate(err)` | `PipelineOutcome::Failed(err)` |
| Step is `CompiledStep::Stop` | (handler bypassed) | `PipelineOutcome::Stopped(ex)` |
| Step is `CompiledStep::Stop` boundary error | `StepDisposition::Propagate(err)` (via `handle_boundary`) | `PipelineOutcome::Failed(err)` |

### Reply-channel adapter

Consumers that today expect `Result<Exchange, CamelError>` from the pipeline get the translation inside `SequentialPipeline::call` / `TracedPipeline::call`:

- `PipelineOutcome::Completed(ex) | PipelineOutcome::Stopped(ex)` → `Ok(ex)`.
- `PipelineOutcome::Failed(err)` → `Err(err)`.

`Stopped(ex)` and `Completed(ex)` are INDISTINGUISHABLE to the reply channel — both deliver the Exchange as a successful response. This is the core fix for Bug B: HTTP consumer's reply finaliser does not need to know whether the pipeline completed normally or stopped; it builds the response from `ex` identically in both cases.

### CircuitBreaker interaction

`RouteChannelService::after_result` (route_compiler.rs:393-396) sees `Ok(ex)` for both `Completed` and `Stopped` because the `PipelineOutcome` → `Result<Exchange, CamelError>` translation happens upstream. Therefore `after_result` counts Stop as success — no code change required in `RouteChannelService` itself. A regression test is mandatory (Task 14).

### UnitOfWork interaction

`ExchangeUoW<S>::call` (exchange_uow.rs:94-133) sees `Ok(ex)` for both `Completed` and `Stopped`, so `on_complete` fires for Stop. The `ex.has_error()` branch remains for explicit `set_error()` calls. A regression test is mandatory (Task 15).

### Sub-pipeline boundary (amendment 2026-06-22)

The original boundary rule above ("PipelineOutcome MUST NOT cross any
Service<Exchange>::Response") applies to **public** Service<Exchange> impls —
i.e., the top-level route pipeline and the consumer reply channel. **Nested
structural sub-pipelines** (Filter, Choice, Loop, Multicast, Split, doTry) are
themselves Service<Exchange> impls from their outer pipeline's perspective,
and currently communicate Stop to the outer pipeline via the legacy
`CamelError::Stopped` sentinel.

**Why this exception exists:** Apache Camel semantics require `.stop()` inside
any nested block to halt the entire route, not just the block. ADR-0024's
`PipelineOutcome::Stopped(ex)` cannot cross the sub-pipeline's
Service<Exchange>::Response boundary without breaking the boundary rule, so
an internal control sentinel is required until EIPs become outcome-aware.

**Migration contract (per e_gpt oracle Option E, 2026-06-22):**

- During Phase 3: nested Stop is mapped to `StopService` at sub-pipeline
  compilers (`step_compilers/{control_flow,routing,splitting}.rs`). The outer
  `run_steps` recognises `Err(CamelError::Stopped)` from a step and translates
  it to `PipelineOutcome::Stopped(original)` — bypassing the route error
  handler. The Exchange from before the step ran is preserved.
- Future epic (post-Phase-3): introduce outcome-aware internal sub-pipeline
  execution OR a dedicated internal control adapter that propagates
  `PipelineOutcome::Stopped` across sub-pipeline boundaries without going
  through Tower Response. Once that lands, `StopService` and
  `CamelError::Stopped` can be removed entirely (Tasks 7 and 22 deferred).

**Boundary rule clarification:** `PipelineOutcome` must not cross any **public**
Service<Exchange>::Response. The `CamelError::Stopped` sentinel is permitted
**only** as an internal control flow signal between a nested structural
sub-pipeline and its outer `run_steps`; it MUST NOT surface to consumer reply
finalisers (HTTP/Kafka/WS/gRPC), which continue to see `Ok(ex)`.

### HTTP consumer fix (Bug B)

The root cause of Bug B is that `StopService::call` returns `Err(CamelError::Stopped)` and discards the Exchange. The HTTP consumer's reply finaliser (`camel-http/src/lib.rs:1185-1188`) cannot read the Exchange state — there is no Exchange to read. After ADR-0024, `Stopped(ex)` arrives as `Ok(ex)` at the Tower boundary, so the HTTP consumer builds its response from the Exchange state using the same code path as `Completed(ex)`. The hardcoded 204 No Content special case is removed.

### ADR-0019 amendment

ADR-0019's in-pipeline disposition table is unchanged (the three `StepDisposition` variants and their meanings are untouched). However, the scope note in ADR-0019 is updated to reference ADR-0024 for `PipelineOutcome` semantics. Specifically: `ExceptionDisposition` governs decisions **inside** the step loop; `PipelineOutcome` governs the **result** of the loop. They are complementary, not overlapping.

## Audit (Task 16, 2026-06-22)

Searched `crates/components/` for `Err(CamelError::Stopped)` arms in consumer reply finalisers (the Bug B pattern).

**Components scanned:** `camel-kafka`, `camel-ws`, `camel-component-grpc`, `camel-cxf`, plus all other `crates/components/*`.

**Findings:** ZERO reply-finaliser special-cases found. The Bug B pattern was unique to `camel-http`.

**Remaining `CamelError::Stopped` references in `crates/components/` (all covered by other Phase 3 tasks):**

| File:line | Site | Phase 3 task |
|-----------|------|--------------|
| `crates/components/camel-component-wasm/src/serde_bridge.rs:216` | WASM error bridge mapping | Task 20 |
| `crates/components/camel-component-api/src/network_retry.rs:331` | doc comment | Task 23 |
| `crates/components/camel-jms/src/producer.rs:140` | JMS `poll_ready` misuse | Tasks 17-18 |
| `crates/components/camel-opensearch/src/producer/mod.rs:539` | OpenSearch `poll_ready` misuse | Task 19 |

No additional Bug B fixes needed beyond `camel-http` (landed in commit `4f81a6e0`).
