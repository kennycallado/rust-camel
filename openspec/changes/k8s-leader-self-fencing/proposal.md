# Proposal: k8s-leader-self-fencing

## Why

bd rc-dcsg (P1, from the e_opus advisory on rc-0gln) claims the leader
"keeps `is_leader=true` until the lease visibly expires" during an API-server
partition. Source analysis of the reconcile loop
(`platform_service.rs:234-306`) corrects that premise — the real defects are
two opposite failure modes plus a cadence flaw:

1. **Unbounded await.** `reconcile_lease` runs with no timeout. On a
   black-hole partition the loop hangs mid-await with `is_leader=true` for as
   long as the TCP stack takes to give up (minutes). The delegate keeps
   consuming while peers that can reach the API server see the lease expire
   and take over — the transient dual consumption the advisory flagged.
2. **Fast-fail flap.** Any quick `Err` (connection refused, DNS blip) maps to
   `(false, None)` and immediately emits `StoppedLeading` — one transient
   failure tears down and later restarts the delegate consumer. Fail-closed,
   but with severe availability churn for single blips.
3. **Renewal cadence.** While leading, the loop sleeps
   `renew_deadline - elapsed` (~10s with defaults) for a 15s lease — one
   renewal attempt per lease cycle, leaving no room for in-budget retries.

## What Changes

Replace both failure modes with one mechanism — renew-deadline-driven
self-fencing (client-go leaderelection semantics):

- The loop tracks `last_successful_renewal`. Every renewal attempt is bounded
  by `tokio::time::timeout(remaining_budget)` where
  `remaining_budget = renew_deadline - since_last_success`; hangs become
  deadline errors at the budget boundary. No unbounded awaits. The loop
  checks the budget before each attempt: an exhausted budget steps down
  without attempting, and a post-failure sleep is capped to the remaining
  budget so a failure at T+9.5s never fences later than T+10s.
- A failed attempt does NOT step down while budget remains: the holder keeps
  leading and retries after a jittered `retry_period` (no flap). A renewal
  409 (optimistic conflict) maps to failed-with-grace, not to lease loss —
  the server proved only a stale generation, not foreign ownership.
- When `renew_deadline` elapses since the last successful renewal without a
  success, the holder steps down locally — `StoppedLeading` emitted,
  `is_leader=false` — independent of observed lease state (self-fencing).
  Because `renew_deadline < lease_duration` (validated invariant), the holder
  fences itself before peers can legally take over, modulo Kubernetes clock
  skew on Lease timestamps (renew times use holder wall clocks).
- While leading, renewal attempts run at jittered `retry_period` cadence
  (replacing `renew_deadline`-cadence), so the budget spans multiple attempts.
- Decision logic is extracted into a pure, injectable state machine
  (outcome → action) so failure sequences — transient blips, exhausted
  budgets, hanging attempts — are unit-testable without a cluster.

Excluded: k3s partition simulation (testcontainers cannot black-hole a
network); leader-election timing changes; the `LeadershipService` trait;
ADR-0035 epoch handling (unchanged — epochs still gate downstream sinks).

## Acceptance criteria

- A leader whose attempts fail transiently within `renew_deadline` never
  emits `StoppedLeading` (unit-tested over simulated outcome sequences).
- A leader whose attempts fail or hang beyond `renew_deadline` since its last
  successful renewal emits `StoppedLeading` within the budget window, without
  observing the lease.
- No await on `reconcile_lease` exceeds the remaining budget.
- All existing canon scenarios (holder identity, exclusivity) still pass.

## Risk budget

Acceptable: increased Lease API traffic while leading (`retry_period` vs
`renew_deadline` cadence — client-go parity, bounded by jitter).
Out of bounds: changes to election correctness (takeover, epoch terms),
the camel-master delegate drain path, or `lease_is_expired` semantics.
