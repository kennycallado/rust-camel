# Proposal: k8s-lease-epoch

## What

Clamp the fencing epoch monotonically on the renewal path. When a leader
renews and the server reports a leader-term lower than the locally stored
epoch, the defensive update currently writes the regression over the local
value. It SHALL instead keep the maximum and log the ignored regression.

## Why

- bd rc-begz (P1), discovered in e_opus's holistic blessing of rc-dcsg
  (2026-08-26): an operator deleting a live Lease between cycles makes the
  leader recreate it with `camel.io/leader-term=1`, and the
  `ContinueLeading` defensive update writes epoch 1 over N — a one-time
  epoch regression. The fencing token (ADR-0035) is the mechanism downstream
  consumers use to reject stale leaders; a regressed epoch can make a stale
  writer look current.
- Adversarial-only trigger (operator misconfig), server-authoritative and
  uniform across pods — but the clamp is cheap, pure, and removes the
  renewal-path regression class (fresh-leadership adoption stays
  server-authoritative by design).
- e_opus pre-identified the remedy: "ignore epoch regressions on the renewal
  path (only accept term >= current while leading) or clamp to
  max(current, server_term)".

## What Changes

- New pure helper in `leadership_fsm`: given the current epoch and the
  server-observed term, return the epoch to store — increases accepted,
  equal kept, regressions ignored (caller logs).
- `apply_action`'s `ContinueLeading` arm uses the helper; a regression is
  logged at `warn` with both values (observable, actionable).
- Unit tests for the helper truth table + the decision-table tests stay
  green.

## Impact

- Affected: `crates/platforms/camel-platform-kubernetes`
  (leadership_fsm.rs, platform_service.rs). No public API change; behavior
  tightens (epoch never regresses while leading).
- specs: `kubernetes-leadership` gains one requirement
  (renewal-path epoch monotonicity).
- Risk: a legitimately re-numbered lease fleet (term reset to 1 after
  operator teardown of ALL leases) pins the LOCAL epoch at the old maximum
  until the pod next enters leadership fresh (BecomeLeader on
  step-down-and-reacquire, or restart); GLOBAL fencing recovers only when
  the server term exceeds the pre-reset maximum. Accepted: fencing tokens
  must err monotonic — a stale-high epoch harms availability, never
  safety.
