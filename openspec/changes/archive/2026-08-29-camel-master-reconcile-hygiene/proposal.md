# Proposal: camel-master-reconcile-hygiene

## Why

Holistic-review and papal-blessing follow-ups on the Master component (rc-tpv4
/ rc-0gln, 2026-08-26) surfaced three supervision-loop defects and one config
gap:

1. **rc-2f08 (p2)** — the supervision loop re-runs `reconcile_event` on every
   watch delivery. A `StartedLeading` delivery that arrives while the
   delegate is already Active drains and recreates the delegate: wasted
   drain+respawn ("delegate churn") and a `stopped`+`started` lifecycle
   metric pair per duplicate. The platform never emits two consecutive
   identical `Started` events (the FSM guards `BecomeLeader` on
   `!currently_leader`); the observed duplicates are tokio-watch coalesced
   Stopped+Started flaps that arrive as a lone `StartedLeading`. A plain
   boolean edge-gate is therefore wrong — it would also swallow a flap whose
   leader term (fencing epoch) changed, which MUST re-reconcile to restamp
   the epoch bridge (ADR-0035).
2. **rc-h5s8 (p3)** — the initial-snapshot and observed-edge delegate
   acquisitions do not count against `delegate_attempts` (the budget gate
   lives only in the retry-tick arm). At `max_attempts = N` operators see
   N+1 create attempts; README and spec both say "max attempts". Fix:
   at most N create attempts per acquisition epoch (exactly N under
   persistent transient failure), unlimited default unchanged.
3. **rc-ys57 (p3)** — advisory: drain_timeout vs lease_duration overlap is
   undocumented (safety there is epoch fencing, not ordering);
   `KubernetesPlatformConfig::validate` checks ordering but no absolute
   renewal slack for NTP skew; the intended cross-route lock sharing of
   `master:X:` is undocumented; README's `delegate_retry_max_attempts`
   default ("30") is stale (real default: unlimited, `max_attempts=0`).

## What Changes

- Epoch-aware idempotence guard for the `StartedLeading` reconciliation:
  skip delegate recreation only when the Active delegate's epoch equals the
  current `leader_epoch`; a term bump forces re-reconciliation.
- Budget-gate ALL delegate acquisition dispatches (initial snapshot and
  observed edge), not just retry ticks. `max_attempts = N` → at most N
  create attempts per acquisition epoch (exactly N under persistent
  transient failure); unlimited default unchanged.
- `KubernetesPlatformConfig::validate` gains an absolute-slack rule:
  `lease_duration - renew_deadline >= retry_period` (one full retry window
  for clock skew + jitter).
- Docs: camel-master README (drain-vs-lease overlap and epoch fencing,
  cross-route lock sharing, stale default fixed), drain field comment, and a
  stale spec-path citation in `leadership.rs`.

Excluded: camel-api `LeadershipEvent`/handle shape (untouched), the manual
retry loop structure, and the K3s integration harness (`master_kubernetes_test`
stays feature-gated).

Affected crates: `camel-component-master`, `camel-platform-kubernetes`.

## Acceptance criteria

- Duplicate same-term `StartedLeading` while Active: no delegate stop or
  recreate (`create_consumer_calls` stays 1), no lifecycle metric pair.
- Term bump while Active (or coalesced flap with new term): re-reconcile
  exactly once, bridge restamped with the new epoch.
- `max_attempts = 1` + transient create failure: exactly one `create_error`
  observation, then budget-exhausted stop (test asserting two is updated).
- Real edges (not-leading→leading, leading→not-leading) reconcile exactly
  once; rc-tpv4 metric tests stay green.
- Invalid slack (`lease - renew < retry`) rejected by `validate()`; defaults
  (15/10/2) pass.
- README documents drain/lease overlap + fencing, lock sharing; default row
  corrected to `0` = unlimited.

## Risk budget

Risk contained to the Master supervision loop and one platform validator.
Out of bounds: camel-api trait changes, renewal-path epoch semantics,
behavior changes visible with the default unlimited-retry policy, and any
K3s-infra-dependent test. Gate suite: `camel-component-master`,
`camel-platform-kubernetes`, plus `camel-test::master_noop_test`
(camel-config is untouched).

Bd: rc-2f08, rc-h5s8, rc-ys57.
