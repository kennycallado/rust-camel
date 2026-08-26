## ADDED Requirements

### Requirement: Leader self-fencing on renewal deadline

The leadership service SHALL bound every Lease reconcile attempt by the
remaining renewal budget, where the budget is `renew_deadline` minus the time
since the last successful renewal. While leading, the service SHALL step down
— clear its leader flag and emit `StoppedLeading` — when the budget is
exhausted without a successful renewal, independent of any observed Lease
state. The service SHALL NOT step down on a failed attempt while budget
remains; it SHALL keep leading and retry.

#### Scenario: transient failure within budget

- **GIVEN** a holder that renewed successfully at time T and a reconcile
  attempt at T+2s that fails, with `renew_deadline` of 10s
- **WHEN** the failure is processed
- **THEN** the holder remains leader, no `StoppedLeading` is emitted, and the
  next attempt is scheduled after a jittered `retry_period` capped to the
  remaining budget

#### Scenario: optimistic conflict within budget

- **GIVEN** a holder whose renewal returns an optimistic-concurrency conflict
  (stale generation) at T+2s, with `renew_deadline` of 10s
- **WHEN** the conflict is processed
- **THEN** the holder remains leader under budget grace — the conflict is not
  treated as observed foreign ownership

#### Scenario: budget exhaustion

- **GIVEN** a holder whose last successful renewal was at time T and attempts
  failing ever since, with `renew_deadline` of 10s
- **WHEN** a failure is processed at or after T+10s, or the budget is found
  exhausted before an attempt
- **THEN** the holder clears its leader flag and emits `StoppedLeading`
  without reading the Lease, and no post-failure sleep extends past the
  budget boundary

#### Scenario: hanging attempt is bounded

- **GIVEN** a holder whose reconcile attempt hangs (network partition)
- **WHEN** the remaining budget elapses during the await
- **THEN** the await resolves as a failed attempt at the budget boundary and
  the holder steps down; the await never exceeds the remaining budget

#### Scenario: loss observed while leading

- **GIVEN** a holder that receives a successful reconcile result stating
  another holder owns a valid Lease
- **WHEN** the result is processed
- **THEN** the holder steps down immediately, without budget grace

### Requirement: Renewal cadence while leading

While leading, the leadership service SHALL schedule renewal attempts at a
jittered `retry_period` cadence measured from the previous attempt, so the
renewal budget spans multiple attempts.

#### Scenario: cadence between renewals

- **GIVEN** a holder with `retry_period` of 2s that just renewed successfully
- **WHEN** the next cycle is scheduled
- **THEN** the sleep is the jittered `retry_period`, not `renew_deadline`

#### Scenario: recovery within budget continues leading

- **GIVEN** a holder that failed one attempt at T+2s and succeeds at T+4s,
  within `renew_deadline` of 10s
- **WHEN** the success is processed
- **THEN** the holder remains leader with no leadership event emitted and the
  budget resets from the new success time

### Requirement: Self-fencing documentation

The documentation SHALL describe the self-fencing semantics: attempts bounded
by the renewal budget, no step-down while budget remains, step-down at budget
exhaustion independent of observed Lease state, and renewal at jittered
`retry_period` cadence while leading.

#### Scenario: operator reads the docs

- **GIVEN** an operator running the master component on Kubernetes
- **WHEN** the operator reads the Kubernetes platform documentation
- **THEN** the leader-election section states when a leader fences itself
  off during API-server connectivity loss and how transient failures are
  tolerated
