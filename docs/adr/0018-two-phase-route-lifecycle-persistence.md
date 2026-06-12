# ADR-0018: Two-Phase Route Lifecycle Persistence

**Date:** 2026-06-12
**Status:** Accepted
**Amends:** ADR-0002, ADR-0003, ADR-0004, ADR-0007

## Decision

Route lifecycle commands that perform runtime side effects persist control-plane intent before executing the side effect, then confirm or compensate after the side effect returns.

For `StartRoute`, the Runtime records `RouteStartRequested`, projects the Route as `Starting`, starts the runtime Consumer/Pipeline, then records `RouteStarted` and projects `Started`. If the runtime side effect fails after intent was persisted, Runtime records `RouteFailed`, projects `Failed`, and publishes failure events. Non-atomic lifecycle flows use the same compensation rule when a side effect fails after repository or projection state changed.

Route aggregate writes use optimistic version checks. The expected version is captured before mutation; compensation captures the stored version before `fail()` increments it. Journal replay, repository state, projections, and published events therefore agree on the same lifecycle sequence.

## Context

The previous lifecycle path could make runtime side effects and persisted lifecycle state disagree under failures. A start attempt can fail after accepting intent, and hot reload / supervision can race with operator commands. Because RuntimeBus is CQRS with optional journaling, control-plane history must be monotonic and replayable; pretending a persisted intent never happened creates divergent projections after crash recovery.

`Starting` is externally observable through `RouteStatusProjection`. It is intentionally visible to operators because it marks accepted intent that has not yet become a running Consumer.

## Considered Options

### Persist only after side effect succeeds

Rejected. Simpler happy path, but a crash between side effect success and persistence loses the lifecycle transition and can orphan a live Consumer outside the journal.

### Roll back persisted intent on side-effect failure

Rejected. Rollback creates a second hidden state machine and conflicts with append-only event replay. Future readers would see missing history rather than accepted intent followed by failure.

### Persist intent, then confirm or compensate

Accepted. It makes `Starting` visible and requires more projection reconciliation code, but preserves auditable history, replay consistency, and optimistic concurrency boundaries.

## Consequences

- `Starting` is a public lifecycle state, not an implementation detail.
- `RouteRuntimeAggregate::fail()` increments version so replayed `RouteFailed` events and live compensation paths converge.
- Repository writes must use `save_if_version` or an atomic unit-of-work port with the same expected-version semantics.
- Projection updates after non-unit-of-work writes must reconcile from aggregate state; failure to persist compensation is a system-broken inconsistency requiring manual reconciliation.
- Hot reload and route-supervised Consumer failure remain Route lifecycle events, not data-plane mutations.
