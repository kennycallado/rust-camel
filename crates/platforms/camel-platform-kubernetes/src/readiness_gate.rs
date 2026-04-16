use async_trait::async_trait;
use camel_api::platform::ReadinessGate;
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::{Patch, PatchParams};
use tracing::warn;

/// Kubernetes readiness gate — patches the pod's `status.conditions` via the Kubernetes API
/// to signal readiness state to the cluster.
///
/// Requires the pod to have a custom readiness condition declared in its spec:
/// `readinessGates: [{conditionType: "camel.apache.org/ready"}]`
pub struct KubernetesReadinessGate {
    pod_api: Api<Pod>,
    pod_name: String,
    condition_type: String,
}

impl KubernetesReadinessGate {
    pub fn new(client: kube::Client, namespace: &str, pod_name: impl Into<String>) -> Self {
        Self {
            pod_api: Api::namespaced(client, namespace),
            pod_name: pod_name.into(),
            condition_type: "camel.apache.org/ready".to_string(),
        }
    }

    pub fn with_condition_type(mut self, condition_type: impl Into<String>) -> Self {
        self.condition_type = condition_type.into();
        self
    }

    async fn patch_condition(&self, status: &str, reason: &str) {
        let patch = serde_json::json!({
            "status": {
                "conditions": [{
                    "type": self.condition_type,
                    "status": status,
                    "reason": reason,
                    "lastTransitionTime": chrono::Utc::now().to_rfc3339()
                }]
            }
        });
        let pp = PatchParams::default();
        if let Err(err) = self
            .pod_api
            .patch_status(&self.pod_name, &pp, &Patch::Merge(&patch))
            .await
        {
            warn!(
                pod_name = %self.pod_name,
                condition_type = %self.condition_type,
                error = %err,
                "failed to patch pod readiness condition"
            );
        }
    }
}

#[async_trait]
impl ReadinessGate for KubernetesReadinessGate {
    async fn notify_ready(&self) {
        self.patch_condition("True", "CamelReady").await;
    }

    async fn notify_not_ready(&self, reason: &str) {
        self.patch_condition("False", reason).await;
    }

    async fn notify_starting(&self) {
        self.patch_condition("False", "Starting").await;
    }
}
