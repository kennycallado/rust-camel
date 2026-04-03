#[derive(Debug, Clone, PartialEq)]
pub struct HttpConfig {
    pub connect_timeout_ms: u64,
    pub pool_max_idle_per_host: usize,
    pub pool_idle_timeout_ms: u64,
    pub follow_redirects: bool,
    pub response_timeout_ms: u64,
    pub max_body_size: usize,
    pub max_request_body: usize,
    pub allow_private_ips: bool,
    pub blocked_hosts: Vec<String>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 5_000,
            pool_max_idle_per_host: 100,
            pool_idle_timeout_ms: 90_000,
            follow_redirects: false,
            response_timeout_ms: 30_000,
            max_body_size: 10_485_760,
            max_request_body: 2_097_152,
            allow_private_ips: false,
            blocked_hosts: Vec::new(),
        }
    }
}

impl HttpConfig {
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
    pub fn with_response_timeout_ms(mut self, ms: u64) -> Self {
        self.response_timeout_ms = ms;
        self
    }
    pub fn with_max_body_size(mut self, n: usize) -> Self {
        self.max_body_size = n;
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
        assert_eq!(cfg.response_timeout_ms, 30_000);
        assert_eq!(cfg.max_body_size, 10_485_760);
        assert_eq!(cfg.max_request_body, 2_097_152);
        assert!(!cfg.allow_private_ips);
        assert!(cfg.blocked_hosts.is_empty());
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
}
