//! Tests that `KubernetesLeaderElector` and `KubernetesReadinessGate` can be wired
//! into `CamelContextBuilder` via the platform ports API.

use std::sync::Arc;

use camel_core::context::CamelContextBuilder;
use camel_platform_kubernetes::{
    KubernetesLeaderElector, KubernetesReadinessGate, LeaderElectorConfig,
};

fn install_rustls_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Verify that the builder accepts a `KubernetesLeaderElector` as a `LeaderElector` port.
/// This is a compile + wiring test — it does NOT connect to a real cluster.
/// Uses `kube::Client::try_default()` which will fail gracefully in CI without a cluster.
#[tokio::test]
async fn test_wire_leader_elector_into_context() {
    install_rustls_provider();

    let client = match kube::Client::try_default().await {
        Ok(client) => client,
        Err(_) => return,
    };

    let elector = Arc::new(KubernetesLeaderElector::new(
        client,
        LeaderElectorConfig::default(),
    ));
    let ctx = CamelContextBuilder::new()
        .leader_elector(elector)
        .build()
        .await
        .expect("context build should succeed");

    let _ = ctx.leader_elector();
}

/// Verify that the builder accepts a `KubernetesReadinessGate` as a `ReadinessGate` port.
#[tokio::test]
async fn test_wire_readiness_gate_into_context() {
    install_rustls_provider();

    let client = match kube::Client::try_default().await {
        Ok(client) => client,
        Err(_) => return,
    };

    let gate = Arc::new(KubernetesReadinessGate::new(client, "default", "test-pod"));
    let ctx = CamelContextBuilder::new()
        .readiness_gate(gate)
        .build()
        .await
        .expect("context build should succeed");

    let _ = ctx.readiness_gate();
}
