use camel_api::CamelError;

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
pub struct MasterComponentConfig {
    pub drain_timeout_ms: u64,
    pub delegate_retry_max_attempts: Option<u32>,
}

impl Default for MasterComponentConfig {
    fn default() -> Self {
        Self {
            drain_timeout_ms: 5000,
            delegate_retry_max_attempts: Some(30),
        }
    }
}
