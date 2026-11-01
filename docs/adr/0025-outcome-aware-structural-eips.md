# ADR-0025: Outcome-aware Structural EIPs

**Date:** 2026-06-22
**Status:** Accepted (Phase 4)
**Amends:** ADR-0024
**Bd issue:** rc-5uv

## Decision

Structural EIPs (Filter / Choice / Loop / Throttle / Split / StreamingSplit / Multicast / LoadBalance / doTry) implement the `OutcomePipeline` trait (internal, one layer above Tower) and propagate `PipelineOutcome::Stopped(ex)` directly via the new `CompiledStep::Segment` variant. Sub-pipelines NO longer cross Tower `Service<Exchange>` boundary.

This fixes the Option E interim bug where `eip_outcome_to_result` dropped `Stopped(_ex)` Exchange state at the sub-pipeline boundary, losing mutations made inside the nested block before Stop.

## Context

Phase 3 (ADR-0024, commit `397a6cdc`) introduced `PipelineOutcome` for top-level route Stop propagation (Bug B fix). Nested structural EIPs were left on an interim mechanism (Option E) using `CamelError::Stopped` as an internal sentinel. Oracle audit (ses_1102e7531ffekfddOnVOt5xB14, 2026-06-22) found this drops Exchange state at sub-pipeline boundaries — a silent semantic bug.

## Comparison with alternatives

| Option | Description | Ruling |
|---|---|---|
| A | Trait `OutcomePipeline` only | Lacks ownership/cloning home |
| B | Tower Service w/ PipelineOutcome response | poll_ready boilerplate unjustified |
| C | `Box<dyn Fn>` pointer | Loses trait extensibility for stateful impls |
| **D** | **Trait + wrapper struct** | **CHOSEN** — cleanest clippy, extensibility hooks |

Oracle ruled: D. Trait is right primitive; wrapper struct is right ownership/API boundary. `CompiledStep::Segment(OutcomeSegment)` keeps CompiledStep clean, centralises cloning/tracing/metrics later, avoids leaking huge closure types or fake Tower errors.

## Consequences

Public contract `BoxProcessor = Service<Exchange, Response=Exchange, Error=CamelError>` is UNCHANGED. `PipelineOutcome` remains internal / public-adapter-adjacent; does NOT become normal user-facing processor response.

### Eight canonical invariants

1. `Continued` after Segment failure continues **after the outer Segment step**, NOT inside the child pipeline.
2. Retry retries the **whole structural EIP** from pre-step Exchange, NOT the failed child step.
3. If retry attempt returns `Stopped(ex)`, use the **retry-attempt stopped Exchange**, NOT the original.
4. `doTry` remains a **local error-handler island**; the outer handler sees only unhandled `Failed`.
5. Redelivery headers apply to Segment retries **same as Process retries**.
6. `PipelineOutcome` **never** becomes public `Service<Exchange>::Response`.
7. Parallel Split/Multicast must **not aggregate** completed sibling output after any branch returns Stop.
8. Tracing/metrics must **classify Stop as successful control flow**, NOT error.

## Glossary

**Stop EIP** ≠ **Route lifecycle Stop**. The former is data-plane control flow (Phase 4 subject). The latter is route lifecycle state (ADR-0004/0007/0018 subject).

## Migration

Per oracle M3 strategy: parallel construction, per-EIP switch. Build infrastructure beside current path; migrate one EIP at a time; delete `StopService`/`CamelError::Stopped` last. See `docs/superpowers/plans/2026-06-22-rc-5uv-phase-4-outcome-aware-eip.md` for task sequence.


## Status update (2026-10-27)

Nine structural EIPs originally carried dual implementations (a Tower `*Service` plus an `OutcomeSegment`). rc-ut13q (commit `7b1467aa3`, "drop the dual structural Tower services") deleted seven — Filter, Choice, Loop, Throttle, Splitter, Multicast, and LoadBalancer — because the compiler only ever drove the segments. Mission 420 (commit `6e48e00eb`) deleted `DoTryService`, which only the Rust builder `.do_try()` path used; the builder now compiles to `DoTrySegment`. `StreamingSplitterService` was deleted by rc-ltbwi (mission 422) once the builder gained `BuilderStep::StreamSplit`. The dual shape is not supported API.

## Status update (2026-10-29)

Invariant #4 makes doTry a local error-handler island. It does not decide the finally block's fate. The pre-fix `DoTrySegment` read it as license to skip `doFinally` when a catch body failed. That reading was wrong.

The fix restores Java try/catch/finally parity (JLS §14.20.2). The finally body runs exactly once even when the catch body fails. The catch error stays the surfaced failure. A throwing finally body restores the previous catch error under the existing `fail_after_finally` parity rule. Stop in finally still stops the outer route.

The island property is unchanged. The catch chain is not re-entered. The outer handler sees only the unhandled failure.

Traceability: corrected under bd rc-h3i8d. The declarative runtime is `DoTrySegment` since the `DoTryService` deletion (see the 2026-10-27 update).

## Amendment: exception property retention (2026-11-01)

`Exchange::clear_error()` removes `CamelExceptionMessage`, `CamelExceptionKind`, and `CamelExceptionCaught` together with the error. Any path that clears a disposition through `clear_error()` therefore drops those properties: `Handled` everywhere, and `Continued` at step level. At a route boundary `Continued` maps to `Propagate` and keeps the error. This intentionally diverges from Java Camel, where `Exchange.EXCEPTION_CAUGHT` survives a handled catch. The divergence is recorded here under bd rc-zaqjd; it is not an implementation accident.

The supported workaround is to read or copy the exception inside the catch body, for example into a user property or header. Steps after a `Handled` or `Continued` exit SHALL NOT rely on `CamelExceptionCaught`.

The two rejected options:

- Rejected — restore in `DoTrySegment`: splits doTry from the route-level handler.
- Rejected — restore in `clear_error()`: leaves stale exception state across redelivery and exchange reuse.

## Amendment: CamelExceptionHandled canonical rule (2026-11-01)

The `CamelExceptionHandled` exchange property has one canonical rule, and it applies to both doTry and the route-level handler. bd rc-ho9j1 owns the decision. The existing do-try spec scenarios cover the `Handled` and `Continued` cases; rc-ud3rl defers the additional scenarios.

1. Control flow keys off `PipelineOutcome` only.
2. `CamelExceptionHandled` is Camel-compatibility only; no core logic reads it.
3. Only a `Handled` arm can set it, and only where that arm calls `Exchange::handle_error()`: the doTry `Handled` arm and the route-level `Handled` paths that execute `on_exception` steps (step, boundary, and retry-service variants). A `Handled` arm that dispatches to the dead-letter channel clears with `clear_error()` and does not set it. `Continued` never sets the marker. Neither does the dead-letter-channel dispatch path.
4. `Handled` stops the block while `Continued` advances past it.
5. `DoTrySegment` removes a stale marker on entry, so an absent marker, a marker removed on entry, and a marker retained from a nested `Handled` block are three distinct states; only the third is observable after the block.
6. `clear_error()` does not touch the marker, so an outer `Continued` does not clear a marker that a nested `Handled` block set.
