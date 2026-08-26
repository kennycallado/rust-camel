# kubernetes-leadership Specification

## Purpose
TBD - created by archiving change k8s-lease-holder-identity. Update Purpose after archive.
## Requirements
### Requirement: Kubernetes leader identity resolution

The Kubernetes platform SHALL resolve the node identity from the first
non-empty source in the chain `POD_NAME` environment variable, `HOSTNAME`
environment variable, local hostname. The platform SHALL fail platform
construction with a configuration error when every source is empty. The
platform SHALL log a warning when resolution uses `HOSTNAME` or the local
hostname instead of `POD_NAME`.

#### Scenario: POD_NAME set

- **GIVEN** the `POD_NAME` environment variable is set to `my-pod`
- **WHEN** identity resolution runs
- **THEN** the resolved node id is `my-pod` and no fallback warning is logged

#### Scenario: fallback to HOSTNAME

- **GIVEN** `POD_NAME` is unset and `HOSTNAME` is set to `my-host`
- **WHEN** identity resolution runs
- **THEN** the resolved node id is `my-host` and a warning names the fallback
  source

#### Scenario: no resolvable identity

- **GIVEN** `POD_NAME`, `HOSTNAME`, and the local hostname are all empty or
  unavailable
- **WHEN** platform construction runs
- **THEN** construction fails with a `PlatformError::Config` error and no
  leadership service participates in election

### Requirement: Lease holder identity format

The leadership service SHALL write `holderIdentity` as
`<namespace>/<node_id>`, where the namespace is the first non-empty value of
the platform config namespace, the identity namespace, then `default`. The
leadership service SHALL reject construction with a configuration error when
`node_id` is empty. The namespace used in the holder string SHALL be the same
value that scopes the Lease API client.

#### Scenario: namespaced holder written

- **GIVEN** a leadership service constructed with namespace `prod` and node id
  `my-pod`
- **WHEN** the service acquires or renews a Lease
- **THEN** the Lease `holderIdentity` field equals `prod/my-pod`

#### Scenario: default namespace applied

- **GIVEN** a leadership service constructed with an empty config namespace, no
  identity namespace, and node id `my-pod`
- **WHEN** the service acquires a Lease
- **THEN** the Lease `holderIdentity` field equals `default/my-pod`

#### Scenario: empty node id rejected

- **GIVEN** a node id that resolves to the empty string
- **WHEN** the leadership service is constructed
- **THEN** construction fails with a `PlatformError::Config` error

### Requirement: Lease holder exclusivity

The leadership service SHALL evaluate a Lease as held by this node only when
the full holder string written on the Lease equals this node's holder string.
Renewal and release guards SHALL use the same full-string comparison.

#### Scenario: round-trip recognition

- **GIVEN** a Lease whose `holderIdentity` equals the holder string of
  identity A
- **WHEN** identity A reconciles the Lease
- **THEN** A evaluates the Lease as its own and renews it

#### Scenario: foreign holder not recognized

- **GIVEN** a Lease whose `holderIdentity` equals the holder string of
  identity A
- **WHEN** identity B, with a different valid node id, reconciles or tries to
  release the Lease
- **THEN** B evaluates the Lease as not its own and does not renew or release
  it

### Requirement: Identity documentation

The documentation SHALL state that production deployments expose `POD_NAME`
through the Kubernetes Downward API, SHALL document the
`<namespace>/<node_id>` holder format, and SHALL describe the migration
effect: the first post-upgrade acquisition rewrites each Lease's holder, and
the format change does not bypass lease expiry or optimistic concurrency.

#### Scenario: operator reads the docs

- **GIVEN** an operator deploys the master component on Kubernetes
- **WHEN** the operator reads the component documentation
- **THEN** the docs name the required environment variables, the holder format
  visible in `kubectl get lease`, and the post-upgrade holder rewrite
  behavior

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

### Requirement: Kubernetes election lease carries the provider label

The Kubernetes platform SHALL create its leader-election Lease with
the label `provider=camel` on the Lease metadata, so operators can
filter camel-owned leases. The label SHALL NOT be configurable and
SHALL NOT participate in election, fencing, or renewal decisions.
Renewal and takeover of an existing Lease SHALL NOT modify
`metadata.labels` — the labels of the lease as last fetched are sent
back unchanged.

#### Scenario: first-time lease creation carries the label

- **GIVEN** the platform reconciles a lock whose Lease does not yet exist
- **WHEN** `reconcile_lease` creates the Lease
- **THEN** the Lease metadata carries labels exactly containing `provider=camel` alongside its name and the `camel.io/leader-term` annotation initialized to `1`

#### Scenario: renewal does not modify existing labels

- **GIVEN** the platform holds a Lease whose metadata carries arbitrary labels (including none)
- **WHEN** `reconcile_lease` renews that Lease
- **THEN** the replace operation sends `metadata.labels` exactly as last fetched, adding or removing none

#### Scenario: takeover does not modify existing labels

- **GIVEN** a Lease held by another holder whose metadata carries arbitrary labels (including none) has expired
- **WHEN** `reconcile_lease` takes over that Lease
- **THEN** the replace operation sends `metadata.labels` exactly as last fetched, adding or removing none

