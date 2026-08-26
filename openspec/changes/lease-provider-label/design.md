# Design: lease-provider-label

## Approach

One-line production change plus a test. In
`reconcile_lease` (`crates/platforms/camel-platform-kubernetes/src/platform_service.rs`,
the first-time-create arm), the `Lease` construction gains
`labels: Some(BTreeMap from [("provider", "camel")])` on its
`ObjectMeta`, mirroring the existing `annotations` initialization
one field above. Constant value; matches Apache Camel's
NativeLeaseResourceManager parity requirement (bd rc-j94g).

The renew and takeover paths mutate the fetched Lease in place and
`leases.replace(...)` it — k8s `replace` sends the full object as
fetched+mutated, so pre-existing labels round-trip. A label-less
legacy lease simply stays label-less until recreated; the label is
ops metadata, not protocol state, so partial adoption is harmless.

Test: the create arm is inline in `reconcile_lease`, so the
construction extracts into a small `fn build_first_time_lease(lease_name,
holder_identity, config, now) -> Lease` helper (pure, no API call)
called by the create arm — the test asserts
`lease.metadata.labels == Some({"provider": "camel"})`, the name,
and the leader-term annotation deterministically without a live
API. The label-preservation clauses (renewal/takeover) are
unchanged behavior verified by the existing in-place mutation +
`leases.replace` flow (metadata of the fetched object round-trips);
they get no new unit test — a code-shape assertion is delegated to
review, since k8s `replace` semantics are external to this crate.

## Affected crates

- `camel-platform-kubernetes`: creation-site labels + extracted pure
  helper + unit test. No API change, no dependency change.

## Architecture boundaries

Platforms layer only; consumes `k8s_openapi` types it already
imports (`ObjectMeta`). No component or service crate touched; no
dependency direction change. The label is inert metadata for the
election protocol — leadership FSM (leadership_fsm.rs), fencing
epochs, and renewal timing are untouched.

## Phases

Single-phase: one coherent slice.

## Test strategy

Pure-helper unit test asserting the full ObjectMeta shape (name,
labels `provider=camel`, leader-term annotation "1") plus LeaseSpec
holder/duration fields; existing integration tests
(`kubernetes_test.rs`, `master_kubernetes_test.rs` — infra-gated)
untouched.
