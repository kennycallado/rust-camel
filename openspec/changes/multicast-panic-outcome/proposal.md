# Proposal: multicast-panic-outcome

## Why

In `crates/camel-processor/src/multicast_segment.rs`, `parallel_multicast`
drains its `JoinSet` with `if let Ok((idx, Some(o))) = res`. Every
`JoinError` is silently dropped. A branch task that panics therefore:

1. Contributes NO outcome (not even `Failed`), yet is counted in
   `branch_count`, so the partial-success warn under-reports
   `failed_branches`.
2. Can launder a zero-success run to `Completed(aggregator([]))` when all
   branches panic and no `last_error` was set. This violates the
   outcome-based composition invariant (ADR-0058): a panic is zero-success
   attempted work and MUST NOT surface as success.
3. Escapes the per-branch timeout arm's error mapping (a panic inside the
   timeout window aborts the task into a dropped `JoinError` instead of
   becoming a `Failed` outcome).

`recipient_list` already handles this exact case (`Err(join_err) if
join_err.is_panic()` maps to a representative `ProcessorError`, with an
ADR-0058 comment and test coverage). Multicast, the parallel sibling,
diverges. This is a
component-contract inconsistency between two Segments governed by the same
spec (bd rc-f88o, discovered during rc-b41j holistic review).

## What Changes

- `crates/camel-processor/src/multicast_segment.rs`: wrap each branch's
  outcome future with `catch_unwind` inside the spawned task, before the
  timeout arm, mapping a panic to
  `Failed(ProcessorError("multicast branch {idx} panicked"))`. The
  synthetic `Failed` preserves the branch index and flows through every
  existing downstream path (Stop-winner scan, `failed_branches` accounting,
  `last_error` highest-index rule, zero-success guard) with zero changes
  to that logic. Cancellation `JoinError` values remain excluded from
  failure accounting. The current Stop path does not abort in-flight tasks.
- `openspec/specs/segment-outcome-composition/spec.md`: MODIFIED
  requirement "multicast zero-success operational failure returns Failed"
  gains normative panic-classification clauses and scenarios.
- `crates/camel-processor/src/multicast_segment.rs` test module: six tests
  prove the bug and the fix. They cover the delta spec scenarios and the
  panic-payload regression in the design. The test module was extracted
  during implementation to
  `crates/camel-processor/src/multicast_segment_tests.rs` via
  `#[cfg(test)] #[path]` (crate precedent).
- `crates/camel-processor/CONTEXT.md`: the `MulticastSegment` row of the
  Structural EIP Segments table records the panic-to-`Failed` mapping
  clause.

## Acceptance Criteria

- All-panicked parallel multicast reports `Failed` (representative
  `ProcessorError`), never `Completed(_)`.
- A panicked branch counts toward `failed_branches`; mixed
  panic+failed runs report the correct count.
- `Stopped` still wins over panicked branches (Stop-winner scan intact).
- A panic inside the timeout window maps to `Failed`, not a dropped
  `JoinError`.
- A panic payload whose `Drop` implementation panics cannot escape the
  mapping as a second task panic.
- Sequential-mode and non-panicking parallel behavior unchanged.

## Risk Budget

Low-medium. The change is additive to the `results` vector; first_failed /
last_error / accounting / aggregation logic is untouched. New observable:
`last_error` may now surface a representative panic error where none
surfaced before. The spec delta covers this behavior. The implementation
must forget the caught panic payload after it constructs the representative
error. This prevents a payload with a panicking `Drop` implementation from
escaping as a second panic. It creates a bounded leak on an already-panicked
branch. Blast surface is confined to `camel-processor` (Runtime boundary).
There is no DSL, component, or CLI impact.

Affected crates: `camel-processor`. bd: rc-f88o.

### Self-grill record

**Questions generated:**
1. [glossary] Does the proposal use the canonical names for the pipeline
   abstraction and its outcomes?
2. [sharpen] Which behavior is normative, and which diagnostic is only an
   implementation detail?
3. [scenario] Which panic and control-flow combinations can bypass or override
   the new representative failure?
4. [cross-ref] Does the current drain and accounting code produce each claimed
   pre-fix defect, and does the recipient-list precedent support this mapping?

**Answers (with citations):**
1. [glossary] Yes. `OutcomePipeline`, `PipelineOutcome`, `Failed`, and `Stopped`
   match the runtime vocabulary (`CONTEXT-MAP.md:139-147`).
2. [sharpen] Panic classification and zero-success behavior are normative. The
   warn-level discarded-failure diagnostic is best-effort observability
   (`specs/segment-outcome-composition/spec.md:14-26,35-51`).
3. [scenario] The plan covers partial success, all-panic, mixed failure and
   panic, `Stopped` precedence, timeout-window panic, and a panic payload with
   a panicking `Drop` (`design.md:108-131`). Aborting panics cannot produce an
   in-process outcome (`design.md:81-83`).
4. [cross-ref] The drain discards `JoinError` values before the Stop scan and
   failure accounting (`crates/camel-processor/src/multicast_segment.rs:217-223,225-310`).
   Recipient-list already maps panic `JoinError` values to `ProcessorError`
   and excludes cancellation (`crates/camel-processor/src/recipient_list.rs:110-121`).

**Outcome:** refine. The test location now names the existing in-file module.
The implementation approach and scope stand.
**Self-grill mode:** self-grill-proposals skill
