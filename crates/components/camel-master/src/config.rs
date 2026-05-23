use camel_api::CamelError;

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

/// Configuration for the master/leader-election component.
///
/// Controls drain timeout for graceful delegate shutdown and the maximum number
/// of delegate start retry attempts while leading.
#[derive(Debug, Clone)]
pub struct MasterComponentConfig {
    /// Timeout in milliseconds for draining a delegate consumer on leadership loss.
    pub drain_timeout_ms: u64,
    /// Maximum number of times to retry starting the delegate consumer after a failure.
    /// `None` means unlimited retries.
    pub delegate_retry_max_attempts: Option<u32>,
}

impl MasterComponentConfig {
    /// Create a new config with the given drain timeout and retry limit.
    pub fn new(drain_timeout_ms: u64, delegate_retry_max_attempts: Option<u32>) -> Self {
        Self {
            drain_timeout_ms,
            delegate_retry_max_attempts,
        }
    }
}

impl Default for MasterComponentConfig {
    fn default() -> Self {
        Self {
            drain_timeout_ms: 5000,
            delegate_retry_max_attempts: Some(30),
        }
    }
}
