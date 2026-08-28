# Design: k8s-lease-epoch

## Context

The fencing epoch is stored from the Lease's server-authoritative
`camel.io/leader-term` annotation (ADR-0035). `BecomeLeader` stores
unconditionally (takeover — correct). `ContinueLeading`'s defensive update
(platform_service.rs:272-278) also stores unconditionally whenever the
observed term differs — including lower. An operator deleting a live Lease
makes the leader recreate it at term 1, regressing epoch N→1.

## Goals / Non-Goals

- Goals: epoch monotonic while leading; regressions observed-and-logged;
  pure, unit-testable decision.
- Non-Goals: changing `BecomeLeader` (it fires only on the local
  transition into leadership — `currently_leader == false`; an `Acquired`
  result while already leader routes to `ContinueLeading`, including the
  delete/recreate path); detecting fleet-wide term resets; persisting
  epochs across restarts.

## Decisions

### D1 — Pure clamp helper + private renewal updater

`leadership_fsm::clamp_epoch(current: u64, observed: Option<u64>) -> EpochUpdate`
with `enum EpochUpdate { Keep, Store(u64) }`: `None` → Keep (annotation
stripped — existing only-if-Some semantics); `Some(t)` where `t > current`
→ Store(t); else Keep. `platform_service` gains a small private
`apply_renewal_epoch(leader_epoch: &AtomicU64, observed: Option<u64>) -> u64`
that loads, calls `clamp_epoch`, conditionally stores, and returns the
prior value — pure of logging. A module-level private
`note_renewal_epoch(leader_epoch: &AtomicU64, observed: Option<u64>, lease_name: &str)`
wraps it: calls the updater, compares prior value vs observation, emits the
regression warning. The `apply_action` closure delegates to
`note_renewal_epoch` — extracted at module scope precisely so the warning
branch is directly testable without a live-lease mock.

Alternatives rejected: clamping inside `decide` — the fsm does not hold the
stored epoch (`LoopState` has no epoch; the AtomicU64 lives in the task),
and threading it through would widen the fsm surface for one comparison.
Leaving the logic inline in the closure — untestable; the defect site IS
the store path.

### D2 — Regression logged at warn inside note_renewal_epoch

`note_renewal_epoch` emits `warn!` with lease name, prior epoch, observed
term, and the words "ignoring epoch regression". Not an error: leadership
itself is healthy. `apply_renewal_epoch` stays logging-free.

### D3 — Test seam

Two layers: (1) `clamp_epoch` truth-table unit tests in
`leadership_fsm.rs`'s existing `mod tests`; (2) direct unit tests of the
private `apply_renewal_epoch` in `platform_service.rs`'s test module,
asserting actual `AtomicU64` state transitions for all four observations
(None, equal, increase, regression) and the prior-value return. Plus
captured-log tests of `note_renewal_epoch` for the warning contract
(module-scope fn — directly callable from tests; no closure extraction
gymnastics): the regression path emits the "ignoring epoch regression"
warning (with lease name and both values), and the equal/None paths emit
no epoch-update or epoch-regression log. Test names:
`apply_renewal_epoch_*` (4 atomic-state cases) and
`note_renewal_epoch_regression_logs_warning` /
`note_renewal_epoch_equal_and_none_emit_no_epoch_log` (captured-log,
using the crate's existing tracing-capture test idiom if present, else a
`tracing_subscriber` fmt layer with `MakeWriter` into a shared buffer).

## Risks / Trade-offs

- Fleet-wide term reset pins the LOCAL epoch at the old max until the
  next BecomeLeader (step-down-and-reacquire or restart); GLOBAL fencing
  recovers only when the server term exceeds the pre-reset maximum.
  Fencing correctness dominates: a stale-high epoch harms availability
  (current leader outranked), never safety. The clamp removes the
  renewal-path regression class; adoption on fresh leadership remains
  server-authoritative by design.
- `Renewed { term: Option<u64> }` semantics unchanged — `None` still means
  "server stripped the annotation, keep local".

## Migration Plan

None — behavior tightens silently for conforming servers.

## Open Questions

None.
