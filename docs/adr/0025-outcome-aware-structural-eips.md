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

