# camel-platform-kubernetes

Kubernetes-native implementations of the rust-camel Platform SPI (`camel-api`) for distributed runtime coordination. This crate provides pod identity discovery from Downward API environment variables, per-lock Lease-based leader election, and pod readiness-gate updates via the Kubernetes API.

## Installation

```toml
[dependencies]
camel-platform-kubernetes = "0.7"
kube = "0.99"
```

## Usage

### `KubernetesPlatformService` (recommended)

The easiest way to use the Kubernetes platform is via `Camel.toml`:

```toml
[platform]
type = "kubernetes"
namespace = "default"
lease_name_prefix = "camel-"
lease_duration_secs = 15
renew_deadline_secs = 10
retry_period_secs = 2
jitter_factor = 0.2
```

This wires automatically through `CamelConfig::configure_context()` when the `kubernetes` feature is enabled in `camel-config`.

### Manual wiring

```rust
use std::sync::Arc;
use std::time::Duration;

use camel_api::PlatformIdentity;
use camel_core::context::CamelContextBuilder;
use camel_platform_kubernetes::{
    KubernetesLeadershipService, KubernetesPlatformConfig, KubernetesPlatformService,
};

let config = KubernetesPlatformConfig {
    namespace: "default".to_string(),
    lease_name_prefix: "camel-".to_string(),
    lease_duration: Duration::from_secs(15),
    renew_deadline: Duration::from_secs(10),
    retry_period: Duration::from_secs(2),
    jitter_factor: 0.2,
};

let platform = Arc::new(KubernetesPlatformService::try_default(config).await?);

let ctx = CamelContextBuilder::new()
    .platform_service(platform)
    .build()
    .await?;
```

### Per-lock leadership

Each `master:<lockname>:<delegate-uri>` route requests leadership for its specific lock:

```rust
// In camel-master internally:
let handle = platform_service.leadership().start("orders").await?;
if handle.is_leader() {
    // start delegate consumer
}
```

### `KubernetesPlatformIdentity`

```rust
use camel_platform_kubernetes::KubernetesPlatformIdentity;

let identity = KubernetesPlatformIdentity::from_env();
```

### `KubernetesReadinessGate`

```rust
use std::sync::Arc;

use camel_platform_kubernetes::KubernetesReadinessGate;

let client = kube::Client::try_default().await?;
let pod_name = std::env::var("POD_NAME")?;
let namespace = std::env::var("POD_NAMESPACE")?;

let readiness_gate = Arc::new(KubernetesReadinessGate::new(client, &namespace, pod_name));
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
