# ADR-0039: Configurable Loop Iteration Cap

**Date:** 2026-07-11
**Status:** Accepted (implemented in `5f6d6d10`)
**Cross-refs:** ADR-0033 (Security defaults), ADR-0038 (Configurable DoS caps), ADR-0032 (Exchange-data trust boundary)

## Context

`MAX_LOOP_ITERATIONS = 10_000` was a hardcoded const bounding both Count-mode loop iterations and While-mode loop safety guards. It bounds a resource decision driven by exchange data (operator-supplied loop count / while predicate), qualifying it for a per-item escape hatch under ADR-0033's classification rule (established in ADR-0038).

## Decision

Add `max_iterations: Option<usize>` to the loop DSL surface (`LoopFullConfig`). Thread it through `LoopStepDef` → `BuilderStep::DeclarativeLoop` → `LoopConfig` → `LoopService`/`LoopSegment`. When absent, default to `MAX_LOOP_ITERATIONS` (10,000).

No upper ceiling — the operator makes an explicit per-Step resource decision. `max_iterations: 0` is rejected at compile time.

## ADR-0033 Compliance

Per-item escape hatch: satisfied. Each loop step can independently set `max_iterations`. No global disable switch.

## Out of Scope

- `DEFAULT_MATERIALIZE_LIMIT` callers (tracked as bd rc-b9q8).
- Per-Route (not per-Step) max_iterations setting.
