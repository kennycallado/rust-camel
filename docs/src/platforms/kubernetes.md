# Kubernetes platform

The Kubernetes platform integrates rust-camel with a Kubernetes cluster.
It provides leader election through Lease objects, readiness patching on
the pod status, and pod identity from the Downward API.

Source: `crates/platforms/camel-platform-kubernetes/`.

## Platform service

`KubernetesPlatformService` implements the `PlatformService` trait. It
binds three parts:

- `KubernetesLeadershipService`: leader election through Kubernetes
  Lease objects.
- `KubernetesReadinessGate`: patches the pod `status.conditions` to
  signal readiness.
- `KubernetesPlatformIdentity`: detects pod name, namespace, and labels
  from the Downward API.

See `crates/camel-api/src/platform.rs` for the trait contracts.

## Leader election

Leader election uses Kubernetes Lease objects from the
`coordination.k8s.io` API group. Each named lock maps to one Lease in
the configured namespace.

The Lease `holderIdentity` has the form `<namespace>/<node_id>`. The
namespace is resolved once: config namespace, then pod namespace, then
`default`. The same value scopes the Lease API client. This is the value
an operator sees in `kubectl get lease`.

The node id resolves from the first non-empty source in the chain
`POD_NAME` → `HOSTNAME` → local hostname. `POD_NAME` comes from the
Downward API. Fallback sources log a warning. When no source resolves,
platform construction fails with a config error. After an upgrade, the
first acquisition rewrites each Lease holder to the new format. This
rewrite does not bypass lease expiry or optimistic concurrency.

The `KubernetesLeadershipService` runs a background loop for each lock:

1. Read the current Lease from the API.
2. If the Lease expired or does not exist, try to acquire it.
3. If this pod holds the Lease, renew it before the lease duration
   expires.
4. If another pod holds a valid Lease, wait and retry.

### Self-fencing

Renewal obeys a renewal budget. The budget is `renew_deadline`, measured
from the last successful renewal. Each renew attempt gets only the
remaining budget. A hung attempt fails when its budget runs out.

A leader that cannot renew within the budget steps down. It emits
`StoppedLeading` and stops its delegate. The step-down does not depend
on the Lease state the leader observes.

Failures within the budget keep the leader in place. This covers
transient API failures and optimistic-concurrency conflicts. The loop
retries at the jittered `retry_period` cadence. The sleep before each
retry is shortened so it never crosses the budget boundary.

Validation enforces `renew_deadline < lease_duration`. The holder
therefore fences itself before the Lease can legally expire for peers.
This ordering holds modulo Kubernetes clock skew on Lease timestamps.

**Reference**: `crates/platforms/camel-platform-kubernetes/CONTEXT.md`
(Self-fencing).

### Fencing token

The Lease carries a `camel.io/leader-term` annotation. This annotation
is a monotonic fencing token. Each takeover increments the term. The
Master component stamps every Exchange from a `master:` route with the
current term. Downstream sinks can reject envelopes that carry a stale
term. See [ADR-0035](../adr/0035-leader-epoch-fencing-token.md).

### Configuration

`KubernetesPlatformConfig` controls the election timing:

| Field | Default | Description |
|---|---|---|
| `namespace` | `""` (auto-detect) | Namespace for Lease objects |
| `lease_name_prefix` | `"camel-"` | Prefix for Lease names |
| `lease_duration` | 15s | Validity duration of a Lease |
| `renew_deadline` | 10s | Renew window before Lease expiry |
| `retry_period` | 2s | Interval between election cycles when not leader |
| `jitter_factor` | 0.2 | Random jitter for retry timing (0.0-1.0) |

Validation rules from `KubernetesPlatformConfig::validate()`:

- `renew_deadline` must be less than `lease_duration`.
- `retry_period` must be less than `renew_deadline`.
- `jitter_factor` must be in the range `[0.0, 1.0]`.

See `crates/platforms/camel-platform-kubernetes/CONTEXT.md` for the
dependency boundary and log-level policy.

## Readiness gate

`KubernetesReadinessGate` patches the pod `status.conditions` through
the Kubernetes API. The pod spec must declare a custom readiness gate:

```yaml
spec:
  readinessGates:
    - conditionType: "camel.apache.org/ready"
```

The gate exposes three transitions:

- `notify_starting()`: sets the condition to `False` with reason
  `"Starting"`.
- `notify_ready()`: sets the condition to `True` with reason
  `"CamelReady"`.
- `notify_not_ready(reason)`: sets the condition to `False` with the
  given reason.

The condition type defaults to `"camel.apache.org/ready"`. Call
`with_condition_type()` to set a custom type.

When you configure a `HealthSource`, the platform service polls
readiness every 10 seconds and updates the gate.

## Master/Leader pattern

Routes with the `master:` scheme activate only on the leader pod. The
URI format is:

```text
master:<lock-name>:<component>:<component-uri>
```

For example, `master:mylock:timer:tick?period=1000` starts only on the
pod that holds the lock named `mylock`. When the pod loses leadership,
the route stops. When the pod re-acquires leadership, the route starts
again.

The `master:` scheme works with any `LeadershipService` implementation.
Use `KubernetesLeadershipService` in production. Use
`NoopLeadershipService` for local testing.

### Example: Rust API

```rust,ignore
{{#include ../../../examples/master-leader/src/main.rs:master-route}}
```

### Example: YAML DSL

```yaml
{{#include ../../../examples/master-leader-yaml/routes/master.yaml:master-yaml-route}}
```

See the full examples:

- `examples/master-leader/`: Rust API with simulated leadership.
- `examples/master-leader-yaml/`: YAML DSL with simulated leadership.
- `examples/kubernetes-platform/`: end-to-end Kubernetes leader election
  with K3s.

## Route activation on the leader

When a route uses the `master:` scheme, the Master component wraps the
consumer with a leadership bridge. The bridge:

1. Subscribes to leadership events from the `LeadershipHandle`.
2. On `StartedLeading`, starts the delegate consumer.
3. On `StoppedLeading`, stops the delegate consumer.
4. Stamps every Exchange with the leader epoch fencing token.

The bridge uses a bounded channel (128-deep) between the delegate
consumer and the pipeline. On delegate stop, the bridge drains its
buffer and exits. On route shutdown, the bridge aborts at once.

See `camel-master/src/leadership.rs` and
[ADR-0035](../adr/0035-leader-epoch-fencing-token.md).

**Reference**: [Platform Kubernetes crate](https://github.com/kennycallado/rust-camel/blob/main/crates/platforms/camel-platform-kubernetes/CONTEXT.md)
