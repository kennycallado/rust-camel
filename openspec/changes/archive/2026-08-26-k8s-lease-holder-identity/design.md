# Design: k8s-lease-holder-identity

## Approach

Fix the identity at its source and make the holder string unambiguous, in one
atomic change. Four coordinated moves:

1. **Strict resolution with an injectable seam.** Add
   `KubernetesPlatformIdentity::try_from_env() -> Result<Self, PlatformError>`.
   Resolution runs through a private resolver function that accepts injected
   `POD_NAME`, `HOSTNAME`, and local-hostname sources and returns both the
   resolved value and its source, so tests deterministically cover each
   fallback tier, the unavailable/empty local-hostname case, and the fallback
   warning without depending on the host. Chain: `POD_NAME` env → `HOSTNAME`
   env → local hostname (via the `hostname` crate, added as a dependency of
   `camel-platform-kubernetes`). If all are empty or unset, return
   `PlatformError::Config`. When resolution comes from `HOSTNAME` or the local
   hostname (not `POD_NAME`), emit `warn!` naming the fallback source — in
   Kubernetes with `hostNetwork: true` or an overridden `spec.hostname`,
   `HOSTNAME` is the node hostname, so `POD_NAME` is the only authoritative
   source. The existing `from_env()` stays as a deprecated shim that preserves
   its legacy infallible behavior (empty-string defaults) for source
   compatibility; all framework paths migrate to `try_from_env()`. The
   `new(...)` explicit constructor is unchanged. `KubernetesLeadershipService::
   new` rejects legacy empty identities (move 4).

2. **Namespace and holder resolved once, structurally.** `KubernetesLeadershipService::new`
   resolves the namespace — first non-empty of `config.namespace`, then
   `identity.namespace`, then `"default"` — in one normalization function, and
   stores one canonical namespace and one holder string
   `<namespace>/<node_id>` at construction. `try_default` constructs the
   identity strictly (via `try_from_env`) and delegates to the same
   constructor, so every construction path stores the same canonical pair.
   `start`, `reconcile_lease` (create, renew, takeover), and `release_lease`
   use the stored values; the Lease API scope and the holder string derive
   from the same stored namespace. The `is_ours` and release-guard comparisons
   already compare full strings, so a consistently produced namespaced format
   keeps them correct.

3. **No duplicate namespace helpers.** The normalization lives in one
   function used by both `try_default` and `KubernetesLeadershipService::new`.
   The `camel-config` wiring keeps its existing resolution, which now matches
   the normalized result.

4. **Defense in depth.** `KubernetesLeadershipService::new` returns
   `Err(PlatformError::Config)` when `identity.node_id` is empty. A node that
   cannot say who it is must not compete for leadership. This mirrors Apache
   Camel's `RuntimeCamelException` on unresolvable pod name.

The anti-split-brain proof is a pure unit test over `reconcile_lease` /
`release_lease` behavior: a Lease whose `holder_identity` equals identity A's
holder string yields `is_ours == true` for A and `false` for B, where B has a
different valid node id. Empty-identity coverage is the constructor-rejection
test: an identity with an empty `node_id` never reaches reconciliation.

Migration behavior: the first post-upgrade acquisition rewrites each Lease's
holder. The format change does not bypass lease expiry or optimistic
concurrency (`resourceVersion`) conflicts. Also recorded in the docs task.

## Affected crates

- `camel-platform-kubernetes`: strict identity resolution with injectable
  resolver seam; single namespace-normalization function; stored canonical
  namespace and holder; `new()` validation; unit tests. New dependency:
  `hostname`.
- `camel-config`: no behavioral change expected; wiring review only.
- `camel-test`: update `master_kubernetes_test` expected holders to
  `default/test-pod` and `default/config-test-pod`. That suite needs k3s via
  testcontainers; local verification is deferred to CI when Docker is not
  available.
- `docs` (`docs/src/components/master.md`, plus the Kubernetes platform doc if
  one exists): Downward API requirement, holder format, migration behavior.

## Architecture boundaries

Components → Services boundary respected: `camel-master` (component) is
unchanged; it already delegates election to the `PlatformService` SPI
(`camel-api`). All changes live in the platform implementation crate behind
that SPI. No change to the `LeadershipService` trait, `PlatformIdentity`
struct, election timings, or the ADR-0035 leader-term annotation protocol.

Single-phase change: five small tasks, one crate plus docs and a test update;
no milestone grouping needed.

## Alternatives considered

- **Warn + random suffix on empty identity.** Rejected (e_opus): ops still
  cannot correlate the leader to a pod, and a fresh suffix per boot makes every
  restart look like a new holder, forcing takeover churn. Fail-loud instead.
- **Bare pod name as holder (exact Apache Camel parity).** Rejected: our
  fallback chain (and non-K8s runs) can produce colliding bare hostnames; the
  namespaced form costs nothing, is more diagnostic, and we already diverge via
  the leader-term annotation.
- **Defer the format change to a follow-up.** Rejected for atomicity within a
  binary: each binary must use one canonical holder for writes, ownership
  checks, and release guards. A binary that computed two different holder
  strings could misrecognize its own Lease. Mixed old/new pods across a
  rolling deploy intentionally treat each other as foreign holders and remain
  protected by lease expiry plus `resourceVersion` optimistic-concurrency
  conflicts — one canonical format per binary is the invariant that matters,
  and it must ship with the resolution fix.
