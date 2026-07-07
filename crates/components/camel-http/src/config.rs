use serde::Deserialize;

use camel_component_api::CamelError;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HttpConfig {
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_pool_max_idle_per_host")]
    pub pool_max_idle_per_host: usize,
    #[serde(default = "default_pool_idle_timeout_ms")]
    pub pool_idle_timeout_ms: u64,
    #[serde(default)]
    pub follow_redirects: bool,
    #[serde(default)]
    pub max_redirects: Option<usize>,
    #[serde(default = "default_response_timeout_ms")]
    pub response_timeout_ms: u64,
    #[serde(default = "default_read_timeout_ms")]
    pub read_timeout_ms: u64,
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
    #[serde(default = "default_max_request_body")]
    pub max_request_body: usize,
    #[serde(default)]
    pub allow_private_ips: bool,
    #[serde(default)]
    pub blocked_hosts: Vec<String>,
    #[serde(default)]
    pub ok_status_code_range: Option<String>,
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    #[serde(default)]
    pub proxy_url: Option<String>,
}

/// TLS configuration for HTTP/HTTPS client connections.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TlsConfig {
    /// Enables TLS customization for client connections.
    pub enabled: bool,
    /// Verifies peer certificates when true. Defaults to true.
    #[serde(default = "default_verify_peer")]
    pub verify_peer: bool,
    /// Optional path to custom CA certificate bundle (PEM or DER).
    #[serde(default)]
    pub ca_cert_path: Option<String>,
    /// Optional path to client certificate for mTLS (PEM).
    #[serde(default)]
    pub client_cert_path: Option<String>,
    /// Optional path to client private key for mTLS (PEM).
    #[serde(default)]
    pub client_key_path: Option<String>,
    /// If true, skips certificate verification (discouraged).
    #[serde(default)]
    pub insecure: bool,
}

fn default_verify_peer() -> bool {
    true
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            verify_peer: default_verify_peer(),
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            insecure: false,
        }
    }
}

/// Server-side TLS configuration for HTTP consumer endpoints.
///
/// When present, the HTTP server binds with TLS via `axum_server::from_tcp_rustls`.
/// When absent, the server binds plain HTTP via `axum::serve`.
#[derive(Clone)]
pub struct ServerTlsConfig {
    /// Path to the PEM-encoded server certificate chain.
    pub cert_path: String,
    /// Path to the PEM-encoded server private key.
    pub key_path: String,
}

impl std::fmt::Debug for ServerTlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerTlsConfig")
            .field("cert_path", &"[REDACTED]")
            .field("key_path", &"[REDACTED]")
            .finish()
    }
}

fn default_connect_timeout_ms() -> u64 {
    5_000
}

fn default_pool_max_idle_per_host() -> usize {
    100
}

fn default_pool_idle_timeout_ms() -> u64 {
    90_000
}

fn default_response_timeout_ms() -> u64 {
    30_000
}

fn default_read_timeout_ms() -> u64 {
    30_000
}

fn default_max_body_size() -> usize {
    10_485_760
}

fn default_max_response_bytes() -> usize {
    10_485_760
}

fn default_max_request_body() -> usize {
    2_097_152
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            connect_timeout_ms: default_connect_timeout_ms(),
            pool_max_idle_per_host: default_pool_max_idle_per_host(),
            pool_idle_timeout_ms: default_pool_idle_timeout_ms(),
            follow_redirects: false,
            max_redirects: None,
            response_timeout_ms: default_response_timeout_ms(),
            read_timeout_ms: default_read_timeout_ms(),
            max_body_size: default_max_body_size(),
            max_response_bytes: default_max_response_bytes(),
            max_request_body: default_max_request_body(),
            allow_private_ips: false,
            blocked_hosts: Vec::new(),
            ok_status_code_range: None,
            tls: None,
            proxy_url: None,
        }
    }
}

impl HttpConfig {
    pub fn validate(&self) -> Result<(), CamelError> {
        if let Some(max_redirects) = self.max_redirects
            && max_redirects > 20
        {
            return Err(CamelError::Config(
                "max_redirects must be <= 20".to_string(),
            ));
        }

        if let Some(range) = &self.ok_status_code_range {
            parse_ok_status_code_range(range)?;
        }

        if let Some(proxy_url) = &self.proxy_url {
            reqwest::Proxy::all(proxy_url)
                .map_err(|e| CamelError::Config(format!("invalid proxy_url: {e}")))?;
        }

        Ok(())
    }

