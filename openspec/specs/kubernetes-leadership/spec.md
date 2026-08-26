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

