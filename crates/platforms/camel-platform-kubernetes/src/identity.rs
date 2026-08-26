//! Kubernetes platform identity, auto-detected from the environment.
//!
//! Node id resolves from the first non-empty source in the chain
//! `POD_NAME` → `HOSTNAME` → local hostname; `try_from_env()` fails with
//! `PlatformError::Config` when all are empty. The deprecated `from_env()`
//! keeps the legacy empty-string defaults.
//!
//! Additional pod fields (set via the Downward API):
//! - `POD_NAMESPACE` — namespace of the current pod
//! - `POD_NODE_NAME` — node where the pod is running
//! - `POD_SERVICE_ACCOUNT` — service account name

use std::collections::HashMap;

use camel_api::platform::{PlatformError, PlatformIdentity};
use tracing::warn;

/// Source from which the node identity was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentitySource {
    PodName,
    HostnameEnv,
    LocalHostname,
}

/// Returns the trimmed value when non-empty, `None` otherwise.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// Resolves the node identity from the first non-empty (trimmed) source in the
/// order PodName → HostnameEnv → LocalHostname, paired with its source.
///
/// Returns [`unresolvable_identity_error`] when every source is empty or absent.
fn resolve_node_identity(
    pod_name: Option<&str>,
    hostname_env: Option<&str>,
    local_hostname: Option<&str>,
) -> Result<(String, IdentitySource), PlatformError> {
    if let Some(name) = non_empty(pod_name) {
        return Ok((name.to_string(), IdentitySource::PodName));
    }
    if let Some(name) = non_empty(hostname_env) {
        return Ok((name.to_string(), IdentitySource::HostnameEnv));
    }
    if let Some(name) = non_empty(local_hostname) {
        return Ok((name.to_string(), IdentitySource::LocalHostname));
    }
    Err(unresolvable_identity_error())
}

/// Error returned when no identity source resolves to a non-empty value.
fn unresolvable_identity_error() -> PlatformError {
    PlatformError::Config(
        "cannot resolve node identity: POD_NAME, HOSTNAME, and local hostname are all empty".into(),
    )
}

/// Operator warning for identity resolved from a fallback source.
///
/// Returns `None` for the authoritative source (`PodName`).
fn fallback_warning(source: IdentitySource) -> Option<&'static str> {
    match source {
        IdentitySource::PodName => None,
        IdentitySource::HostnameEnv => Some(
            "node identity resolved from the HOSTNAME environment variable; \
             POD_NAME via the Downward API is the authoritative source",
        ),
        IdentitySource::LocalHostname => Some(
            "node identity resolved from the local hostname; \
             POD_NAME via the Downward API is the authoritative source",
        ),
    }
}

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
    #[deprecated(
        since = "0.35.0",
        note = "use try_from_env; from_env silently produces an empty node id"
    )]
    pub fn from_env() -> Self {
        Self {
            pod_name: std::env::var("POD_NAME").unwrap_or_default(),
            namespace: std::env::var("POD_NAMESPACE").unwrap_or_default(),
            node_name: std::env::var("POD_NODE_NAME").unwrap_or_default(),
            service_account: std::env::var("POD_SERVICE_ACCOUNT").unwrap_or_default(),
        }
    }

    /// Creates a new identity, failing when no identity source resolves.
    ///
    /// Resolves the pod name from the first non-empty source in the order
    /// `POD_NAME` → `HOSTNAME` → local hostname. When a fallback source is
    /// used, emits a warning that `POD_NAME` via the Downward API is the
    /// authoritative source.
    pub fn try_from_env() -> Result<Self, PlatformError> {
        let pod_name = std::env::var("POD_NAME").ok();
        let hostname_env = std::env::var("HOSTNAME").ok();
        // `hostname::get()` returns the system hostname as an `OsString`;
        // map it to a `String` and treat any failure (including non-UTF-8) as absent.
        let local_hostname = hostname::get()
            .ok()
            .and_then(|host| host.into_string().ok());
        let (pod_name, source) = resolve_node_identity(
            pod_name.as_deref(),
            hostname_env.as_deref(),
            local_hostname.as_deref(),
        )?;
        if let Some(message) = fallback_warning(source) {
            warn!("{message}");
        }
        Ok(Self {
            pod_name,
            namespace: std::env::var("POD_NAMESPACE").unwrap_or_default(),
            node_name: std::env::var("POD_NODE_NAME").unwrap_or_default(),
            service_account: std::env::var("POD_SERVICE_ACCOUNT").unwrap_or_default(),
        })
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
    #[allow(deprecated)]
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

    #[test]
    fn resolver_prefers_pod_name() {
        let resolved = resolve_node_identity(Some("my-pod"), Some("my-host"), Some("local"));
        assert_eq!(
            resolved.expect("pod name present"),
            ("my-pod".to_string(), IdentitySource::PodName)
        );
    }

    #[test]
    fn resolver_falls_back_to_hostname_env() {
        let resolved = resolve_node_identity(None, Some("my-host"), Some("local"));
        assert_eq!(
            resolved.expect("hostname env present"),
            ("my-host".to_string(), IdentitySource::HostnameEnv)
        );
    }

    #[test]
    fn resolver_falls_back_to_local_hostname() {
        let resolved = resolve_node_identity(None, None, Some("local"));
        assert_eq!(
            resolved.expect("local hostname present"),
            ("local".to_string(), IdentitySource::LocalHostname)
        );
    }

    #[test]
    fn resolver_ignores_empty_strings() {
        let resolved = resolve_node_identity(Some(""), Some(""), Some(""));
        let err = resolved.expect_err("all sources empty");
        let expected = match unresolvable_identity_error() {
            PlatformError::Config(message) => message,
            other => panic!("expected Config error, got: {other:?}"),
        };
        assert!(matches!(err, PlatformError::Config(ref message) if *message == expected));
    }

    #[test]
    fn resolver_trims_whitespace() {
        let resolved = resolve_node_identity(Some("  pod  "), None, None);
        assert_eq!(
            resolved.expect("pod name present"),
            ("pod".to_string(), IdentitySource::PodName)
        );
    }

    #[test]
    fn unresolvable_error_names_all_sources() {
        let err = resolve_node_identity(None, None, None).expect_err("no source available");
        let message = match err {
            PlatformError::Config(message) => message,
            other => panic!("expected Config error, got: {other:?}"),
        };
        assert!(message.contains("POD_NAME"), "message: {message}");
        assert!(message.contains("HOSTNAME"), "message: {message}");
        assert!(message.contains("local hostname"), "message: {message}");
    }

    #[test]
    fn fallback_warning_only_for_fallback_sources() {
        assert!(fallback_warning(IdentitySource::PodName).is_none());
        let hostname_env_warning =
            fallback_warning(IdentitySource::HostnameEnv).expect("warning for HOSTNAME fallback");
        assert!(hostname_env_warning.contains("HOSTNAME"));
        let local_hostname_warning = fallback_warning(IdentitySource::LocalHostname)
            .expect("warning for local hostname fallback");
        assert!(local_hostname_warning.contains("local hostname"));
    }
}
