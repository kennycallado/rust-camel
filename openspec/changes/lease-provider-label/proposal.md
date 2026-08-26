# Proposal: lease-provider-label

## Why

bd rc-j94g (Apache Camel parity): NativeLeaseResourceManager in
Apache Camel sets the label `provider=camel` on lease metadata so
operators can filter camel-owned leases among the many leases a
cluster hosts (`kubectl get leases -l provider=camel`). The rust
kubernetes platform creates its election Lease without labels, so
camel-owned leases are indistinguishable from any other
component's. Non-correctness; ops convenience.

## What Changes

- Set `labels: Some({"provider": "camel"})` on the Lease ObjectMeta
  at first-time creation in
  `crates/platforms/camel-platform-kubernetes/src/platform_service.rs`
  (`reconcile_lease`, the `Lease {` construction). Renew and takeover
  paths already `replace` the previously fetched Lease object, so
  existing labels (including ours) round-trip unchanged — no change
  needed there.
- Unit test asserting the created Lease carries the label; existing
  k8s integration harness (real API) stays untouched (infra-gated in
  CI).

Excluded: labeling anything other than the Lease (ConfigMaps,
deployments); making the label configurable (Apache Camel hardcodes
it — parity wins); touching holderIdentity/epoch logic.

## Acceptance criteria

- A lease created by the platform carries label
  `provider=camel` alongside the existing name and annotations.
- Renewal/takeover of a pre-existing (label-less) lease does not
  strip or add labels beyond what it fetched.
- `cargo test -p camel-platform-kubernetes` green including the new
  assertion; quality gates green (Rust changed).

## Risk budget

Single-field addition on an object we already fully construct at
creation; no protocol, election, fencing, or timing semantics
touched. Label is constant — no cardinality or injection surface.
In bounds: the creation-site labels field, tests, and a
behavior-preserving extraction of the first-time Lease construction
into a pure helper for deterministic unit testing. Out of bounds:
any change to renewal, takeover, fencing, timing, or configuration
semantics.