    pub fn with_connect_timeout_ms(mut self, ms: u64) -> Self {
        self.connect_timeout_ms = ms;
        self
    }
    pub fn with_pool_max_idle_per_host(mut self, n: usize) -> Self {
        self.pool_max_idle_per_host = n;
        self
    }
    pub fn with_pool_idle_timeout_ms(mut self, ms: u64) -> Self {
        self.pool_idle_timeout_ms = ms;
        self
    }
    pub fn with_follow_redirects(mut self, follow: bool) -> Self {
        self.follow_redirects = follow;
        self
    }
    pub fn with_max_redirects(mut self, max_redirects: Option<usize>) -> Self {
        self.max_redirects = max_redirects;
        self
    }
    pub fn with_response_timeout_ms(mut self, ms: u64) -> Self {
        self.response_timeout_ms = ms;
        self
    }
    pub fn with_read_timeout_ms(mut self, ms: u64) -> Self {
        self.read_timeout_ms = ms;
        self
    }
    pub fn with_max_body_size(mut self, n: usize) -> Self {
        self.max_body_size = n;
        self
    }
    pub fn with_max_response_bytes(mut self, n: usize) -> Self {
        self.max_response_bytes = n;
        self
    }
    pub fn with_max_request_body(mut self, n: usize) -> Self {
        self.max_request_body = n;
        self
    }
    pub fn with_allow_private_ips(mut self, allow: bool) -> Self {
        self.allow_private_ips = allow;
        self
    }
    pub fn with_blocked_hosts(mut self, hosts: Vec<String>) -> Self {
        self.blocked_hosts = hosts;
        self
    }
    pub fn with_ok_status_code_range(mut self, range: Option<String>) -> Self {
        self.ok_status_code_range = range;
        self
    }
    pub fn with_tls(mut self, tls: Option<TlsConfig>) -> Self {
        self.tls = tls;
        self
    }
    pub fn with_proxy_url(mut self, proxy_url: Option<String>) -> Self {
        self.proxy_url = proxy_url;
        self
    }
}

pub(crate) fn parse_ok_status_code_range(range: &str) -> Result<(u16, u16), CamelError> {
    let (start_str, end_str) = range.split_once('-').ok_or_else(|| {
        CamelError::Config("ok_status_code_range must be in NNN-NNN format".to_string())
    })?;

    if start_str.len() != 3
        || end_str.len() != 3
        || !start_str.chars().all(|c| c.is_ascii_digit())
        || !end_str.chars().all(|c| c.is_ascii_digit())
    {
        return Err(CamelError::Config(
            "ok_status_code_range must be in NNN-NNN format".to_string(),
        ));
    }

    let start = start_str
        .parse::<u16>()
        .map_err(|_| CamelError::Config("ok_status_code_range start is invalid".to_string()))?;
    let end = end_str
        .parse::<u16>()
        .map_err(|_| CamelError::Config("ok_status_code_range end is invalid".to_string()))?;

    if start > end {
        return Err(CamelError::Config(
            "ok_status_code_range start must be <= end".to_string(),
        ));
    }

    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_config_defaults() {
        let cfg = HttpConfig::default();
        assert_eq!(cfg.connect_timeout_ms, 5_000);
        assert_eq!(cfg.pool_max_idle_per_host, 100);
        assert_eq!(cfg.pool_idle_timeout_ms, 90_000);
        assert!(!cfg.follow_redirects);
        assert_eq!(cfg.max_redirects, None);
        assert_eq!(cfg.response_timeout_ms, 30_000);
        assert_eq!(cfg.max_body_size, 10_485_760);
        assert_eq!(cfg.max_request_body, 2_097_152);
        assert!(!cfg.allow_private_ips);
        assert!(cfg.blocked_hosts.is_empty());
        assert!(cfg.tls.is_none());
        assert!(cfg.proxy_url.is_none());
    }

    #[test]
    fn test_http_config_builder() {
        let cfg = HttpConfig::default()
            .with_connect_timeout_ms(1_000)
            .with_pool_max_idle_per_host(50)
            .with_follow_redirects(true)
            .with_allow_private_ips(true)
            .with_blocked_hosts(vec!["evil.com".to_string()]);
        assert_eq!(cfg.connect_timeout_ms, 1_000);
        assert_eq!(cfg.pool_max_idle_per_host, 50);
        assert!(cfg.follow_redirects);
        assert!(cfg.allow_private_ips);
        assert_eq!(cfg.blocked_hosts, vec!["evil.com".to_string()]);
        assert_eq!(cfg.response_timeout_ms, 30_000);
    }

    #[test]
    fn test_rejects_max_redirects_over_limit() {
        let cfg = HttpConfig {
            max_redirects: Some(21),
            ..HttpConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_accepts_valid_max_redirects() {
        let cfg = HttpConfig {
            max_redirects: Some(10),
            ..HttpConfig::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_rejects_malformed_status_range() {
        let cfg = HttpConfig {
            ok_status_code_range: Some("abc-xyz".into()),
            ..HttpConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_accepts_valid_status_range() {
        let cfg = HttpConfig {
            ok_status_code_range: Some("200-299".into()),
            ..HttpConfig::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_rejects_invalid_proxy_url() {
        let cfg = HttpConfig {
            proxy_url: Some("::not-a-proxy::".into()),
            ..HttpConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn server_tls_config_debug_redacts_paths() {
        let cfg = super::ServerTlsConfig {
            cert_path: "/secret/cert.pem".to_string(),
            key_path: "/secret/key.pem".to_string(),
        };
        let debug = format!("{:?}", cfg);
        assert!(
            !debug.contains("/secret"),
            "paths must be redacted: {debug}"
        );
        assert!(debug.contains("REDACTED"), "must show REDACTED: {debug}");
    }
}
