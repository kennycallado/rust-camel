use camel_api::CamelError;
use camel_component_api::NetworkRetryPolicy;

#[derive(Debug, Clone)]
pub struct MasterUriConfig {
    pub lock_name: String,
    pub delegate_uri: String,
}

impl MasterUriConfig {
    pub fn parse(uri: &str) -> Result<Self, CamelError> {
        let mut parts = uri.splitn(3, ':');
        let scheme = parts.next().unwrap_or_default();
        let lock_name = parts.next().unwrap_or_default();
        let delegate_uri = parts.next().unwrap_or_default();
        if scheme != "master" || lock_name.is_empty() || delegate_uri.is_empty() {
            return Err(CamelError::InvalidUri(format!(
                "{uri}: expected master:<lockname>:<delegate-uri>"
            )));
        }
        Ok(Self {
            lock_name: lock_name.to_string(),
            delegate_uri: delegate_uri.to_string(),
        })
    }
}

/// Per-component reconnect default: unlimited retries (max_attempts=0),
/// preserving the previous `None = unlimited` behavior. Operators can opt
/// into bounded retry via TOML `[components.master.reconnect]`.
fn master_reconnect_default() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 0, // unlimited
        ..NetworkRetryPolicy::default()
    }
}

/// Configuration for the master/leader-election component.
///
/// Controls drain timeout for graceful delegate shutdown and reconnection policy.
///
/// ## Backward compatibility
///
/// The `delegate_retry_max_attempts` field is retained as a backward-compat alias.
/// When set (not `None`), it bridges into `reconnect.max_attempts` during construction
/// in `MasterComponent::new()`. If `reconnect` is also explicitly configured, the
/// explicit `reconnect` value wins.
#[derive(Debug, Clone)]
pub struct MasterComponentConfig {
    /// Timeout in milliseconds for draining a delegate consumer on leadership loss.
    pub drain_timeout_ms: u64,
    /// Structured reconnection policy, replacing the flat `delegate_retry_max_attempts`
    /// field for new configs. Default: unlimited (`max_attempts=0`).
    pub reconnect: NetworkRetryPolicy,
    /// Backward-compat alias for `reconnect.max_attempts`. `None` means unlimited.
    /// Bridged into `reconnect` during `MasterComponent::new()`.
    pub delegate_retry_max_attempts: Option<u32>,
}

impl MasterComponentConfig {
    /// Create a new config with the given drain timeout and retry limit.
    pub fn new(drain_timeout_ms: u64, delegate_retry_max_attempts: Option<u32>) -> Self {
        Self {
            drain_timeout_ms,
            reconnect: master_reconnect_default(),
            delegate_retry_max_attempts,
        }
    }
}

impl Default for MasterComponentConfig {
    fn default() -> Self {
        Self {
            drain_timeout_ms: 5000,
            reconnect: master_reconnect_default(),
            delegate_retry_max_attempts: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_config_has_reconnect_policy() {
        let cfg = MasterComponentConfig::default();
        // Default should be unlimited (max_attempts=0) to preserve old None behavior
        assert_eq!(cfg.reconnect.max_attempts, 0);
        assert!(cfg.reconnect.enabled);
        // Backward-compat field defaults to None
        assert_eq!(cfg.delegate_retry_max_attempts, None);
    }

    #[test]
    fn master_config_default_reconnect_is_unlimited() {
        let policy = master_reconnect_default();
        assert_eq!(policy.max_attempts, 0);
        assert!(policy.enabled);
        // Unlimited means should_retry always returns true
        assert!(policy.should_retry(0));
        assert!(policy.should_retry(100));
        assert!(policy.should_retry(10_000));
    }

    #[test]
    fn master_config_new_preserves_drain_timeout() {
        let cfg = MasterComponentConfig::new(10_000, Some(5));
        assert_eq!(cfg.drain_timeout_ms, 10_000);
    }
}
