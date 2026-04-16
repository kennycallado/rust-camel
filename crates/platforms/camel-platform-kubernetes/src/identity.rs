//! Kubernetes platform identity, auto-detected from Downward API environment variables.
//!
//! Reads pod identity from the following environment variables (set via the Downward API):
//! - `POD_NAME` — name of the current pod
//! - `POD_NAMESPACE` — namespace of the current pod
//! - `POD_NODE_NAME` — node where the pod is running
//! - `POD_SERVICE_ACCOUNT` — service account name
//!
//! Falls back to empty strings if variables are absent.

use std::collections::HashMap;

use camel_api::platform::PlatformIdentity;

/// Kubernetes platform identity, auto-detected from Downward API environment variables.
#[derive(Debug, Clone)]
pub struct KubernetesPlatformIdentity {
    pod_name: String,
    namespace: String,
    node_name: String,
    service_account: String,
}

impl KubernetesPlatformIdentity {
    /// Creates a new identity by reading Downward API environment variables.
    pub fn from_env() -> Self {
        Self {
            pod_name: std::env::var("POD_NAME").unwrap_or_default(),
            namespace: std::env::var("POD_NAMESPACE").unwrap_or_default(),
            node_name: std::env::var("POD_NODE_NAME").unwrap_or_default(),
            service_account: std::env::var("POD_SERVICE_ACCOUNT").unwrap_or_default(),
        }
    }

    /// Creates a new identity from explicit values.
    pub fn new(
        pod_name: impl Into<String>,
        namespace: impl Into<String>,
        node_name: impl Into<String>,
        service_account: impl Into<String>,
    ) -> Self {
        Self {
            pod_name: pod_name.into(),
            namespace: namespace.into(),
            node_name: node_name.into(),
            service_account: service_account.into(),
        }
    }

    /// Returns the pod name.
    pub fn pod_name(&self) -> &str {
        &self.pod_name
    }

    /// Returns the namespace, if set.
    pub fn namespace(&self) -> Option<&str> {
        if self.namespace.is_empty() {
            None
        } else {
            Some(&self.namespace)
        }
    }

    /// Returns the node name, if set.
    pub fn node_name(&self) -> Option<&str> {
        if self.node_name.is_empty() {
            None
        } else {
            Some(&self.node_name)
        }
    }

    /// Returns the service account, if set.
    pub fn service_account(&self) -> Option<&str> {
        if self.service_account.is_empty() {
            None
        } else {
            Some(&self.service_account)
        }
    }

    /// Converts into the API-level [`PlatformIdentity`].
    pub fn into_platform_identity(self) -> PlatformIdentity {
        let namespace = if self.namespace.is_empty() {
            None
        } else {
            Some(self.namespace)
        };
        let mut labels = HashMap::new();
        if !self.node_name.is_empty() {
            labels.insert("node_name".to_string(), self.node_name);
        }
        if !self.service_account.is_empty() {
            labels.insert("service_account".to_string(), self.service_account);
        }
        PlatformIdentity {
            node_id: self.pod_name,
            namespace,
            labels,
        }
    }
}

impl From<KubernetesPlatformIdentity> for PlatformIdentity {
    fn from(value: KubernetesPlatformIdentity) -> Self {
        value.into_platform_identity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_empty_identity() {
        let identity = KubernetesPlatformIdentity::new("", "", "", "");
        assert_eq!(identity.pod_name(), "");
        assert!(identity.namespace().is_none());
        assert!(identity.node_name().is_none());
        assert!(identity.service_account().is_none());
    }

    #[test]
    fn test_new_with_values() {
        let identity =
            KubernetesPlatformIdentity::new("my-pod-abc", "production", "node-1", "default");
        assert_eq!(identity.pod_name(), "my-pod-abc");
        assert_eq!(identity.namespace(), Some("production"));
        assert_eq!(identity.node_name(), Some("node-1"));
        assert_eq!(identity.service_account(), Some("default"));
    }

    #[test]
    fn test_into_platform_identity_empty() {
        let identity = KubernetesPlatformIdentity::new("", "", "", "");
        let platform_id: PlatformIdentity = identity.into();
        assert_eq!(platform_id.node_id, "");
        assert!(platform_id.namespace.is_none());
        assert!(platform_id.labels.is_empty());
    }

    #[test]
    fn test_into_platform_identity_with_values() {
        let identity =
            KubernetesPlatformIdentity::new("my-pod-xyz", "staging", "worker-2", "camel-sa");
        let platform_id: PlatformIdentity = identity.into();
        assert_eq!(platform_id.node_id, "my-pod-xyz");
        assert_eq!(platform_id.namespace.as_deref(), Some("staging"));
        assert_eq!(
            platform_id.labels.get("node_name").map(|s| s.as_str()),
            Some("worker-2")
        );
        assert_eq!(
            platform_id
                .labels
                .get("service_account")
                .map(|s| s.as_str()),
            Some("camel-sa")
        );
    }

    #[test]
    fn test_into_platform_identity_partial_values() {
        // Only pod name and namespace set, no node or service account
        let identity = KubernetesPlatformIdentity::new("pod-123", "default", "", "");
        let platform_id: PlatformIdentity = identity.into();
        assert_eq!(platform_id.node_id, "pod-123");
        assert_eq!(platform_id.namespace.as_deref(), Some("default"));
        assert!(platform_id.labels.is_empty());
    }

    #[test]
    fn test_from_env_with_no_vars() {
        // This test validates from_env() doesn't panic when vars are absent
        let identity = KubernetesPlatformIdentity::from_env();
        // We can't assert specific values due to parallel test env leakage,
        // but we can verify it returns valid strings and doesn't panic
        let _ = identity.pod_name();
        let _ = identity.namespace();
        let _ = identity.node_name();
        let _ = identity.service_account();
    }
}
