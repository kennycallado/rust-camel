# CQRS RuntimeBus for Route Lifecycle Control

Route lifecycle mutations go through `RuntimeCommandBus` and reads through `RuntimeQueryBus`, backed by projections and an optional redb event journal. Direct mutable state would be simpler, but the CQRS model gives us crash recovery, command deduplication, and projection-backed reads that don't block the command path.

The added architectural weight is justified because route lifecycle operations (start, stop, hot-reload) are infrequent, and the journal makes the control plane auditable and recoverable without adding a separate database dependency.

## Amendment: two-phase lifecycle persistence

ADR-0018 refines this decision: lifecycle commands that perform runtime side effects persist intent first, expose intermediate states such as `Starting` through projections, then confirm success or compensate to `Failed`. Aggregate writes use optimistic versions so command handling, projections, event publication, and journal replay stay consistent under concurrent lifecycle operations.

## Clarification: synchronous-projection CQRS (amended by ADR-0045)

This is **synchronous-projection CQRS**, not eventual-consistency CQRS: the read-side projection is
updated within the same optimistic-versioned UnitOfWork as the command that produced it, so there is
no projection lag on the read path. Supervision decisions that read route status therefore see a
state consistent with the last completed command. "Strong consistency" here refers to the read-model
freshness guarantee, not CAP-theorem linearizability. CQRS scope is the **control plane only**; the
data plane (Exchange/Pipeline processing) is not CQRS — see ADR-0045 §3.
