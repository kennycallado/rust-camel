# camel-platform-kubernetes

Kubernetes-native implementations of the rust-camel Platform SPI (`camel-api`) for distributed runtime coordination. This crate provides pod identity discovery from Downward API environment variables, Lease-based leader election, and pod readiness-gate updates via the Kubernetes API.

## Installation

```toml
[dependencies]
camel-core = "0.6"
camel-platform-kubernetes = "0.6"
kube = "0.99"
```

## Usage

### `KubernetesPlatformIdentity`

```rust
use camel_core::context::CamelContextBuilder;
use camel_platform_kubernetes::KubernetesPlatformIdentity;

let identity = KubernetesPlatformIdentity::from_env().into_platform_identity();

let ctx = CamelContextBuilder::new()
    .platform_identity(identity)
    .build()
    .await?;
```

### `KubernetesLeaderElector`

```rust
use std::{sync::Arc, time::Duration};

use camel_core::context::CamelContextBuilder;
use camel_platform_kubernetes::{KubernetesLeaderElector, LeaderElectorConfig};

let client = kube::Client::try_default().await?;

let elector = Arc::new(KubernetesLeaderElector::new(
    client,
    LeaderElectorConfig {
        lease_name: "camel-leader".to_string(),
        namespace: "default".to_string(),
        lease_duration: Duration::from_secs(15),
        renew_deadline: Duration::from_secs(10),
        retry_period: Duration::from_secs(2),
    },
));

let ctx = CamelContextBuilder::new()
    .leader_elector(elector)
    .build()
    .await?;
```

### `KubernetesReadinessGate`

```rust
use std::sync::Arc;

use camel_core::context::CamelContextBuilder;
use camel_platform_kubernetes::KubernetesReadinessGate;

let client = kube::Client::try_default().await?;
let pod_name = std::env::var("POD_NAME")?;
let namespace = std::env::var("POD_NAMESPACE")?;

let readiness_gate = Arc::new(KubernetesReadinessGate::new(client, &namespace, pod_name));

let ctx = CamelContextBuilder::new()
    .readiness_gate(readiness_gate)
    .build()
    .await?;
```

## Required Downward API environment variables

`KubernetesPlatformIdentity::from_env()` reads:

- `POD_NAME`
- `POD_NAMESPACE`
- `POD_NODE_NAME`
- `POD_SERVICE_ACCOUNT`

Example pod env configuration:

```yaml
env:
  - name: POD_NAME
    valueFrom:
      fieldRef: { fieldPath: metadata.name }
  - name: POD_NAMESPACE
    valueFrom:
      fieldRef: { fieldPath: metadata.namespace }
  - name: POD_NODE_NAME
    valueFrom:
      fieldRef: { fieldPath: spec.nodeName }
  - name: POD_SERVICE_ACCOUNT
    valueFrom:
      fieldRef: { fieldPath: spec.serviceAccountName }
```

## Required RBAC (Lease election)

For leader election with `coordination.k8s.io/v1` Leases, grant your service account permissions on `leases` in the target namespace:

- verbs: `get`, `create`, `update`, `patch`, `delete`

If you use `KubernetesReadinessGate`, also allow patching pod status:

- resource: `pods/status`
- verbs: `patch`

## Running K3s integration tests

K3s integration tests are ignored by default and require Docker with privileged containers.

```bash
cargo test -p camel-platform-kubernetes --test k3s_integration -- --ignored
```
