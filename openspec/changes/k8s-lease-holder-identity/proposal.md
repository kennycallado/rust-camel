# Proposal: k8s-lease-holder-identity

## Why

Kubernetes Leases written by `KubernetesLeadershipService` can carry an empty
`holderIdentity`. `KubernetesPlatformIdentity::from_env()` reads `POD_NAME` with
`unwrap_or_default()`, so a pod without the Downward API env var resolves
`node_id` to `""` silently. The leadership service then writes
`holderIdentity: Some("")` on create, renew, takeover, and release.

Nothing validates the identity. `KubernetesPlatformConfig::validate()` checks
lease timings only; `KubernetesLeadershipService::new` never checks `node_id`.

In a distributed deployment every under-provisioned pod shares the identity
`""`. In `reconcile_lease`, `is_ours = holder == Some("")` is true for all
contenders, so two pods can renew the same Lease at the same time. Result:
split-brain and duplicate consumption on `master:` routes. The ADR-0035
leader-term epoch does not prevent this — epochs diverge only on takeover, and
two empty-identity renewers never take over each other.

Apache Camel (verified on `apache/camel` main) resolves the pod name as
explicit config → `HOSTNAME` env → local hostname, and throws
`RuntimeCamelException` when it cannot resolve. Holder identity is never empty.

bd issue: rc-0gln (P1). Advisory: e_opus, 2026-08-25 (verdict: MODIFY — add
namespaced holder format and unify namespace resolution).

## What Changes

Included:

- Strict identity resolution: `POD_NAME` → `HOSTNAME` → local hostname; a new
  `try_from_env()` fails with `PlatformError::Config` when all sources are
  empty. A `warn!` fires when resolution uses a fallback rather than `POD_NAME`.
- Namespaced holder identity: `holderIdentity = "<namespace>/<node_id>"`, where
  the namespace is resolved once (`config.namespace` → `POD_NAMESPACE` →
  `"default"`) and threaded into both the Lease API scope and the holder string.
  This removes the current divergence where `from_env` and `camel-config`
  resolve `POD_NAMESPACE` with different fallbacks.
- Defense in depth: `KubernetesLeadershipService::new` rejects an empty
  `node_id` with `PlatformError::Config`.
- Tests: fallback chain, empty-identity rejection, and the round-trip proof
  that a Lease written by one identity is recognized as `is_ours` only by that
  identity (the anti-split-brain test).
- Docs: `POD_NAME` via Downward API is required in production; document the
  holder format and the migration note: the first post-upgrade acquisition
  rewrites each Lease's holder; the format change does not bypass lease expiry
  or optimistic concurrency.

Excluded (filed as bd follow-ups, `discovered-from: rc-0gln`): leader
self-fencing on loss of API-server connectivity (rc-0gln child, P1, own
change); `provider=camel` Lease label (P2); MST-001 metrics wiring (P2);
`drain_timeout` and clock-skew review (P3).

## Acceptance criteria

- A pod without `POD_NAME`, `HOSTNAME`, or a local hostname fails platform
  construction with a config error — it never participates in leader election.
- `holderIdentity` on every Lease written by this service is non-empty and has
  the form `<namespace>/<node_id>`.
- Two different identities never both evaluate `is_ours == true` for the same
  Lease; unit tests prove the round-trip through renew and release guards.
- `kubectl get lease` shows an operator-readable leader.

## Risk budget

Acceptable: one extra leadership transition during the rolling deploy that
introduces the new holder format (old pods held bare or empty identities).
Out of bounds: any behavior change to election timing, epoch handling, or the
`LeadershipService` trait; any fix for the loss-of-API-server self-fencing gap
(that is a separate change with its own design pass).
