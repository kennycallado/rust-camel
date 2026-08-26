## ADDED Requirements

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
